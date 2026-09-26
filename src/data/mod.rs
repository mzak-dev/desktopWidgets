//! Each Data Source declares how often its fields change, so a window wakes
//! only when a bound value can differ (decision 16, ADR-0004).

mod audio;
mod clock;
pub(crate) mod media;
mod shortcuts;
mod sys;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::code::{CodeSpec, Status, WasmSource};
use crate::value::Value;
use crate::workspace::InstanceCfg;

pub use audio::Audio;
pub use clock::{Clock, Tm, clock_value, now_local};
pub use media::Media;
pub use shortcuts::{ID_SEP, Shortcut, Shortcuts, file_stem, folder_items, icon_id, shortcuts_value, starter_apps};
pub use sys::Sys;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cadence {
    /// Every display frame: smooth motion only.
    Frame,
    /// Every this many milliseconds (at least 16), e.g. an animation's own frame rate.
    Millis(u32),
    Second,
    TenSecond,
    Minute,
}

impl Cadence {
    pub fn period(self) -> Duration {
        match self {
            Cadence::Frame => Duration::from_millis(8),
            Cadence::Millis(n) => Duration::from_millis(n.max(16) as u64),
            Cadence::Second => Duration::from_secs(1),
            Cadence::TenSecond => Duration::from_secs(10),
            Cadence::Minute => Duration::from_secs(60),
        }
    }
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
    /// How often `field` ("" is the whole object) can change for this Instance. `None`: only
    /// on events, never with time. It is asked again after every redraw, so it may follow
    /// the source's state: a media source says `Second` for the position while playing and
    /// `None` while paused, and calls its `Notifier` when playback resumes, so the Widget
    /// redraws and asks again.
    fn cadence(&self, field: &str, cx: &SourceCx) -> Option<Cadence>;
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
    /// Params to save: (Instance id, param name, value).
    pub params: Vec<(String, String, Value)>,
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

