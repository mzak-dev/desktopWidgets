//! One module in wasmi, under a fuel budget and a memory cap (ADR-0008). The ABI is JSON
//! in linear memory; results of host calls are parked here and fetched with `take`, so the
//! host never calls into the guest from inside an import.

use std::sync::Arc;
use std::time::{Duration, Instant};

use wasmi::{Caller, CompilationMode, Config, EnforcedLimits, Engine, Extern, Instance, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TrapCode, TypedFunc};

use super::store::KvStore;

pub const ABI: i32 = 1;
const HOST_FUNCS: &[&str] = &["log", "now_ms", "http", "store_get", "store_set", "take"];
const EXPORTS: &[&str] = &["memory", "wf_abi", "wf_alloc", "wf_sample"];

#[derive(Clone, Debug)]
pub struct Limits {
    pub fuel_call: u64,
    pub fuel_init: u64,
    pub memory: usize,
    pub input: usize,
    pub output: usize,
    pub log_line: usize,
    pub logs_per_minute: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { fuel_call: 250_000_000, fuel_init: 50_000_000, memory: 32 << 20, input: 64 << 10, output: 1 << 20, log_line: 512, logs_per_minute: 20 }
    }
}

/// An HTTP request (JSON) to its response (JSON, errors included).
pub type HttpFn = Arc<dyn Fn(&[u8]) -> Vec<u8> + Send + Sync>;

/// What the host lends a module.
pub struct Env {
    http: Option<HttpFn>,
    store: Option<Arc<KvStore>>,
    pub logs: Vec<String>,
    used_net: bool,
    parked: Vec<u8>,
    limits: StoreLimits,
    input_cap: usize,
    log_line: usize,
    log_window: (Instant, usize, usize),
}

impl Env {
    pub fn new(http: Option<HttpFn>, store: Option<Arc<KvStore>>, l: &Limits) -> Env {
        let limits = StoreLimitsBuilder::new().memory_size(l.memory).instances(1).tables(1).memories(1).trap_on_grow_failure(false).build();
        Env { http, store, logs: Vec::new(), used_net: false, parked: Vec::new(), limits, input_cap: l.input, log_line: l.log_line, log_window: (Instant::now(), 0, l.logs_per_minute) }
    }

    fn log(&mut self, line: String) {
        let (start, n, cap) = &mut self.log_window;
        if start.elapsed() >= Duration::from_secs(60) {
            (*start, *n) = (Instant::now(), 0);
        }
        if *n < *cap {
            *n += 1;
            self.logs.push(line);
        }
    }
}

pub struct Compiled {
    engine: Engine,
    module: Module,
}

impl Compiled {
    /// Compiles eagerly, so a bad module fails here rather than on some later call, and a
    /// call's fuel pays only for running.
    pub fn load(wasm: &[u8]) -> Result<Compiled, String> {
        let mut config = Config::default();
        config.consume_fuel(true).compilation_mode(CompilationMode::Eager).enforced_limits(EnforcedLimits::strict()).set_max_recursion_depth(1024);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, wasm).map_err(|e| format!("not a module Wayfinder can run: {e}"))?;
        for i in module.imports() {
            if i.module() != "wf" || !HOST_FUNCS.contains(&i.name()) {
                let hint = if i.module().starts_with("wasi") { " (built for WASI? Build for wasm32-unknown-unknown)" } else { "" };
                return Err(format!("it needs `{}.{}`, which Wayfinder does not provide{hint}", i.module(), i.name()));
            }
        }
        let exports: Vec<&str> = module.exports().map(|e| e.name()).collect();
        if let Some(missing) = EXPORTS.iter().find(|e| !exports.contains(e)) {
            return Err(format!("it does not export `{missing}`; build it with the wayfinder-plugin crate"));
        }
        Ok(Compiled { engine, module })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    Sample,
    Act,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Fault {
    OutOfFuel,
    Trap(String),
    BadOutput(String),
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fault::OutOfFuel => write!(f, "it ran too long and was stopped"),
            Fault::Trap(e) => write!(f, "it crashed: {e}"),
            Fault::BadOutput(e) => write!(f, "{e}"),
        }
    }
}

