//! Each Data Source declares how often its fields change, so a window wakes
//! only when a bound value can differ (decision 16, ADR-0004).

mod clock;
mod shortcuts;
mod sys;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use crate::code::{CodeSpec, News, Status, WasmSource};
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

pub trait DataSource: Send + Sync {
    fn name(&self) -> &str;
    fn value(&self, cx: &SourceCx) -> Value;
    /// `field` "" is the whole object. `None`: changes only on events, never with time.
    fn cadence(&self, field: &str) -> Option<Cadence>;
    fn watched_paths(&self, _cfg: &InstanceCfg) -> Vec<PathBuf> {
        vec![]
    }
    fn invalidate(&self) {}
}

pub struct DataSources {
    list: Vec<Box<dyn DataSource>>,
    /// Plugins' Code Sources, each with the key it was started from.
    code: Vec<(String, WasmSource)>,
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
        Self { list, code: Vec::new() }
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

    /// Sends `verb` to the Code Source `source`; false if there is none by that name.
    pub fn act(&self, source: &str, verb: &str, arg: &str, cx: &SourceCx) -> bool {
        self.code(source).map(|c| c.act(verb, arg, cx)).is_some()
    }

    /// Per Code Source name, what changed since last asked.
    pub fn take_news(&self) -> Vec<(String, News)> {
        self.code.iter().map(|(_, c)| (c.name().to_string(), c.take_news())).collect()
    }

    pub fn retain(&self, live: &BTreeSet<String>) {
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

    pub fn watched_paths(&self, cfg: &InstanceCfg) -> Vec<PathBuf> {
        self.list.iter().flat_map(|s| s.watched_paths(cfg)).collect()
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
