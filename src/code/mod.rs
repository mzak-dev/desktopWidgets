//! Code Sources (ADR-0008): a Plugin's WebAssembly module serving a Data Source. Each one
//! owns a worker thread; the UI thread only reads the last values and never waits.

pub mod fs;
pub mod launch;
pub mod runtime;
pub mod schedule;
pub mod store;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::data::{Cadence, DataSource, News, SourceCx};
use crate::net::{Fetch, HostPattern, Net};
use crate::value::Value;

use self::runtime::{ABI, Called, Compiled, Entry, Env, Fault, HttpFn, Limits, Runtime};
use self::schedule::{Job, Outcome, Resample, Schedule};
use self::store::KvStore;

/// A Plugin's `[code]`: which module serves which source, and what it may reach.
#[derive(Clone, Debug, PartialEq)]
pub struct CodeSpec {
    pub plugin: String,
    pub source: String,
    pub module: PathBuf,
    pub hosts: Vec<HostPattern>,
    /// Folders under home it may read.
    pub fs_read: Vec<fs::FsRoot>,
    /// Params whose value, a folder the user picked, it may read for that Instance.
    pub fs_read_params: Vec<String>,
    /// What Widgets showing its values may open besides https:// links.
    pub launch: Vec<launch::LaunchRule>,
    /// Shown until the first sample, with `loading` set.
    pub initial: Value,
}

/// What a source borrows from the app.
#[derive(Clone)]
pub struct Deps {
    pub fetch: Option<Arc<dyn Fetch>>,
    pub store: Option<Arc<KvStore>>,
    /// Wakes the app to call `take_news`.
    pub notify: Arc<dyn Fn() + Send + Sync>,
    pub limits: Limits,
    pub places: fs::Places,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Starting,
    Ready { fuel: u64 },
    Failing { error: String, retry_in: Duration },
    /// The module cannot run at all (bad file, wrong ABI); only a new file helps.
    Broken(String),
}

impl Status {
    /// One line for the Plugins page.
    pub fn line(&self) -> String {
        match self {
            Status::Starting => "Starting".into(),
            Status::Ready { .. } => "Running".into(),
            Status::Failing { error, retry_in } => format!("Error: {error}; trying again in {} s", retry_in.as_secs().max(1)),
            Status::Broken(e) => format!("Cannot run: {e}"),
        }
    }
}

enum Msg {
    Need { instance: String, params: String, picked: Vec<PathBuf> },
    Act { instance: String, params: String, picked: Vec<PathBuf>, verb: String, arg: String },
    Retain(BTreeSet<String>),
}

struct Slot {
    params: String,
    value: Value,
}

struct Shared {
    slots: Mutex<HashMap<String, Slot>>,
    news: Mutex<News>,
    status: Mutex<Status>,
}

pub struct WasmSource {
    name: String,
    tx: Sender<Msg>,
    shared: Arc<Shared>,
    initial: Value,
    fs_read_params: Vec<String>,
    launch: Vec<launch::LaunchRule>,
    worker: JoinHandle<()>,
}

fn params_json(params: &BTreeMap<String, Value>) -> String {
    serde_json::Value::Object(params.iter().map(|(k, v)| (k.clone(), v.into())).collect()).to_string()
}

/// `v` (an object) with the engine's own fields set, so bindings always resolve.
fn with_state(v: &Value, loading: bool, error: &str) -> Value {
    let mut o = match v {
        Value::Obj(o) => o.clone(),
        _ => BTreeMap::new(),
    };
    o.insert("loading".into(), Value::Bool(loading));
    o.insert("error".into(), Value::Str(error.into()));
    Value::Obj(o)
}