    /// Saves `value` as the Instance's param `name`, as if the user had set it in Settings:
    /// it survives restarts and Settings shows it. A folder dropped on a gallery stays its
    /// folder. Only native sources get a Notifier; plugin code never changes params (ADR-0008).
    pub fn set_param(&self, instance: &str, name: &str, value: Value) {
        let (instance, name) = (instance.to_string(), name.to_string());
        self.board.post(&self.source, |n| n.params.push((instance, name, value)));
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
    /// The built-in sources, with their caches (album art) in the temp folder. The app uses
    /// `builtin_in` with its data folder.
    pub fn builtin() -> Self {
        Self::builtin_in(&std::env::temp_dir().join("wayfinder"))
    }

    /// The built-in sources, caching under `<data>/.cache` (a dot-folder never reloads content).
    pub fn builtin_in(data: &Path) -> Self {
        let cache = data.join(".cache");
        Self::new(vec![Box::new(Clock), Box::new(Sys::default()), Box::new(Shortcuts::default()), Box::new(Media::new(cache.join("media"))), Box::new(Audio::default())])
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

    /// Every source but plugin code: the built-ins and any registered by the app.
    pub fn native_names(&self) -> BTreeSet<String> {
        self.list.iter().map(|s| s.name().to_string()).collect()
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
    /// Every param that grants some plugin's code a folder (`fs_read_params`).
    pub fn file_params(&self) -> BTreeSet<String> {
        self.code.iter().flat_map(|(_, c)| c.file_params().iter().cloned()).collect()
    }

    /// The launch rules of each Code Source `deps` reads (see `code::launch::allowed`).
    pub fn launch_rules(&self, deps: &BTreeSet<String>) -> Vec<&[crate::code::launch::LaunchRule]> {
        let names: BTreeSet<&str> = deps.iter().map(|d| d.split('.').next().unwrap_or(d)).collect();
        names.into_iter().filter_map(|n| self.code(n)).map(|s| s.launch_rules()).collect()
    }

    pub fn value(&self, name: &str, cx: &SourceCx) -> Option<Value> {
        self.get(name).map(|s| s.value(cx))
    }

    pub fn cadence_of(&self, path: &str, cx: &SourceCx) -> Option<Cadence> {
        let (root, field) = path.split_once('.').unwrap_or((path, ""));
        self.get(root)?.cadence(field, cx)
    }

    /// How soon an Instance reading `deps` must redraw, asked after each of its redraws.
    pub fn next_wake(&self, deps: &BTreeSet<String>, cx: &SourceCx) -> Option<Duration> {
        let fastest = deps.iter().filter_map(|d| self.cadence_of(d, cx)).min_by_key(|c| c.period())?;
        let tm = &cx.tm;
        let into_sec = tm.ms as u64;
        let ms = match fastest {
            Cadence::Frame => 8,
            Cadence::Millis(_) => return Some(fastest.period()),
            Cadence::Second => 1000 - into_sec,
            Cadence::TenSecond => (10 - (tm.second % 10)) as u64 * 1000 - into_sec,
            Cadence::Minute => (60 - tm.second) as u64 * 1000 - into_sec,
        };
        Some(Duration::from_millis(ms + 2)) // just after the boundary, never just before
    }

    pub fn needs_every_frame(&self, deps: &BTreeSet<String>, cx: &SourceCx) -> bool {
        deps.iter().any(|d| self.cadence_of(d, cx) == Some(Cadence::Frame))
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

    fn wake(src: &DataSources, d: &BTreeSet<String>, t: Tm) -> Option<Duration> {
        let (cfg, params) = (InstanceCfg::default(), BTreeMap::new());
        src.next_wake(d, &SourceCx { cfg: &cfg, params: &params, tm: t, icon_pack: "Default" })
    }

    fn every_frame(src: &DataSources, d: &BTreeSet<String>) -> bool {
        with_cx(|cx| src.needs_every_frame(d, cx))
    }

    #[test]
    fn wakes_exactly_when_a_bound_value_can_change() {
        let src = DataSources::builtin();
        // no clock dependency: never wakes on time
        assert_eq!(wake(&src, &deps(&["param.x", "shortcuts.items"]), tm(1, 2, 3, 0)), None);
        // minute-level: next minute boundary (+2ms guard), not next second
        assert_eq!(wake(&src, &deps(&["clock.minute"]), tm(12, 0, 30, 500)), Some(Duration::from_millis(29_500 + 2)));
        // second-level beats minute-level when both are present
        assert_eq!(wake(&src, &deps(&["clock.minute", "clock.second"]), tm(12, 0, 30, 250)), Some(Duration::from_millis(750 + 2)));
        // ten-second minute-hand step
        assert_eq!(wake(&src, &deps(&["clock.minute_angle"]), tm(12, 0, 23, 0)), Some(Duration::from_millis(7_000 + 2)));
        // smooth second hand needs frames
        assert!(every_frame(&src, &deps(&["clock.second_smooth"])));
        assert!(!every_frame(&src, &deps(&["clock.second"])));
    }

    #[test]
    fn a_millis_cadence_wakes_at_its_own_rate() {
        struct Gif;
        impl DataSource for Gif {
            fn name(&self) -> &str {
                "gif"
            }
            fn value(&self, _: &SourceCx) -> Value {
                Value::Nil
            }
            fn cadence(&self, _: &str, _: &SourceCx) -> Option<Cadence> {
                Some(Cadence::Millis(83))
            }
        }
        let mut src = DataSources::builtin();
        src.register(Box::new(Gif)).unwrap();
        assert_eq!(wake(&src, &deps(&["gif.frame", "clock.minute"]), tm(1, 2, 3, 0)), Some(Duration::from_millis(83)), "12 fps, not every display frame");
        assert!(!every_frame(&src, &deps(&["gif.frame"])));
        assert_eq!(wake(&src, &deps(&["gif.frame", "clock.second_smooth"]), tm(1, 2, 3, 0)), Some(Duration::from_millis(10)), "a frame cadence still wins");
        assert_eq!(Cadence::Millis(1).period(), Duration::from_millis(16));
    }

    #[test]
    fn cadence_follows_the_sources_state_and_the_instance() {
        use std::sync::atomic::{AtomicBool, Ordering};
        #[derive(Default)]
        struct Player {
            playing: AtomicBool,
        }
        impl DataSource for Player {
            fn name(&self) -> &str {
                "player"
            }
            fn value(&self, _: &SourceCx) -> Value {
                Value::Nil
            }
            fn cadence(&self, field: &str, cx: &SourceCx) -> Option<Cadence> {
                let ticking = field == "position" && self.playing.load(Ordering::Relaxed) && cx.params.get("progress").is_none_or(Value::truthy);
                ticking.then_some(Cadence::Second)
            }
        }
        let player = Arc::new(Player::default());
        struct Shared(Arc<Player>);
        impl DataSource for Shared {
            fn name(&self) -> &str {
                self.0.name()
            }
            fn value(&self, cx: &SourceCx) -> Value {
                self.0.value(cx)
            }
            fn cadence(&self, f: &str, cx: &SourceCx) -> Option<Cadence> {
                self.0.cadence(f, cx)
            }
        }
        let mut src = DataSources::builtin();
        src.register(Box::new(Shared(player.clone()))).unwrap();
        let bar = deps(&["player.position"]);
        assert_eq!(wake(&src, &bar, tm(1, 2, 3, 0)), None, "paused: the bar sleeps");
        player.playing.store(true, Ordering::Relaxed);
        assert_eq!(wake(&src, &bar, tm(1, 2, 3, 0)), Some(Duration::from_millis(1002)), "playing: once a second");
        let (cfg, off) = (InstanceCfg::default(), BTreeMap::from([("progress".to_string(), Value::Bool(false))]));
        assert_eq!(src.next_wake(&bar, &SourceCx { cfg: &cfg, params: &off, tm: tm(1, 2, 3, 0), icon_pack: "Default" }), None, "an Instance that hides the bar never ticks");
    }

    #[test]
    fn cadence_is_asked_of_the_source_named_by_the_path() {
        let src = DataSources::builtin();
        assert_eq!(with_cx(|cx| src.cadence_of("sys.gauges", cx)), Some(Cadence::Second));
        assert_eq!(with_cx(|cx| src.cadence_of("clock", cx)), Some(Cadence::Frame), "a whole source changes as often as its fastest field");
        assert_eq!(with_cx(|cx| src.cadence_of("shortcuts.items", cx)), None);
        assert_eq!(with_cx(|cx| src.cadence_of("item.name", cx)), None, "not a source: a repeat variable");
    }

    #[test]
    fn sync_keeps_an_unchanged_source_alive() {
        use crate::code::runtime::{Limits, tests::returning};
        use crate::code::tests::start;
        let mut src = DataSources::builtin();
        let spec = |n: &str| CodeSpec { plugin: n.into(), source: n.into(), module: "nope.wasm".into(), hosts: vec![], fs_read: vec![], fs_read_params: vec![], launch: vec![], initial: Value::Nil };
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
        assert!(src.launch_rules(&deps(&["sys.gauges", "clock.minute"])).is_empty(), "no Code Source read");
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
            "player"
        }
        fn value(&self, _: &SourceCx) -> Value {
            Value::obj([("title", "Song".into())])
        }
        fn cadence(&self, _: &str, _: &SourceCx) -> Option<Cadence> {
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
        let cfg = InstanceCfg { id: "player-1".into(), ..Default::default() };
        let params = cfg.params_map();
        f(&SourceCx { cfg: &cfg, params: &params, tm: tm(1, 2, 3, 0), icon_pack: "Default" })
    }

    #[test]
    fn a_registered_source_is_read_acted_on_and_retained() {
        let mut src = DataSources::builtin();
        src.register(Box::new(Media::default())).unwrap();
        assert!(with_cx(|cx| src.value("player", cx)).is_some_and(|v| v.get("title").is_some()));
        assert!(with_cx(|cx| src.act("player", "play_pause", "", cx)), "on_click = \"player.play_pause\"");
        assert!(!with_cx(|cx| src.act("player", "eject", "", cx)), "a verb it does not know");
        assert!(!with_cx(|cx| src.act("nope", "x", "", cx)));
        src.retain(&BTreeSet::from(["player-1".to_string()]));
        assert!(src.names().contains(&"player".to_string()));
        assert!(src.register(Box::new(Media::default())).unwrap_err().contains("already"));
        struct Named(&'static str);
        impl DataSource for Named {
            fn name(&self) -> &str {
                self.0
            }
            fn value(&self, _: &SourceCx) -> Value {
                Value::Nil
            }
            fn cadence(&self, _: &str, _: &SourceCx) -> Option<Cadence> {
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
                "player"
            }
            fn value(&self, cx: &SourceCx) -> Value {
                self.0.value(cx)
            }
            fn cadence(&self, _: &str, _: &SourceCx) -> Option<Cadence> {
                None
            }
            fn attach(&self, n: Notifier) {
                self.0.attach(n)
            }
        }
        src.register(Box::new(Shared(media.clone()))).unwrap();
        let notify = media.notify.lock().unwrap().clone().expect("attached on register");
        std::thread::spawn(move || {
            notify.changed_for("player-1");
            notify.changed();
            notify.log("new track");
            notify.set_param("gallery-1", "folder", Value::Str("D:\\Photos".into()));
        })
        .join()
        .unwrap();
        rx.recv_timeout(Duration::from_secs(5)).expect("the app was woken");
        let news = src.take_news();
        let (name, n) = &news[0];
        assert_eq!((name.as_str(), n.all, n.changed.contains("player-1"), n.logs.clone()), ("player", true, true, vec!["new track".to_string()]));
        assert_eq!(n.params, [("gallery-1".to_string(), "folder".to_string(), Value::Str("D:\\Photos".into()))], "a param for the app to save");
        assert!(src.take_news().is_empty(), "taken once");
        assert!(DataSources::reads(&deps(&["player.title"]), "player") && !DataSources::reads(&deps(&["playerx.x"]), "player"));
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
            fn cadence(&self, _: &str, _: &SourceCx) -> Option<Cadence> {
                Some(Cadence::Minute)
            }
        }
        let src = DataSources::new(vec![Box::new(Weather)]);
        let cfg = InstanceCfg::default();
        let params = cfg.params_map();
        let cx = SourceCx { cfg: &cfg, params: &params, tm: tm(1, 2, 3, 0), icon_pack: "Default" };
        assert_eq!(src.value("weather", &cx).and_then(|v| v.get("temp").cloned()), Some(Value::Num(21.0)));
        assert_eq!(wake(&src, &deps(&["weather.temp"]), tm(12, 0, 0, 0)), Some(Duration::from_millis(60_002)));
    }
}
