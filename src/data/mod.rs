//! Data Sources (CONTEXT.md): named producers of values that widget
//! definitions bind to. Each one is a module implementing `DataSource`, and
//! declares how often each of its fields can change (`Cadence`), which is what
//! lets the scheduler wake a window exactly when a bound value can differ and
//! never otherwise (decision 16, ADR-0004).
//!
//! Adding a source: a file here implementing `DataSource`, and a line in
//! `DataSources::builtin`. Widgets then bind to `{<name>.<field>}`.

mod clock;
mod shortcuts;
mod sys;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use crate::value::Value;
use crate::workspace::InstanceCfg;

pub use clock::{Clock, Tm, clock_value, now_local};
pub use shortcuts::{ID_SEP, Shortcut, Shortcuts, file_stem, folder_items, icon_id, shortcuts_value, starter_apps};
pub use sys::Sys;

/// How often a bound field can change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cadence {
    Frame,
    Second,
    TenSecond,
    Minute,
}

/// What a source may look at when producing its value for one Instance.
pub struct SourceCx<'a> {
    pub cfg: &'a InstanceCfg,
    /// The moment being drawn.
    pub tm: Tm,
    pub icon_pack: &'a str,
}

pub trait DataSource: Send + Sync {
    /// The root name widgets bind to (`clock` in `{clock.minute}`).
    fn name(&self) -> &'static str;
    /// The value for one Instance at one moment. Only called when a widget
    /// actually reads this source, at most once per build.
    fn value(&self, cx: &SourceCx) -> Value;
    /// How often `field` (the path after the name; "" for the whole object)
    /// can change. `None`: only on events, so it never wakes a window by time.
    fn cadence(&self, field: &str) -> Option<Cadence>;
    /// Paths whose changes this source reflects for `cfg` (they are watched).
    fn watch(&self, _cfg: &InstanceCfg) -> Vec<PathBuf> {
        vec![]
    }
    /// Forget cached data: a watched path or an Instance's params changed.
    fn invalidate(&self) {}
}

/// The registry of Data Sources.
pub struct DataSources {
    list: Vec<Box<dyn DataSource>>,
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
        Self { list }
    }

    pub fn get(&self, name: &str) -> Option<&dyn DataSource> {
        self.list.iter().find(|s| s.name() == name).map(|s| s.as_ref())
    }

    /// The value of source `name` for one Instance, or `None` if there is no such source.
    pub fn value(&self, name: &str, cx: &SourceCx) -> Option<Value> {
        self.get(name).map(|s| s.value(cx))
    }

    /// How often a bound dotted path (`clock.minute`) can change.
    pub fn cadence_of(&self, path: &str) -> Option<Cadence> {
        let (root, field) = path.split_once('.').unwrap_or((path, ""));
        self.get(root)?.cadence(field)
    }

    /// Time until the next moment any dependency can change; `None` when the
    /// widget depends on nothing that changes with time (it then never wakes on time).
    pub fn next_wake(&self, deps: &BTreeSet<String>, tm: &Tm) -> Option<Duration> {
        let fastest = deps.iter().filter_map(|d| self.cadence_of(d)).min()?;
        let into_sec = tm.ms as u64;
        let ms = match fastest {
            Cadence::Frame => 8,
            Cadence::Second => 1000 - into_sec,
            Cadence::TenSecond => (10 - (tm.second % 10)) as u64 * 1000 - into_sec,
            Cadence::Minute => (60 - tm.second) as u64 * 1000 - into_sec,
        };
        Some(Duration::from_millis(ms + 2)) // land just after the boundary, never just before
    }

    /// True when the widget needs a frame every vsync (smooth second hand).
    pub fn is_continuous(&self, deps: &BTreeSet<String>) -> bool {
        deps.iter().any(|d| self.cadence_of(d) == Some(Cadence::Frame))
    }

    /// Every path any source watches for `cfg`.
    pub fn watch(&self, cfg: &InstanceCfg) -> Vec<PathBuf> {
        self.list.iter().flat_map(|s| s.watch(cfg)).collect()
    }

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
        assert!(src.is_continuous(&deps(&["clock.second_smooth"])));
        assert!(!src.is_continuous(&deps(&["clock.second"])));
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
    fn a_new_source_is_one_impl_and_one_registration() {
        struct Weather;
        impl DataSource for Weather {
            fn name(&self) -> &'static str {
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
        let cx = SourceCx { cfg: &cfg, tm: tm(1, 2, 3, 0), icon_pack: "Default" };
        assert_eq!(src.value("weather", &cx).and_then(|v| v.get("temp").cloned()), Some(Value::Num(21.0)));
        assert_eq!(src.next_wake(&deps(&["weather.temp"]), &tm(12, 0, 0, 0)), Some(Duration::from_millis(60_002)));
    }
}