#[derive(Debug)]
pub struct Called {
    pub output: Vec<u8>,
    pub fuel: u64,
    pub cpu: Duration,
    pub used_net: bool,
}

fn fault(e: wasmi::Error) -> Fault {
    match e.as_trap_code() {
        Some(TrapCode::OutOfFuel) => Fault::OutOfFuel,
        _ => Fault::Trap(e.to_string()),
    }
}

fn guest_memory(caller: &Caller<'_, Env>) -> Result<Memory, wasmi::Error> {
    caller.get_export("memory").and_then(Extern::into_memory).ok_or_else(|| wasmi::Error::new("no memory export"))
}

/// `len` bytes at `ptr`, refusing negative or oversized lengths before allocating.
fn read_guest(caller: &Caller<'_, Env>, ptr: i32, len: i32, cap: usize) -> Result<Vec<u8>, wasmi::Error> {
    if ptr < 0 || len < 0 || len as usize > cap {
        return Err(wasmi::Error::new(format!("a host call was passed {len} bytes at {ptr}")));
    }
    let mut buf = vec![0; len as usize];
    guest_memory(caller)?.read(caller, ptr as usize, &mut buf).map_err(|e| wasmi::Error::new(e.to_string()))?;
    Ok(buf)
}

fn park(caller: &mut Caller<'_, Env>, bytes: Vec<u8>) -> i32 {
    let n = bytes.len() as i32;
    caller.data_mut().parked = bytes;
    n
}

fn linker(engine: &Engine) -> Linker<Env> {
    let mut l = Linker::new(engine);
    l.func_wrap("wf", "log", |caller: Caller<'_, Env>, ptr: i32, len: i32| -> Result<(), wasmi::Error> {
        let cap = caller.data().log_line;
        let bytes = read_guest(&caller, ptr, len.min(cap as i32), cap)?;
        let mut caller = caller;
        caller.data_mut().log(String::from_utf8_lossy(&bytes).into_owned());
        Ok(())
    })
    .expect("log");
    l.func_wrap("wf", "now_ms", || -> i64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64) }).expect("now_ms");
    l.func_wrap("wf", "http", |mut caller: Caller<'_, Env>, ptr: i32, len: i32| -> Result<i32, wasmi::Error> {
        let req = read_guest(&caller, ptr, len, caller.data().input_cap)?;
        let resp = match caller.data().http.clone() {
            Some(http) => {
                caller.data_mut().used_net = true;
                http(&req)
            }
            None => br#"{"error":"this plugin may not use the network"}"#.to_vec(),
        };
        Ok(park(&mut caller, resp))
    })
    .expect("http");
    l.func_wrap("wf", "store_get", |mut caller: Caller<'_, Env>, ptr: i32, len: i32| -> Result<i32, wasmi::Error> {
        let key = String::from_utf8_lossy(&read_guest(&caller, ptr, len, caller.data().input_cap)?).into_owned();
        match caller.data().store.as_ref().and_then(|s| s.get(&key)) {
            Some(v) => Ok(park(&mut caller, v.into_bytes())),
            None => Ok(-1),
        }
    })
    .expect("store_get");
    l.func_wrap("wf", "store_set", |caller: Caller<'_, Env>, kptr: i32, klen: i32, vptr: i32, vlen: i32| -> Result<i32, wasmi::Error> {
        let cap = super::store::STORE_CAP;
        let key = String::from_utf8_lossy(&read_guest(&caller, kptr, klen, cap)?).into_owned();
        let value = if vlen == 0 { None } else { Some(String::from_utf8_lossy(&read_guest(&caller, vptr, vlen, cap)?).into_owned()) };
        Ok(match caller.data().store.as_ref().map(|s| s.set(&key, value)) {
            Some(Ok(())) => 0,
            _ => -1,
        })
    })
    .expect("store_set");
    l.func_wrap("wf", "take", |mut caller: Caller<'_, Env>, ptr: i32| -> Result<(), wasmi::Error> {
        let bytes = std::mem::take(&mut caller.data_mut().parked);
        if ptr < 0 {
            return Err(wasmi::Error::new("take: negative pointer"));
        }
        let memory = guest_memory(&caller)?;
        memory.write(&mut caller, ptr as usize, &bytes).map_err(|e| wasmi::Error::new(e.to_string()))
    })
    .expect("take");
    l
}

