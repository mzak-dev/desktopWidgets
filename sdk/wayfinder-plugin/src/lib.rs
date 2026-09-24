//! Write the code of a Wayfinder plugin in Rust.
//!
//! A plugin's code is a **Code Source**: a Data Source its TOML widgets bind to
//! (`{weather.temp}`) and send actions to (`on_click = "weather.refresh"`). Implement
//! [`Source`], export it with [`export_source!`], and build for `wasm32-unknown-unknown`:
//!
//! ```ignore
//! use wayfinder_plugin::*;
//!
//! #[derive(Default)]
//! struct Weather;
//!
//! impl Source for Weather {
//!     fn sample(&mut self, cx: &Cx) -> Sample {
//!         let url = format!("https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}&current=temperature_2m", cx.number("latitude"), cx.number("longitude"));
//!         match http::get(&url).and_then(|r| r.json()) {
//!             Ok(v) => Sample::new(json!({ "temp": v["current"]["temperature_2m"] })).every(minutes(15)),
//!             Err(e) => Sample::error(e).every(minutes(1)),
//!         }
//!     }
//! }
//!
//! export_source!(Weather);
//! ```
//!
//! The host runs the module in a sandbox: it may only reach the hosts its `plugin.toml`
//! lists, keep up to 1 MB in [`store`], and must answer within its fuel budget.

use std::cell::RefCell;
use std::time::Duration;

use serde::{Deserialize, Serialize};
pub use serde_json::{Value, json};

/// The plugin ABI this crate speaks; the host refuses modules built for another.
pub const ABI: i32 = 1;

pub fn seconds(n: u64) -> Duration {
    Duration::from_secs(n)
}

pub fn minutes(n: u64) -> Duration {
    Duration::from_secs(n * 60)
}

/// Local time on the user's machine when the call was made.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Local {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

/// What one call knows: which widget asked, its settings, and the time.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Cx {
    /// The widget Instance (`weather-1`). Keep per-widget state keyed by it.
    pub instance: String,
    /// The widget's params, defaults filled in.
    pub params: Value,
    /// Milliseconds since 1970 (UTC).
    pub now_ms: i64,
    pub local: Local,
    #[serde(skip)]
    verb: String,
    #[serde(skip)]
    arg: String,
}

impl Cx {
    pub fn param(&self, name: &str) -> Option<&Value> {
        self.params.get(name)
    }

    /// A text param, or "".
    pub fn text(&self, name: &str) -> &str {
        self.param(name).and_then(Value::as_str).unwrap_or("")
    }

    /// A number param, or 0.
    pub fn number(&self, name: &str) -> f64 {
        self.param(name).and_then(Value::as_f64).unwrap_or(0.0)
    }

    pub fn flag(&self, name: &str) -> bool {
        self.param(name).and_then(Value::as_bool).unwrap_or(false)
    }
}

/// What a sample hands the widget.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Sample {
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_s: Option<f64>,
}

impl Sample {
    /// `value` must be an object: its fields are what `{source.field}` reads.
    pub fn new(value: Value) -> Sample {
        Sample { value: Some(value), error: None, refresh_s: None }
    }

    /// Shown as `{source.error}`; the last value stays.
    pub fn error(message: impl Into<String>) -> Sample {
        Sample { value: None, error: Some(message.into()), refresh_s: None }
    }

    /// Sample again after `d` (at least 1 s, or 1 minute after using the network). Without
    /// it, the widget is sampled once, then again only after an action or a settings change.
    pub fn every(mut self, d: Duration) -> Sample {
        self.refresh_s = Some(d.as_secs_f64());
        self
    }

    pub fn value(&self) -> Option<&Value> {
        self.value.as_ref()
    }

    pub fn error_message(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn refresh(&self) -> Option<Duration> {
        self.refresh_s.map(Duration::from_secs_f64)
    }
}

/// What to sample again after an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// The widget that sent it (the default).
    Resample,
    /// Every widget using this source.
    ResampleAll,
    Nothing,
}

/// A plugin's code. One value of it lives for as long as the module runs; it starts fresh
/// (from `Default`) after a crash, so keep anything that must last in [`store`].
pub trait Source: Default {
    fn sample(&mut self, cx: &Cx) -> Sample;

    /// `on_click = "weather.refresh some-arg"` arrives as `act("refresh", "some-arg", cx)`.
    fn act(&mut self, _verb: &str, _arg: &str, _cx: &Cx) -> Act {
        Act::Resample
    }
}

