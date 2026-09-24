//! Each Data Source declares how often its fields change, so a window wakes
//! only when a bound value can differ (decision 16, ADR-0004).

mod clock;
mod shortcuts;
mod sys;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::code::{CodeSpec, Status, WasmSource};
use crate::value::Value;
use crate::workspace::InstanceCfg;

pub use clock::{Clock, Tm, clock_value, now_local};
pub use shortcuts::{ID_SEP, Shortcut, Shortcuts, file_stem, folder_items, icon_id, shortcuts_value, starter_apps};
pub use sys::Sys;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cadence {
    Frame,
    Second,
    TenSecond,
    Minute,
}

pub struct SourceCx<'a> {
    pub cfg: &'a InstanceCfg,
    /// The Instance's params with its Widget's defaults filled in.
    pub params: &'a std::collections::BTreeMap<String, Value>,
    pub tm: Tm,
    pub icon_pack: &'a str,
}

/// A named producer of values Widgets bind to. Built-in, registered by an app that links
/// Wayfinder as a library (`DataSources::register`), or a Plugin's Code Source.
pub trait DataSource: Send + Sync {
    fn name(&self) -> &str;
    fn value(&self, cx: &SourceCx) -> Value;
    /// `field` "" is the whole object. `None`: changes only on events, never with time.
    fn cadence(&self, field: &str) -> Option<Cadence>;
    /// Folders to watch for this Instance; a change calls `path_changed`.
    fn watched_paths(&self, _cx: &SourceCx) -> Vec<PathBuf> {
        vec![]
    }
    /// One of `watched_paths` changed.
    fn path_changed(&self, _path: &Path) {
        self.invalidate();
    }
    fn invalidate(&self) {}
    /// `on_click = "<name>.<verb> <arg>"`. True when handled.
    fn act(&self, _verb: &str, _arg: &str, _cx: &SourceCx) -> bool {
        false
    }
    /// Only these Instances exist now: free whatever was kept for the others.
    fn retain(&self, _live: &BTreeSet<String>) {}
    /// Called once, when registered. Keep the `Notifier` to say from any thread that values
    /// changed, instead of declaring a cadence and being polled.
    fn attach(&self, _notify: Notifier) {}
}

/// What changed in one source since the app last asked.
#[derive(Debug, Default, PartialEq)]
pub struct News {
    /// Everything it serves may have changed.
    pub all: bool,
    /// Only these Instances' values changed.
    pub changed: BTreeSet<String>,
    pub logs: Vec<String>,
    pub status_changed: bool,
}

#[derive(Default)]
struct Board {
    pending: Mutex<BTreeMap<String, News>>,
    wake: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl Board {
    fn post(&self, source: &str, f: impl FnOnce(&mut News)) {
        f(self.pending.lock().unwrap().entry(source.to_string()).or_default());
        if let Some(w) = self.wake.lock().unwrap().clone() {
            w();
        }
    }
}

/// A source's line to the app, usable from any thread: say what changed and the Widgets
/// reading it redraw, with no polling.
#[derive(Clone)]
pub struct Notifier {
    source: String,
    board: Arc<Board>,
}

impl Notifier {
    /// Everything this source serves may have changed.
    pub fn changed(&self) {
        self.board.post(&self.source, |n| n.all = true);
    }

    /// Only this Instance's value changed.
    pub fn changed_for(&self, instance: &str) {
        self.board.post(&self.source, |n| {
            n.changed.insert(instance.to_string());
        });
    }

    /// A line for Wayfinder's log.
    pub fn log(&self, line: impl Into<String>) {
        let line = line.into();
        self.board.post(&self.source, |n| n.logs.push(line));
    }
}

pub struct DataSources {
    list: Vec<Box<dyn DataSource>>,
    /// Plugins' Code Sources, each with the key it was started from.
    code: Vec<(String, WasmSource)>,
    board: Arc<Board>,
}

impl Default for DataSources {
    fn default() -> Self {
        Self::builtin()
    }
}

impl DataSources {
    pub fn builtin() -> Self {
        Self::new(vec![Box::new(Clock), Box::new(Sys::default()), Box::new(Shortcuts::default())])
    }

    pub fn new(list: Vec<Box<dyn DataSource>>) -> Self {
        let board = Arc::new(Board::default());
        for s in &list {
            s.attach(Notifier { source: s.name().to_string(), board: board.clone() });
        }
        Self { list, code: Vec::new(), board }
    }