pub struct Runtime {
    store: Store<Env>,
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
    sample: TypedFunc<(i32, i32), i64>,
    act: Option<TypedFunc<(i32, i32), i64>>,
    limits: Limits,
}

impl Runtime {
    pub fn new(c: &Compiled, env: Env, limits: &Limits) -> Result<Runtime, Fault> {
        let mut store = Store::new(&c.engine, env);
        store.limiter(|env| &mut env.limits);
        store.set_fuel(limits.fuel_init).map_err(fault)?;
        let instance: Instance = linker(&c.engine).instantiate_and_start(&mut store, &c.module).map_err(fault)?;
        let bad = |what: &str| Fault::BadOutput(format!("its `{what}` export has the wrong type"));
        let memory = instance.get_memory(&store, "memory").ok_or_else(|| bad("memory"))?;
        let abi = instance.get_typed_func::<(), i32>(&store, "wf_abi").map_err(|_| bad("wf_abi"))?.call(&mut store, ()).map_err(fault)?;
        if abi != ABI {
            return Err(Fault::BadOutput(format!("it was built for plugin ABI {abi}; this Wayfinder runs ABI {ABI}")));
        }
        let alloc = instance.get_typed_func::<i32, i32>(&store, "wf_alloc").map_err(|_| bad("wf_alloc"))?;
        let sample = instance.get_typed_func::<(i32, i32), i64>(&store, "wf_sample").map_err(|_| bad("wf_sample"))?;
        let act = instance.get_typed_func::<(i32, i32), i64>(&store, "wf_act").ok();
        Ok(Runtime { store, memory, alloc, sample, act, limits: limits.clone() })
    }

    pub fn env_mut(&mut self) -> &mut Env {
        self.store.data_mut()
    }

    pub fn has_act(&self) -> bool {
        self.act.is_some()
    }

    /// Hands `input` to an entry point and copies its output out.
    pub fn call(&mut self, entry: Entry, input: &[u8]) -> Result<Called, Fault> {
        let f = match entry {
            Entry::Sample => self.sample,
            Entry::Act => self.act.ok_or_else(|| Fault::BadOutput("it has no actions".into()))?,
        };
        if input.len() > self.limits.input {
            return Err(Fault::BadOutput(format!("its input would be {} KB", input.len() >> 10)));
        }
        self.store.set_fuel(self.limits.fuel_call).map_err(fault)?;
        self.store.data_mut().used_net = false;
        let t = Instant::now();
        let ptr = self.alloc.call(&mut self.store, input.len() as i32).map_err(fault)?;
        self.memory.write(&mut self.store, ptr.max(0) as usize, input).map_err(|e| Fault::BadOutput(format!("wf_alloc gave a bad pointer: {e}")))?;
        let packed = f.call(&mut self.store, (ptr, input.len() as i32)).map_err(fault)?;
        let (optr, olen) = ((packed as u64 >> 32) as usize, (packed as u64 & 0xffff_ffff) as usize);
        if olen > self.limits.output {
            return Err(Fault::BadOutput(format!("its output is {} KB, over the {} KB limit", olen >> 10, self.limits.output >> 10)));
        }
        let mut output = vec![0; olen];
        self.memory.read(&self.store, optr, &mut output).map_err(|_| Fault::BadOutput("its output points outside its memory".into()))?;
        let fuel = self.limits.fuel_call - self.store.get_fuel().unwrap_or(0);
        Ok(Called { output, fuel, cpu: t.elapsed(), used_net: self.store.data().used_net })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn escape(s: &str) -> String {
        s.replace('\\', "\\\\").replace('"', "\\\"")
    }

    /// A module with a bump allocator from 1024; `body` is `wf_sample`'s body, ending in the
    /// packed result. Returns `(ptr, len)` of `data` at 16 via `$out`.
    pub(crate) fn module(imports: &str, data: &str, body: &str, abi: i32) -> String {
        format!(
            r#"(module
              {imports}
              (memory (export "memory") 1)
              (global $bump (mut i32) (i32.const 1024))
              (func (export "wf_abi") (result i32) i32.const {abi})
              (func (export "wf_alloc") (param $n i32) (result i32) (local $p i32)
                global.get $bump local.set $p
                global.get $bump local.get $n i32.add global.set $bump
                local.get $p)
              (data (i32.const 16) "{data}")
              (func $out (result i64) i64.const 16 i64.const 32 i64.shl i64.const {len} i64.or)
              (func (export "wf_sample") (param $ptr i32) (param $len i32) (result i64) {body})
              (func (export "wf_act") (param $ptr i32) (param $len i32) (result i64) (call $out)))"#,
            data = escape(data),
            len = data.len(),
        )
    }