/// Sends a line to Wayfinder's log (`wayfinder.log`, Settings > Log).
#[macro_export]
macro_rules! log {
    ($($t:tt)*) => { $crate::log_line(&format!($($t)*)) };
}

pub fn log_line(line: &str) {
    host::log(line.as_bytes());
}

pub mod http {
    use super::*;

    #[derive(Clone, Debug, Default, Serialize)]
    pub struct Request {
        pub method: String,
        pub url: String,
        pub headers: std::collections::BTreeMap<String, String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub body: Option<String>,
    }

    #[derive(Clone, Debug, Default, Deserialize, PartialEq)]
    #[serde(default)]
    pub struct Response {
        pub status: u16,
        pub content_type: String,
        /// Set on a redirect, which is never followed for you.
        pub location: Option<String>,
        pub body: String,
    }

    impl Response {
        /// The body as JSON, or an error naming the status for anything but 2xx.
        pub fn json(&self) -> Result<Value, String> {
            if !(200..300).contains(&self.status) {
                return Err(format!("the server answered {}", self.status));
            }
            serde_json::from_str(&self.body).map_err(|e| format!("not JSON: {e}"))
        }
    }

    #[derive(Deserialize)]
    struct Refused {
        error: String,
    }

    /// Sends it and waits (the host allows 15 s). Only `https://` hosts the plugin's
    /// `plugin.toml` lists under `[code] net` can be reached.
    pub fn request(req: &Request) -> Result<Response, String> {
        let out = host::http(&serde_json::to_vec(req).map_err(|e| e.to_string())?);
        if let Ok(r) = serde_json::from_slice::<Refused>(&out) {
            return Err(r.error);
        }
        serde_json::from_slice(&out).map_err(|e| format!("bad answer from the host: {e}"))
    }

    pub fn get(url: &str) -> Result<Response, String> {
        request(&Request { method: "GET".into(), url: url.into(), ..Default::default() })
    }
}

/// Up to 1 MB of text per plugin that survives restarts and upgrades.
pub mod store {
    use super::host;

    pub fn get(key: &str) -> Option<String> {
        host::store_get(key.as_bytes()).map(|v| String::from_utf8_lossy(&v).into_owned())
    }

    /// False when it would pass 1 MB (the old value stays).
    pub fn set(key: &str, value: &str) -> bool {
        !value.is_empty() && host::store_set(key.as_bytes(), value.as_bytes())
    }

    pub fn remove(key: &str) {
        host::store_set(key.as_bytes(), b"");
    }
}

#[cfg(target_arch = "wasm32")]
mod host {
    #[link(wasm_import_module = "wf")]
    unsafe extern "C" {
        #[link_name = "log"]
        fn wf_log(ptr: *const u8, len: i32);
        #[link_name = "http"]
        fn wf_http(ptr: *const u8, len: i32) -> i32;
        #[link_name = "store_get"]
        fn wf_store_get(ptr: *const u8, len: i32) -> i32;
        #[link_name = "store_set"]
        fn wf_store_set(kptr: *const u8, klen: i32, vptr: *const u8, vlen: i32) -> i32;
        #[link_name = "take"]
        fn wf_take(ptr: *mut u8);
    }

    /// Fetches what the last host call parked.
    fn take(n: i32) -> Vec<u8> {
        let mut buf = vec![0u8; n.max(0) as usize];
        unsafe { wf_take(buf.as_mut_ptr()) };
        buf
    }

    pub fn log(line: &[u8]) {
        unsafe { wf_log(line.as_ptr(), line.len() as i32) }
    }

    pub fn http(req: &[u8]) -> Vec<u8> {
        let n = unsafe { wf_http(req.as_ptr(), req.len() as i32) };
        take(n)
    }

    pub fn store_get(key: &[u8]) -> Option<Vec<u8>> {
        let n = unsafe { wf_store_get(key.as_ptr(), key.len() as i32) };
        (n >= 0).then(|| take(n))
    }

    pub fn store_set(key: &[u8], value: &[u8]) -> bool {
        unsafe { wf_store_set(key.as_ptr(), key.len() as i32, value.as_ptr(), value.len() as i32) == 0 }
    }
}

/// On your own machine (`cargo test`), host calls go to [`testing`] instead.
#[cfg(not(target_arch = "wasm32"))]
mod host {
    use super::testing::FAKE;