    /// Adds a source, as an app built on Wayfinder does for its own (`Options::extra_sources`).
    /// Its name must be new: a built-in is never replaced.
    pub fn register(&mut self, source: Box<dyn DataSource>) -> Result<(), String> {
        let name = source.name().to_string();
        let ident = name.chars().next().is_some_and(|c| c.is_ascii_lowercase()) && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !ident {
            return Err(format!("data source `{name}`: use lower-case letters, digits and `_`, starting with a letter"));
        }
        if self.list.iter().any(|s| s.name() == name) {
            return Err(format!("there already is a data source `{name}`"));
        }
        source.attach(Notifier { source: name, board: self.board.clone() });
        self.list.push(source);
        Ok(())
    }

    /// Called whenever a source posts news, from whichever thread it runs on.
    pub fn set_waker(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        *self.board.wake.lock().unwrap() = Some(wake);
    }

    pub fn names(&self) -> Vec<String> {
        self.list.iter().map(|s| s.name().to_string()).chain(self.code.iter().map(|(_, c)| c.name().to_string())).collect()
    }

    pub fn get(&self, name: &str) -> Option<&dyn DataSource> {
        self.list.iter().find(|s| s.name() == name).map(|s| s.as_ref()).or_else(|| self.code(name).map(|c| c as &dyn DataSource))
    }

    fn code(&self, name: &str) -> Option<&WasmSource> {
        self.code.iter().find(|(_, c)| c.name() == name).map(|(_, c)| c)
    }

    /// Makes the running Code Sources exactly `wanted` (key, spec). One whose key is unchanged
    /// keeps running, with its values; the built-ins are never touched.
    pub fn sync_code(&mut self, wanted: Vec<(String, CodeSpec)>, mut start: impl FnMut(CodeSpec) -> WasmSource) {
        let mut old = std::mem::take(&mut self.code);
        for (key, spec) in wanted {
            match old.iter().position(|(k, _)| *k == key) {
                Some(i) => self.code.push(old.swap_remove(i)),
                None => self.code.push((key, start(spec))),
            }
        }
        // dropped: their channels close and their threads end after the current call
    }

    /// Sends `verb` to the source `source`; false if none handled it.
    pub fn act(&self, source: &str, verb: &str, arg: &str, cx: &SourceCx) -> bool {
        self.get(source).is_some_and(|s| s.act(verb, arg, cx))
    }

    /// Per source name, what changed since last asked.
    pub fn take_news(&self) -> Vec<(String, News)> {
        let mut out: Vec<(String, News)> = std::mem::take(&mut *self.board.pending.lock().unwrap()).into_iter().collect();
        out.extend(self.code.iter().map(|(_, c)| (c.name().to_string(), c.take_news())).filter(|(_, n)| *n != News::default()));
        out
    }

    pub fn retain(&self, live: &BTreeSet<String>) {
        self.list.iter().for_each(|s| s.retain(live));
        self.code.iter().for_each(|(_, c)| c.retain(live));
    }

    /// Per Code Source name, how it is doing.
    pub fn code_status(&self) -> Vec<(String, Status)> {
        self.code.iter().map(|(_, c)| (c.name().to_string(), c.status())).collect()
    }

    /// Whether any of `deps` reads a Code Source.
    pub fn uses_code(&self, deps: &BTreeSet<String>) -> bool {
        deps.iter().any(|d| self.code(d.split('.').next().unwrap_or(d)).is_some())
    }

    pub fn value(&self, name: &str, cx: &SourceCx) -> Option<Value> {
        self.get(name).map(|s| s.value(cx))
    }

    pub fn cadence_of(&self, path: &str) -> Option<Cadence> {
        let (root, field) = path.split_once('.').unwrap_or((path, ""));
        self.get(root)?.cadence(field)
    }

    pub fn next_wake(&self, deps: &BTreeSet<String>, tm: &Tm) -> Option<Duration> {
        let fastest = deps.iter().filter_map(|d| self.cadence_of(d)).min()?;
        let into_sec = tm.ms as u64;
        let ms = match fastest {
            Cadence::Frame => 8,
            Cadence::Second => 1000 - into_sec,
            Cadence::TenSecond => (10 - (tm.second % 10)) as u64 * 1000 - into_sec,
            Cadence::Minute => (60 - tm.second) as u64 * 1000 - into_sec,
        };
        Some(Duration::from_millis(ms + 2)) // just after the boundary, never just before
    }

    pub fn needs_every_frame(&self, deps: &BTreeSet<String>) -> bool {
        deps.iter().any(|d| self.cadence_of(d) == Some(Cadence::Frame))
    }

    pub fn watched_paths(&self, cx: &SourceCx) -> Vec<PathBuf> {
        self.list.iter().flat_map(|s| s.watched_paths(cx)).collect()
    }