    pub(crate) fn returning(json: &str) -> String {
        module("", json, "(call $out)", ABI)
    }

    fn run(wat: &str, env: Env, limits: &Limits) -> Result<Called, Fault> {
        let c = Compiled::load(wat.as_bytes()).expect("compiles");
        Runtime::new(&c, env, limits)?.call(Entry::Sample, b"{}")
    }

    fn env() -> Env {
        Env::new(None, None, &Limits::default())
    }

    #[test]
    fn constant_json_comes_back() {
        let out = run(&returning(r#"{"value":{"temp":21}}"#), env(), &Limits::default()).unwrap();
        assert_eq!(out.output, br#"{"value":{"temp":21}}"#);
        assert!(!out.used_net);
        let echo = module("", "", "local.get $ptr i64.extend_i32_u i64.const 32 i64.shl local.get $len i64.extend_i32_u i64.or", ABI);
        let c = Compiled::load(echo.as_bytes()).unwrap();
        let mut rt = Runtime::new(&c, env(), &Limits::default()).unwrap();
        assert_eq!(rt.call(Entry::Sample, br#"{"params":{"city":"Oslo"}}"#).unwrap().output, br#"{"params":{"city":"Oslo"}}"#, "input reaches the module intact");
        assert_eq!(rt.call(Entry::Act, b"{}").unwrap().output, b"");
    }

    #[test]
    fn an_endless_loop_runs_out_of_fuel() {
        let wat = module("", "", "(loop $l (br $l)) (call $out)", ABI);
        let limits = Limits { fuel_call: 200_000, ..Limits::default() };
        assert_eq!(run(&wat, env(), &limits).unwrap_err(), Fault::OutOfFuel);
    }

    #[test]
    fn memory_cannot_grow_past_the_limit() {
        let body = "(if (i32.ne (memory.grow (i32.const 1000)) (i32.const -1)) (then unreachable)) (call $out)";
        assert!(run(&module("", "refused", body, ABI), env(), &Limits::default()).is_ok(), "64 MB more is refused, not granted");
        let small = "(if (i32.eq (memory.grow (i32.const 10)) (i32.const -1)) (then unreachable)) (call $out)";
        assert!(run(&module("", "ok", small, ABI), env(), &Limits::default()).is_ok(), "a little more is fine");
    }

    #[test]
    fn a_wrong_abi_is_refused() {
        let c = Compiled::load(module("", "{}", "(call $out)", 2).as_bytes()).unwrap();
        let e = Runtime::new(&c, env(), &Limits::default()).err().unwrap();
        assert!(e.to_string().contains("ABI 2"), "{e}");
    }

    #[test]
    fn wasi_imports_are_refused_at_load() {
        let wat = module(r#"(import "wasi_snapshot_preview1" "fd_write" (func (param i32 i32 i32 i32) (result i32)))"#, "{}", "(call $out)", ABI);
        let e = Compiled::load(wat.as_bytes()).err().unwrap();
        assert!(e.contains("WASI") && e.contains("wasm32-unknown-unknown"), "{e}");
        let odd = module(r#"(import "env" "abort" (func))"#, "{}", "(call $out)", ABI);
        assert!(Compiled::load(odd.as_bytes()).err().unwrap().contains("env.abort"));
        assert!(Compiled::load(b"(module)").err().unwrap().contains("memory"), "the ABI's exports are required");
        assert!(Compiled::load(b"not wasm at all").is_err());
    }

    #[test]
    fn out_of_bounds_or_oversized_output_is_refused() {
        let far = module("", "", "i64.const 70000 i64.const 32 i64.shl i64.const 10 i64.or", ABI);
        assert!(matches!(run(&far, env(), &Limits::default()), Err(Fault::BadOutput(_))));
        let limits = Limits { output: 4, ..Limits::default() };
        let e = run(&returning(r#"{"value":1}"#), env(), &limits).unwrap_err();
        assert!(matches!(e, Fault::BadOutput(ref m) if m.contains("limit")), "{e:?}");
        let trap = module("", "", "unreachable", ABI);
        assert!(matches!(run(&trap, env(), &Limits::default()), Err(Fault::Trap(_))));
    }

    #[test]
    fn store_calls_round_trip_through_take() {
        let imports = r#"(import "wf" "store_set" (func $set (param i32 i32 i32 i32) (result i32)))
                         (import "wf" "store_get" (func $get (param i32 i32) (result i32)))
                         (import "wf" "take" (func $take (param i32)))"#;
        let body = r#"(local $n i32)
            (drop (call $set (i32.const 16) (i32.const 1) (i32.const 17) (i32.const 1)))
            (local.set $n (call $get (i32.const 16) (i32.const 1)))
            (call $take (i32.const 512))
            i64.const 512 i64.const 32 i64.shl local.get $n i64.extend_i32_u i64.or"#;
        let dir = std::env::temp_dir().join(format!("wf-rt-store-{}", std::process::id()));
        let store = Arc::new(KvStore::open(&dir.join("p.json")));
        let out = run(&module(imports, "kv", body, ABI), Env::new(None, Some(store.clone()), &Limits::default()), &Limits::default()).unwrap();
        assert_eq!(out.output, b"v");
        assert_eq!(store.get("k").as_deref(), Some("v"));
        let missing = r#"(local $n i32) (local.set $n (call $get (i32.const 16) (i32.const 1))) (if (i32.ne (local.get $n) (i32.const -1)) (then unreachable)) (call $out)"#;
        assert!(run(&module(imports, "kv", missing, ABI), env(), &Limits::default()).is_ok(), "no store: every key is missing");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn http_goes_through_the_host_and_is_marked() {
        let imports = r#"(import "wf" "http" (func $http (param i32 i32) (result i32)))
                         (import "wf" "take" (func $take (param i32)))"#;
        let body = r#"(local $n i32)
            (local.set $n (call $http (i32.const 16) (i32.const 2)))
            (call $take (i32.const 512))
            i64.const 512 i64.const 32 i64.shl local.get $n i64.extend_i32_u i64.or"#;
        let http: HttpFn = Arc::new(|req: &[u8]| format!(r#"{{"status":200,"echo":"{}"}}"#, String::from_utf8_lossy(req)).into_bytes());
        let out = run(&module(imports, "{}", body, ABI), Env::new(Some(http), None, &Limits::default()), &Limits::default()).unwrap();
        assert_eq!(out.output, br#"{"status":200,"echo":"{}"}"#);
        assert!(out.used_net);
        let refused = run(&module(imports, "{}", body, ABI), env(), &Limits::default()).unwrap();
        assert!(String::from_utf8_lossy(&refused.output).contains("may not use the network"));
        assert!(!refused.used_net);
    }

    #[test]
    fn log_lines_are_capped() {
        let imports = r#"(import "wf" "log" (func $log (param i32 i32)))"#;
        let body = r#"(local $i i32)
            (loop $l
              (call $log (i32.const 0) (i32.const 2000))
              (local.set $i (i32.add (local.get $i) (i32.const 1)))
              (br_if $l (i32.lt_u (local.get $i) (i32.const 30))))
            (call $out)"#;
        let c = Compiled::load(module(imports, "{}", body, ABI).as_bytes()).unwrap();
        let mut rt = Runtime::new(&c, env(), &Limits::default()).unwrap();
        rt.call(Entry::Sample, b"{}").unwrap();
        let logs = &rt.env_mut().logs;
        assert_eq!(logs.len(), 20, "20 lines a minute");
        assert!(logs.iter().all(|l| l.len() <= 512));
    }
}