impl WasmSource {
    pub fn start(spec: CodeSpec, deps: Deps) -> WasmSource {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared { slots: Mutex::new(HashMap::new()), news: Mutex::new(News::default()), status: Mutex::new(Status::Starting) });
        let (name, initial, fs_read_params, launch) = (spec.source.clone(), with_state(&spec.initial, true, ""), spec.fs_read_params.clone(), spec.launch.clone());
        let worker = {
            let shared = shared.clone();
            std::thread::Builder::new().name(format!("plugin {}", spec.plugin)).spawn(move || Worker::new(spec, deps, shared).run(rx)).expect("spawn a plugin worker")
        };
        WasmSource { name, tx, shared, initial, fs_read_params, launch, worker }
    }

    pub fn launch_rules(&self) -> &[launch::LaunchRule] {
        &self.launch
    }

    /// The folders the user picked for this Instance in the params the plugin may read.
    /// Only the Instance's own settings count, never a widget's defaults.
    fn picked(&self, cx: &SourceCx) -> Vec<PathBuf> {
        self.fs_read_params.iter().filter_map(|p| cx.cfg.params.get(p)?.as_str()).map(PathBuf::from).filter(|p| p.is_absolute()).collect()
    }

    pub fn take_news(&self) -> News {
        std::mem::take(&mut *self.shared.news.lock().unwrap())
    }

    pub fn status(&self) -> Status {
        self.shared.status.lock().unwrap().clone()
    }

    /// Closes the channel and hands back the thread, which ends after its current call.
    pub fn stop(self) -> JoinHandle<()> {
        drop(self.tx);
        self.worker
    }
}

impl DataSource for WasmSource {
    fn name(&self) -> &str {
        &self.name
    }

    /// Never blocks: the last value for this Instance, or the placeholder while one is fetched.
    fn value(&self, cx: &SourceCx) -> Value {
        if let Status::Broken(e) = &*self.shared.status.lock().unwrap() {
            return with_state(&self.initial, false, &format!("the plugin's code cannot run: {e}"));
        }
        let params = params_json(cx.params);
        let mut slots = self.shared.slots.lock().unwrap();
        if let Some(s) = slots.get(&cx.cfg.id).filter(|s| s.params == params) {
            return s.value.clone();
        }
        let shown = slots.get(&cx.cfg.id).map_or_else(|| self.initial.clone(), |s| with_state(&s.value, true, ""));
        slots.insert(cx.cfg.id.clone(), Slot { params: params.clone(), value: shown.clone() });
        let _ = self.tx.send(Msg::Need { instance: cx.cfg.id.clone(), params, picked: self.picked(cx) });
        shown
    }

    /// Code values change when the worker says so, never with the clock.
    fn cadence(&self, _field: &str) -> Option<Cadence> {
        None
    }

    fn act(&self, verb: &str, arg: &str, cx: &SourceCx) -> bool {
        let _ = self.tx.send(Msg::Act { instance: cx.cfg.id.clone(), params: params_json(cx.params), picked: self.picked(cx), verb: verb.into(), arg: arg.into() });
        true
    }

    /// Forgets every Instance not in `live`.
    fn retain(&self, live: &BTreeSet<String>) {
        self.shared.slots.lock().unwrap().retain(|id, _| live.contains(id));
        let _ = self.tx.send(Msg::Retain(live.clone()));
    }
}