    pub fn log(line: &[u8]) {
        FAKE.with(|f| f.borrow_mut().logs.push(String::from_utf8_lossy(line).into_owned()));
    }

    pub fn http(req: &[u8]) -> Vec<u8> {
        FAKE.with(|f| match &f.borrow().http {
            Some(answer) => answer(&serde_json::from_slice(req).unwrap_or_default()),
            None => br#"{"error":"no network in tests: call testing::answer_http"}"#.to_vec(),
        })
    }

    pub fn store_get(key: &[u8]) -> Option<Vec<u8>> {
        FAKE.with(|f| f.borrow().store.get(key).cloned())
    }

    pub fn store_set(key: &[u8], value: &[u8]) -> bool {
        FAKE.with(|f| {
            let mut f = f.borrow_mut();
            if value.is_empty() {
                f.store.remove(key);
            } else {
                f.store.insert(key.to_vec(), value.to_vec());
            }
        });
        true
    }
}

/// A stand-in host for unit tests of a [`Source`] on your own machine.
#[cfg(not(target_arch = "wasm32"))]
pub mod testing {
    use super::*;
    use std::collections::BTreeMap;

    type Answer = Box<dyn Fn(&Value) -> Vec<u8>>;

    #[derive(Default)]
    pub(crate) struct Fake {
        pub(crate) logs: Vec<String>,
        pub(crate) http: Option<Answer>,
        pub(crate) store: BTreeMap<Vec<u8>, Vec<u8>>,
    }

    thread_local! {
        pub(crate) static FAKE: RefCell<Fake> = RefCell::new(Fake::default());
    }

    /// Answers every request (its JSON: method, url, headers, body) with `f`.
    pub fn answer_http(f: impl Fn(&Value) -> http::Response + 'static) {
        FAKE.with(|x| {
            x.borrow_mut().http = Some(Box::new(move |req| {
                let r = f(req);
                json!({ "status": r.status, "content_type": r.content_type, "location": r.location, "body": r.body }).to_string().into_bytes()
            }))
        });
    }

    pub fn logs() -> Vec<String> {
        FAKE.with(|f| f.borrow().logs.clone())
    }

    /// A call from widget `instance` with these params.
    pub fn cx(instance: &str, params: Value) -> Cx {
        Cx { instance: instance.into(), params, ..Default::default() }
    }
}

// What `export_source!` expands to. Not for direct use.

thread_local! {
    static OUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

#[doc(hidden)]
pub fn __alloc(len: i32) -> i32 {
    let mut b = vec![0u8; len.max(0) as usize].into_boxed_slice();
    let p = b.as_mut_ptr();
    std::mem::forget(b);
    p as usize as i32
}

/// Takes back the input `__alloc` lent the host.
fn input(ptr: i32, len: i32) -> Vec<u8> {
    unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr as usize as *mut u8, len.max(0) as usize)).into_vec() }
}

fn output(bytes: Vec<u8>) -> i64 {
    OUT.with(|o| {
        *o.borrow_mut() = bytes;
        let o = o.borrow();
        ((o.as_ptr() as usize as i64) << 32) | o.len() as i64
    })
}

fn panics_to_log() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| std::panic::set_hook(Box::new(|p| log_line(&format!("panicked: {p}")))));
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ActIn {
    verb: String,
    arg: String,
}

/// Runs one sample from the host's JSON; public for `testing`-style harnesses too.
#[doc(hidden)]
pub fn __run_sample<S: Source>(src: &mut S, input_json: &[u8]) -> Vec<u8> {
    let out = match serde_json::from_slice::<Cx>(input_json) {
        Ok(cx) => src.sample(&cx),
        Err(e) => Sample::error(format!("bad input from the host: {e}")),
    };
    serde_json::to_vec(&out).unwrap_or_default()
}

#[doc(hidden)]
pub fn __run_act<S: Source>(src: &mut S, input_json: &[u8]) -> Vec<u8> {
    let (mut cx, a): (Cx, ActIn) = (serde_json::from_slice(input_json).unwrap_or_default(), serde_json::from_slice(input_json).unwrap_or_default());
    (cx.verb, cx.arg) = (a.verb, a.arg);
    let resample = match src.act(&cx.verb, &cx.arg, &cx) {
        Act::Resample => "this",
        Act::ResampleAll => "all",
        Act::Nothing => "none",
    };
    json!({ "resample": resample }).to_string().into_bytes()
}