    /// A watched folder changed: only the sources, not all content, need a fresh look.
    pub fn path_changed(&self, path: &Path) {
        self.list.iter().for_each(|s| s.path_changed(path));
    }

    /// Whether `deps` read the source `name`.
    pub fn reads(deps: &BTreeSet<String>, name: &str) -> bool {
        deps.iter().any(|d| d == name || d.strip_prefix(name).is_some_and(|rest| rest.starts_with('.')))
    }

    /// Only the built-ins: a Code Source's values are keyed by params already, and re-running
    /// it on every param edit or file save would re-fetch everything.
    pub fn invalidate(&self) {
        self.list.iter().for_each(|s| s.invalidate());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tm(h: u32, m: u32, s: u32, ms: u32) -> Tm {
        Tm { year: 2026, month: 9, day: 21, dow: 1, hour: h, minute: m, second: s, ms }
    }

    fn deps(p: &[&str]) -> BTreeSet<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn wakes_exactly_when_a_bound_value_can_change() {
        let src = DataSources::builtin();
        // no clock dependency: never wakes on time
        assert_eq!(src.next_wake(&deps(&["param.x", "shortcuts.items"]), &tm(1, 2, 3, 0)), None);
        // minute-level: next minute boundary (+2ms guard), not next second
        assert_eq!(src.next_wake(&deps(&["clock.minute"]), &tm(12, 0, 30, 500)), Some(Duration::from_millis(29_500 + 2)));
        // second-level beats minute-level when both are present
        assert_eq!(src.next_wake(&deps(&["clock.minute", "clock.second"]), &tm(12, 0, 30, 250)), Some(Duration::from_millis(750 + 2)));
        // ten-second minute-hand step
        assert_eq!(src.next_wake(&deps(&["clock.minute_angle"]), &tm(12, 0, 23, 0)), Some(Duration::from_millis(7_000 + 2)));
        // smooth second hand needs frames
        assert!(src.needs_every_frame(&deps(&["clock.second_smooth"])));
        assert!(!src.needs_every_frame(&deps(&["clock.second"])));
    }

    #[test]
    fn cadence_is_asked_of_the_source_named_by_the_path() {
        let src = DataSources::builtin();
        assert_eq!(src.cadence_of("sys.gauges"), Some(Cadence::Second));
        assert_eq!(src.cadence_of("clock"), Some(Cadence::Frame), "a whole source changes as often as its fastest field");
        assert_eq!(src.cadence_of("shortcuts.items"), None);
        assert_eq!(src.cadence_of("item.name"), None, "not a source: a repeat variable");
    }

    #[test]
    fn sync_keeps_an_unchanged_source_alive() {
        use crate::code::runtime::{Limits, tests::returning};
        use crate::code::tests::start;
        let mut src = DataSources::builtin();
        let spec = |n: &str| CodeSpec { plugin: n.into(), source: n.into(), module: "nope.wasm".into(), hosts: vec![], initial: Value::Nil };
        let started = std::cell::RefCell::new(Vec::new());
        let launch = |s: CodeSpec| {
            started.borrow_mut().push(s.source.clone());
            start(&format!("sync-{}", s.source), &returning(r#"{"value":{}}"#), Limits::default(), None).0
        };
        src.sync_code(vec![("a#1".into(), spec("a")), ("b#1".into(), spec("b"))], launch);
        src.sync_code(vec![("a#1".into(), spec("a")), ("b#2".into(), spec("b"))], launch);
        assert_eq!(*started.borrow(), ["a", "b", "b"], "a kept running; b's changed key restarted it");
        src.sync_code(vec![], launch);
        assert!(src.code.is_empty());
    }

    #[test]
    fn builtin_state_survives_sync() {
        let mut src = DataSources::builtin();
        let before = src.get("sys").unwrap() as *const dyn DataSource as *const u8;
        src.sync_code(vec![], |_| unreachable!());
        assert!(std::ptr::eq(before, src.get("sys").unwrap() as *const dyn DataSource as *const u8), "Sys keeps its history");
        assert!(!src.uses_code(&deps(&["sys.gauges", "clock.minute"])));
    }

    /// A native source like an app built on Wayfinder would add.
    #[derive(Default)]
    struct Media {
        acts: Mutex<Vec<String>>,
        live: Mutex<BTreeSet<String>>,
        notify: Mutex<Option<Notifier>>,
    }

    impl DataSource for Media {
        fn name(&self) -> &str {
            "media"
        }
        fn value(&self, _: &SourceCx) -> Value {
            Value::obj([("title", "Song".into())])
        }
        fn cadence(&self, _: &str) -> Option<Cadence> {
            None
        }
        fn act(&self, verb: &str, arg: &str, cx: &SourceCx) -> bool {
            self.acts.lock().unwrap().push(format!("{verb} {arg} {}", cx.cfg.id));
            verb == "play_pause"
        }
        fn retain(&self, live: &BTreeSet<String>) {
            *self.live.lock().unwrap() = live.clone();
        }
        fn attach(&self, notify: Notifier) {
            *self.notify.lock().unwrap() = Some(notify);
        }
    }

    fn with_cx<R>(f: impl FnOnce(&SourceCx) -> R) -> R {
        let cfg = InstanceCfg { id: "media-1".into(), ..Default::default() };
        let params = cfg.params_map();
        f(&SourceCx { cfg: &cfg, params: &params, tm: tm(1, 2, 3, 0), icon_pack: "Default" })
    }

    #[test]
    fn a_registered_source_is_read_acted_on_and_retained() {
        let mut src = DataSources::builtin();
        src.register(Box::new(Media::default())).unwrap();
        assert!(with_cx(|cx| src.value("media", cx)).is_some_and(|v| v.get("title").is_some()));
        assert!(with_cx(|cx| src.act("media", "play_pause", "", cx)), "on_click = \"media.play_pause\"");
        assert!(!with_cx(|cx| src.act("media", "eject", "", cx)), "a verb it does not know");
        assert!(!with_cx(|cx| src.act("nope", "x", "", cx)));
        src.retain(&BTreeSet::from(["media-1".to_string()]));
        assert!(src.names().contains(&"media".to_string()));
        assert!(src.register(Box::new(Media::default())).unwrap_err().contains("already"));
        struct Named(&'static str);
        impl DataSource for Named {
            fn name(&self) -> &str {
                self.0
            }
            fn value(&self, _: &SourceCx) -> Value {
                Value::Nil
            }
            fn cadence(&self, _: &str) -> Option<Cadence> {
                None
            }
        }
        assert!(src.register(Box::new(Named("clock"))).is_err(), "a built-in is never replaced");
        assert!(src.register(Box::new(Named("My Source"))).is_err());
    }

    #[test]
    fn a_native_source_says_it_changed_from_another_thread() {
        let mut src = DataSources::builtin();
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        src.set_waker(Arc::new(move || {
            let _ = tx.lock().unwrap().send(());
        }));
        let media = Arc::new(Media::default());
        struct Shared(Arc<Media>);
        impl DataSource for Shared {
            fn name(&self) -> &str {
                "media"
            }
            fn value(&self, cx: &SourceCx) -> Value {
                self.0.value(cx)
            }
            fn cadence(&self, _: &str) -> Option<Cadence> {
                None
            }
            fn attach(&self, n: Notifier) {
                self.0.attach(n)
            }
        }
        src.register(Box::new(Shared(media.clone()))).unwrap();
        let notify = media.notify.lock().unwrap().clone().expect("attached on register");
        std::thread::spawn(move || {
            notify.changed_for("media-1");
            notify.changed();
            notify.log("new track");
        })
        .join()
        .unwrap();
        rx.recv_timeout(Duration::from_secs(5)).expect("the app was woken");
        let news = src.take_news();
        let (name, n) = &news[0];
        assert_eq!((name.as_str(), n.all, n.changed.contains("media-1"), n.logs.clone()), ("media", true, true, vec!["new track".to_string()]));
        assert!(src.take_news().is_empty(), "taken once");
        assert!(DataSources::reads(&deps(&["media.title"]), "media") && !DataSources::reads(&deps(&["mediaplayer.x"]), "media"));
    }

    #[test]
    fn a_new_source_is_one_impl_and_one_registration() {
        struct Weather;
        impl DataSource for Weather {
            fn name(&self) -> &str {
                "weather"
            }
            fn value(&self, _: &SourceCx) -> Value {
                Value::obj([("temp", 21.into())])
            }
            fn cadence(&self, _: &str) -> Option<Cadence> {
                Some(Cadence::Minute)
            }
        }
        let src = DataSources::new(vec![Box::new(Weather)]);
        let cfg = InstanceCfg::default();
        let params = cfg.params_map();
        let cx = SourceCx { cfg: &cfg, params: &params, tm: tm(1, 2, 3, 0), icon_pack: "Default" };
        assert_eq!(src.value("weather", &cx).and_then(|v| v.get("temp").cloned()), Some(Value::Num(21.0)));
        assert_eq!(src.next_wake(&deps(&["weather.temp"]), &tm(12, 0, 0, 0)), Some(Duration::from_millis(60_002)));
    }
}