struct Worker {
    spec: CodeSpec,
    deps: Deps,
    shared: Arc<Shared>,
    http: Option<HttpFn>,
    compiled: Option<Compiled>,
    runtime: Option<Runtime>,
    schedule: Schedule,
    picked: HashMap<String, Vec<PathBuf>>,
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct SampleOut {
    value: Option<serde_json::Value>,
    error: Option<String>,
    refresh_s: Option<f64>,
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct ActOut {
    resample: Option<String>,
}

impl Worker {
    fn new(spec: CodeSpec, deps: Deps, shared: Arc<Shared>) -> Worker {
        let http = deps.fetch.clone().filter(|_| !spec.hosts.is_empty()).map(|f| {
            let net = Arc::new(Net::new(spec.hosts.clone(), f));
            Arc::new(move |req: &[u8]| net.handle(req)) as HttpFn
        });
        let schedule = Schedule::new(deps.limits.backoff);
        Worker { spec, deps, shared, http, compiled: None, runtime: None, schedule, picked: HashMap::new() }
    }

    fn run(mut self, rx: Receiver<Msg>) {
        match std::fs::read(&self.spec.module).map_err(|e| format!("{}: {e}", self.spec.module.display())).and_then(|w| Compiled::load(&w)) {
            Ok(c) => self.compiled = Some(c),
            Err(e) => return self.set_status(Status::Broken(e)),
        }
        loop {
            while let Some(job) = self.schedule.next(Instant::now()) {
                self.run_job(job);
                if matches!(*self.shared.status.lock().unwrap(), Status::Broken(_)) {
                    return;
                }
            }
            let now = Instant::now();
            let msg = match self.schedule.next_wake(now) {
                None => match rx.recv() {
                    Ok(m) => m,
                    Err(_) => return,
                },
                Some(t) => match rx.recv_timeout(t.saturating_duration_since(now)) {
                    Ok(m) => m,
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => return,
                },
            };
            self.handle(msg);
            while let Ok(m) = rx.try_recv() {
                self.handle(m);
            }
        }
    }

    fn handle(&mut self, msg: Msg) {
        let now = Instant::now();
        match msg {
            Msg::Need { instance, params, picked } => {
                self.schedule.need(&instance, &params, now);
                self.picked.insert(instance, picked);
            }
            Msg::Act { instance, params, picked, verb, arg } => {
                self.schedule.need(&instance, &params, now);
                self.schedule.act(&instance, &verb, &arg);
                self.picked.insert(instance, picked);
            }
            Msg::Retain(live) => {
                self.schedule.retain(&live);
                self.picked.retain(|id, _| live.contains(id));
            }
        }
    }

    fn set_status(&self, s: Status) {
        let mut cur = self.shared.status.lock().unwrap();
        if *cur != s {
            let changed = std::mem::discriminant(&*cur) != std::mem::discriminant(&s);
            *cur = s;
            drop(cur);
            if changed {
                self.shared.news.lock().unwrap().status_changed = true;
                (self.deps.notify)();
            }
        }
    }

    fn input(&self, instance: &str, extra: serde_json::Value) -> Vec<u8> {
        let params: serde_json::Value = self.schedule.params(instance).and_then(|p| serde_json::from_str(p).ok()).unwrap_or_default();
        let t = crate::data::now_local();
        let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        let mut v = serde_json::json!({
            "abi": ABI,
            "instance": instance,
            "params": params,
            "now_ms": now_ms,
            "local": { "year": t.year, "month": t.month, "day": t.day, "hour": t.hour, "minute": t.minute, "second": t.second },
        });
        if let (Some(o), serde_json::Value::Object(e)) = (v.as_object_mut(), extra) {
            o.extend(e);
        }
        v.to_string().into_bytes()
    }

    /// Calls the module for `instance`, building a fresh module instance if the last one faulted.
    fn call(&mut self, instance: &str, entry: Entry, input: &[u8]) -> Result<Called, Fault> {
        if self.runtime.is_none() {
            let env = Env::new(self.http.clone(), self.deps.store.clone(), &self.deps.limits);
            let compiled = self.compiled.as_ref().expect("compiled before any job");
            match Runtime::new(compiled, env, &self.deps.limits) {
                Ok(rt) => self.runtime = Some(rt),
                Err(Fault::BadOutput(e)) => {
                    self.set_status(Status::Broken(e.clone()));
                    return Err(Fault::BadOutput(e));
                }
                Err(f) => return Err(f),
            }
        }
        let rt = self.runtime.as_mut().expect("just built");
        let reads = !self.spec.fs_read.is_empty() || !self.spec.fs_read_params.is_empty();
        rt.env_mut().fs = reads.then(|| fs::Fs::new(&self.deps.places, &self.spec.fs_read, self.picked.get(instance).map_or(&[][..], Vec::as_slice)));
        let out = std::panic::catch_unwind(AssertUnwindSafe(|| rt.call(entry, input))).unwrap_or_else(|_| Err(Fault::Trap("the host panicked".into())));
        let logs = std::mem::take(&mut rt.env_mut().logs);
        if !logs.is_empty() {
            self.shared.news.lock().unwrap().logs.extend(logs);
        }
        if out.is_err() {
            self.runtime = None; // a trap may leave its statics broken
        }
        if let Some(s) = &self.deps.store {
            if let Err(e) = s.flush() {
                self.shared.news.lock().unwrap().logs.push(e);
            }
        }
        out
    }

    fn publish(&self, instance: &str, value: Value) {
        if let Some(s) = self.shared.slots.lock().unwrap().get_mut(instance) {
            s.value = value;
        }
        self.shared.news.lock().unwrap().changed.insert(instance.to_string());
        (self.deps.notify)();
    }

    fn last(&self, instance: &str) -> Value {
        self.shared.slots.lock().unwrap().get(instance).map_or_else(|| self.spec.initial.clone(), |s| s.value.clone())
    }

    fn faulted(&mut self, instance: &str, f: Fault, cpu: Duration) {
        let now = Instant::now();
        self.schedule.done(instance, Outcome::Faulted { cpu }, now);
        if matches!(*self.shared.status.lock().unwrap(), Status::Broken(_)) {
            return;
        }
        let retry_in = self.schedule.held_for(now).unwrap_or_default();
        self.set_status(Status::Failing { error: f.to_string(), retry_in });
        self.publish(instance, with_state(&self.last(instance), false, &f.to_string()));
    }

    fn run_job(&mut self, job: Job) {
        match job {
            Job::Sample(instance) => {
                let input = self.input(&instance, serde_json::json!({}));
                let called = match self.call(&instance, Entry::Sample, &input) {
                    Ok(c) => c,
                    Err(f) => return self.faulted(&instance, f, Duration::ZERO),
                };
                let out: SampleOut = match serde_json::from_slice(&called.output) {
                    Ok(o) => o,
                    Err(e) => return self.faulted(&instance, Fault::BadOutput(format!("its sample is not JSON: {e}")), called.cpu),
                };
                let refresh = out.refresh_s.filter(|s| s.is_finite() && *s > 0.0).map(Duration::from_secs_f64);
                let now = Instant::now();
                if !matches!(&out, SampleOut { value: None, error: None, .. }) {
                    // before the value goes out, so the app never sees a new value with an old status
                    self.set_status(Status::Ready { fuel: called.fuel });
                }
                match (out.value, out.error) {
                    (_, Some(error)) => {
                        self.schedule.done(&instance, Outcome::Errored { refresh, used_net: called.used_net, cpu: called.cpu }, now);
                        self.publish(&instance, with_state(&self.last(&instance), false, &error));
                    }
                    (Some(v @ serde_json::Value::Object(_)), None) => {
                        self.schedule.done(&instance, Outcome::Sampled { refresh, used_net: called.used_net, cpu: called.cpu }, now);
                        self.publish(&instance, with_state(&Value::from(&v), false, ""));
                    }
                    _ => self.faulted(&instance, Fault::BadOutput("its sample has no `value` object".into()), called.cpu),
                }
            }
            Job::Act { instance, verb, arg } => {
                if !self.runtime.as_ref().is_none_or(|r| r.has_act()) {
                    self.schedule.done(&instance, Outcome::Acted { resample: Resample::Nothing, cpu: Duration::ZERO }, Instant::now());
                    return;
                }
                let input = self.input(&instance, serde_json::json!({ "verb": verb, "arg": arg }));
                match self.call(&instance, Entry::Act, &input) {
                    Ok(called) => {
                        let out: ActOut = serde_json::from_slice(&called.output).unwrap_or_default();
                        let resample = match out.resample.as_deref() {
                            Some("all") => Resample::All,
                            Some("none") => Resample::Nothing,
                            _ => Resample::This,
                        };
                        self.schedule.done(&instance, Outcome::Acted { resample, cpu: called.cpu }, Instant::now());
                    }
                    Err(Fault::BadOutput(e)) if e.contains("no actions") => {
                        self.schedule.done(&instance, Outcome::Acted { resample: Resample::Nothing, cpu: Duration::ZERO }, Instant::now());
                    }
                    Err(f) => self.faulted(&instance, f, Duration::ZERO),
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::runtime::tests::{module, returning};
    use super::*;
    use crate::workspace::InstanceCfg;

    pub(crate) fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wf-code-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A source over `wat`, and a channel that hears its notifications.
    pub(crate) fn start(name: &str, wat: &str, limits: Limits, store: Option<Arc<KvStore>>) -> (WasmSource, Receiver<()>) {
        start_with(name, wat, limits, store, |_| {})
    }

    fn start_with(name: &str, wat: &str, limits: Limits, store: Option<Arc<KvStore>>, edit: impl FnOnce(&mut CodeSpec)) -> (WasmSource, Receiver<()>) {
        let dir = tmp(name);
        let path = dir.join("m.wasm");
        std::fs::write(&path, wat).unwrap();
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let deps = Deps { fetch: None, store, notify: Arc::new(move || { let _ = tx.lock().unwrap().send(()); }), limits, places: Default::default() };
        let initial = Value::obj([("temp", 0.into())]);
        let mut spec = CodeSpec { plugin: "p".into(), source: "weather".into(), module: path, hosts: vec![], fs_read: vec![], fs_read_params: vec![], launch: vec![], initial };
        edit(&mut spec);
        (WasmSource::start(spec, deps), rx)
    }

    pub(crate) fn cfg(id: &str) -> InstanceCfg {
        InstanceCfg { id: id.into(), widget: "w".into(), ..Default::default() }
    }

    pub(crate) fn read(src: &WasmSource, c: &InstanceCfg, params: &BTreeMap<String, Value>) -> Value {
        src.value(&SourceCx { cfg: c, params, tm: crate::data::Tm { year: 2026, month: 9, day: 24, dow: 4, hour: 12, minute: 0, second: 0, ms: 0 }, icon_pack: "Default" })
    }

    /// Waits for news about `instance`.
    pub(crate) fn wait_for(src: &WasmSource, rx: &Receiver<()>, instance: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if src.take_news().changed.contains(instance) {
                return;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "no news about {instance}: {:?}", src.status());
            let _ = rx.recv_timeout(left);
        }
    }

    fn num(v: &Value, k: &str) -> f64 {
        v.get(k).and_then(|x| x.as_f64()).unwrap_or(f64::NAN)
    }

    #[test]
    fn value_is_the_placeholder_then_the_sample() {
        let (src, rx) = start("value", &returning(r#"{"value":{"temp":21},"refresh_s":60}"#), Limits::default(), None);
        let (c, p) = (cfg("w-1"), BTreeMap::new());
        let first = read(&src, &c, &p);
        assert_eq!((num(&first, "temp"), first.get("loading").map(Value::truthy)), (0.0, Some(true)), "the manifest's initial values while loading");
        wait_for(&src, &rx, "w-1");
        let v = read(&src, &c, &p);
        assert_eq!((num(&v, "temp"), v.get("loading").map(Value::truthy)), (21.0, Some(false)));
        assert_eq!(src.status(), Status::Ready { fuel: src.status_fuel() });
    }

    #[test]
    fn code_reads_only_the_folder_its_instance_was_given() {
        let dir = tmp("picked-files");
        std::fs::write(dir.join("photo.txt"), "x").unwrap();
        // the answer to a `list` of `dir`, wrapped as the sample's value
        let req = serde_json::json!({ "op": "list", "path": dir.to_string_lossy() }).to_string();
        let imports = r#"(import "wf" "fs" (func $fs (param i32 i32) (result i32)))
                         (import "wf" "take" (func $take (param i32)))"#;
        let body = format!(
            r#"(local $n i32)
            (local.set $n (call $fs (i32.const 25) (i32.const {len})))
            (memory.copy (i32.const 512) (i32.const 16) (i32.const 9))
            (call $take (i32.const 521))
            (i32.store8 (i32.add (i32.const 521) (local.get $n)) (i32.const 125))
            i64.const 512 i64.const 32 i64.shl (i64.extend_i32_u (i32.add (local.get $n) (i32.const 10))) i64.or"#,
            len = req.len()
        );
        let wat = module(imports, &format!("{{\"value\":{req}"), &body, ABI);
        let (src, rx) = start_with("picked", &wat, Limits::default(), None, |s| s.fs_read_params = vec!["folder".into()]);
        let folder = BTreeMap::from([("folder".to_string(), Value::Str(dir.to_string_lossy().into_owned()))]);
        let listed = |id: &str, own: bool| {
            let mut c = cfg(id);
            if own {
                c.params.insert("folder".into(), serde_json::Value::String(dir.to_string_lossy().into_owned()));
            }
            read(&src, &c, &folder);
            wait_for(&src, &rx, id);
            let v = read(&src, &c, &folder);
            match v.get("entries") {
                Some(Value::List(l)) => Ok(l.iter().filter_map(|e| e.get("name")).map(|n| n.to_string()).collect::<Vec<_>>()),
                _ => Err(format!("{v:?}")),
            }
        };
        assert_eq!(listed("w-1", true), Ok(vec!["photo.txt".to_string()]));
        assert!(listed("w-2", false).is_err(), "a widget's default is not the user's pick");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_looping_module_never_blocks_value() {
        let limits = Limits { fuel_call: 20_000_000, ..Limits::default() };
        let (src, _rx) = start("loop", &module("", "", "(loop $l (br $l)) (call $out)", ABI), limits, None);
        let (c, p) = (cfg("w-1"), BTreeMap::new());
        read(&src, &c, &p);
        let t = Instant::now();
        for _ in 0..100 {
            read(&src, &c, &p);
        }
        assert!(t.elapsed() < Duration::from_millis(50), "{:?} for 100 reads", t.elapsed());
    }

    #[test]
    fn an_act_resamples_its_instance() {
        // the first sample says 1, every later one 2
        let body = r#"(if (i32.eqz (global.get $seen)) (then (global.set $seen (i32.const 1)) (return (call $out))))
                      i64.const 64 i64.const 32 i64.shl i64.const 17 i64.or"#;
        let wat = module(r#"(global $seen (mut i32) (i32.const 0)) (data (i32.const 64) "{\"value\":{\"n\":2}}")"#, r#"{"value":{"n":1}}"#, body, ABI);
        let (src, rx) = start("act", &wat, Limits::default(), None);
        let (c, p) = (cfg("w-1"), BTreeMap::new());
        read(&src, &c, &p);
        wait_for(&src, &rx, "w-1");
        assert_eq!(num(&read(&src, &c, &p), "n"), 1.0);
        src.act("refresh", "", &SourceCx { cfg: &c, params: &p, tm: crate::data::Tm { year: 2026, month: 9, day: 24, dow: 4, hour: 12, minute: 0, second: 0, ms: 0 }, icon_pack: "Default" });
        wait_for(&src, &rx, "w-1");
        assert_eq!(num(&read(&src, &c, &p), "n"), 2.0, "the act resampled it");
    }

    #[test]
    fn a_trap_backs_off_then_recovers_fresh() {
        // traps until the store says it has tried once: a fresh instance then succeeds
        let imports = r#"(import "wf" "store_set" (func $set (param i32 i32 i32 i32) (result i32)))
                         (import "wf" "store_get" (func $get (param i32 i32) (result i32)))
                         (data (i32.const 200) "t1")"#;
        let body = r#"(if (i32.eq (call $get (i32.const 200) (i32.const 1)) (i32.const -1))
                        (then (drop (call $set (i32.const 200) (i32.const 1) (i32.const 201) (i32.const 1))) unreachable))
                      (call $out)"#;
        let dir = tmp("trap-store");
        let store = Arc::new(KvStore::open(&dir.join("p.json")));
        let limits = Limits { backoff: Duration::from_millis(50), ..Limits::default() };
        let (src, rx) = start("trap", &module(imports, r#"{"value":{"temp":5},"refresh_s":1}"#, body, ABI), limits, Some(store));
        let (c, p) = (cfg("w-1"), BTreeMap::new());
        read(&src, &c, &p);
        wait_for(&src, &rx, "w-1");
        let failed = read(&src, &c, &p);
        assert!(failed.get("error").is_some_and(|e| e.to_string().contains("crashed")), "{failed:?}");
        assert!(matches!(src.status(), Status::Failing { .. }));
        wait_for(&src, &rx, "w-1");
        let ok = read(&src, &c, &p);
        assert_eq!((num(&ok, "temp"), ok.get("error").map(|e| e.to_string())), (5.0, Some(String::new())));
        assert!(matches!(src.status(), Status::Ready { .. }));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_broken_module_says_why_and_stops() {
        let (src, rx) = start("broken", "(module)", Limits::default(), None);
        let _ = rx.recv_timeout(Duration::from_secs(10));
        assert!(matches!(src.status(), Status::Broken(ref e) if e.contains("memory")), "{:?}", src.status());
        let v = read(&src, &cfg("w-1"), &BTreeMap::new());
        assert!(!v.get("loading").is_some_and(Value::truthy) && v.get("error").is_some_and(|e| e.to_string().contains("cannot run")), "the widget says why, never loads forever: {v:?}");
        assert!(src.stop().join().is_ok());
    }

    #[test]
    fn dropping_the_source_ends_its_thread() {
        let (src, _rx) = start("drop", &returning(r#"{"value":{}}"#), Limits::default(), None);
        let worker = src.stop();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !worker.is_finished() {
            assert!(Instant::now() < deadline, "the worker outlived its source");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The SDK's weather example, built: `WF_EXAMPLE_WASM=<path to weather.wasm>`
    /// (`cargo build --release --target wasm32-unknown-unknown` in `sdk/examples/weather`).
    /// Skipped without it.
    #[test]
    fn the_weather_example_runs_end_to_end() {
        let Some(wasm) = std::env::var_os("WF_EXAMPLE_WASM").map(PathBuf::from) else { return };
        use crate::net::{FakeFetch, Response};
        use crate::widgets::{Inputs, TomlWidget, Widget};
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = calls.clone();
        let fake = FakeFetch(Box::new(move |to, _| {
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            assert_eq!(to.host, "api.open-meteo.com");
            assert!(to.path.starts_with("/v1/forecast?latitude=59.91&longitude=10.75"), "{}", to.path);
            Ok(Response { status: 200, content_type: "application/json".into(), location: None, body: r#"{"current":{"temperature_2m":12.6,"weather_code":3,"wind_speed_10m":9.4}}"#.into() })
        }));
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let deps = Deps { fetch: Some(Arc::new(fake)), store: None, notify: Arc::new(move || { let _ = tx.lock().unwrap().send(()); }), limits: Limits::default(), places: Default::default() };
        let plugin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk/examples/weather/plugin");
        let manifest = crate::plugins::Manifest::parse(&std::fs::read_to_string(plugin.join("plugin.toml")).unwrap()).unwrap();
        let code = manifest.code.expect("the example has code");
        let src = WasmSource::start(CodeSpec { plugin: "weather".into(), source: code.source, module: wasm, hosts: code.net, fs_read: code.fs_read, fs_read_params: code.fs_read_params, launch: code.launch, initial: code.initial }, deps);
        let c = cfg("weather-1");
        let params = BTreeMap::from([("latitude".to_string(), Value::Num(59.91)), ("longitude".to_string(), Value::Num(10.75))]);
        assert!(read(&src, &c, &params).get("loading").is_some_and(Value::truthy));
        wait_for(&src, &rx, "weather-1");
        let v = read(&src, &c, &params);
        assert_eq!((num(&v, "temp"), v.get("sky").map(|s| s.to_string())), (13.0, Some("Cloudy".into())), "{v:?}; {:?}", src.status());
        let cx = SourceCx { cfg: &c, params: &params, tm: crate::data::Tm { year: 2026, month: 9, day: 24, dow: 4, hour: 12, minute: 0, second: 0, ms: 0 }, icon_pack: "Default" };
        src.act("refresh", "", &cx);
        wait_for(&src, &rx, "weather-1");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2, "the click fetched again");

        // and the example's widget builds against those values
        let def = crate::format::WidgetDef::parse("weather", &std::fs::read_to_string(plugin.join("widgets/weather.toml")).unwrap()).unwrap();
        let w = TomlWidget::new(def);
        let full = w.meta().effective_params(&params);
        let st = BTreeMap::new();
        let source = |n: &str| (n == "weather").then(|| v.clone());
        let theme = crate::theme::Theme::compose(&crate::theme::Library::load(std::path::Path::new("nope")), &crate::theme::Selection::default(), &[]);
        let b = w.build(&Inputs { params: &full, state: &st, card_size: (220.0, 120.0), key_prefix: "weather-1", read_source: &source }, &theme, &|_| None).unwrap();
        assert!(b.warnings.is_empty(), "{:?}", b.warnings);
        assert!(b.deps.contains("weather.temp"));
        fn texts(n: &crate::ui::Node, out: &mut Vec<String>) {
            if let crate::ui::Kind::Text(t) = &n.kind {
                out.push(t.text.clone());
            }
            n.children.iter().for_each(|c| texts(c, out));
        }
        let mut shown = Vec::new();
        texts(&b.root, &mut shown);
        assert!(shown.iter().any(|t| t == "13°C") && shown.iter().any(|t| t == "Cloudy"), "{shown:?}");
    }

    impl WasmSource {
        fn status_fuel(&self) -> u64 {
            match self.status() {
                Status::Ready { fuel } => fuel,
                _ => 0,
            }
        }
    }
}