#[doc(hidden)]
pub fn __sample<S: Source>(src: &mut S, ptr: i32, len: i32) -> i64 {
    panics_to_log();
    let i = input(ptr, len);
    output(__run_sample(src, &i))
}

#[doc(hidden)]
pub fn __act<S: Source>(src: &mut S, ptr: i32, len: i32) -> i64 {
    panics_to_log();
    let i = input(ptr, len);
    output(__run_act(src, &i))
}

/// Exports a [`Source`] as the module's entry points.
#[macro_export]
macro_rules! export_source {
    ($t:ty) => {
        #[cfg(target_arch = "wasm32")]
        mod __wayfinder_exports {
            use super::*;
            ::std::thread_local! {
                static SOURCE: ::core::cell::RefCell<$t> = ::core::cell::RefCell::new(<$t as ::core::default::Default>::default());
            }
            #[unsafe(no_mangle)]
            pub extern "C" fn wf_abi() -> i32 {
                $crate::ABI
            }
            #[unsafe(no_mangle)]
            pub extern "C" fn wf_alloc(len: i32) -> i32 {
                $crate::__alloc(len)
            }
            #[unsafe(no_mangle)]
            pub extern "C" fn wf_sample(ptr: i32, len: i32) -> i64 {
                SOURCE.with(|s| $crate::__sample(&mut *s.borrow_mut(), ptr, len))
            }
            #[unsafe(no_mangle)]
            pub extern "C" fn wf_act(ptr: i32, len: i32) -> i64 {
                SOURCE.with(|s| $crate::__act(&mut *s.borrow_mut(), ptr, len))
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Counter {
        n: u32,
    }

    impl Source for Counter {
        fn sample(&mut self, cx: &Cx) -> Sample {
            self.n += 1;
            store::set("last", cx.text("city"));
            log!("sampled {}", self.n);
            Sample::new(json!({ "n": self.n, "city": cx.text("city") })).every(seconds(30))
        }

        fn act(&mut self, verb: &str, _arg: &str, _cx: &Cx) -> Act {
            if verb == "reset" {
                self.n = 0;
                Act::ResampleAll
            } else {
                Act::Nothing
            }
        }
    }

    #[test]
    fn a_sample_speaks_the_hosts_json() {
        let mut c = Counter::default();
        let out: Value = serde_json::from_slice(&__run_sample(&mut c, br#"{"abi":1,"instance":"w-1","params":{"city":"Oslo"},"now_ms":0,"local":{"hour":9}}"#)).unwrap();
        assert_eq!(out, json!({ "value": { "n": 1, "city": "Oslo" }, "refresh_s": 30.0 }));
        assert_eq!(store::get("last").as_deref(), Some("Oslo"));
        assert_eq!(testing::logs(), ["sampled 1"]);
        let bad: Value = serde_json::from_slice(&__run_sample(&mut c, b"nope")).unwrap();
        assert!(bad["error"].as_str().unwrap().contains("bad input"));
    }

    #[test]
    fn an_act_says_what_to_resample() {
        let mut c = Counter { n: 5 };
        let out: Value = serde_json::from_slice(&__run_act(&mut c, br#"{"instance":"w-1","params":{},"verb":"reset","arg":""}"#)).unwrap();
        assert_eq!((out["resample"].as_str(), c.n), (Some("all"), 0));
        let out: Value = serde_json::from_slice(&__run_act(&mut c, br#"{"verb":"other"}"#)).unwrap();
        assert_eq!(out["resample"], "none");
    }

    #[test]
    fn http_goes_through_the_fake_host() {
        assert!(http::get("https://x.com/").unwrap_err().contains("answer_http"));
        testing::answer_http(|req| http::Response { status: 200, body: format!(r#"{{"url":"{}"}}"#, req["url"].as_str().unwrap()), ..Default::default() });
        let r = http::get("https://api.open-meteo.com/v1").unwrap();
        assert_eq!(r.json().unwrap()["url"], "https://api.open-meteo.com/v1");
        testing::answer_http(|_| http::Response { status: 503, ..Default::default() });
        assert_eq!(http::get("https://x.com/").unwrap().json().unwrap_err(), "the server answered 503");
    }

    #[test]
    fn store_set_empty_and_remove() {
        assert!(store::set("k", "v"));
        store::remove("k");
        assert_eq!(store::get("k"), None);
        assert!(!store::set("k", ""), "an empty value is a removal, not a value");
    }
}
