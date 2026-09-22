//! Each Data Source declares how often its fields change, so a window wakes
//! only when a bound value can differ (decision 16, ADR-0004).

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cadence {
    Frame,
    Second,
    TenSecond,
    Minute,
}

pub struct SourceCx<'a> {
    pub cfg: &'a InstanceCfg,
    pub tm: Tm,
    pub icon_pack: &'a str,
}

pub trait DataSource: Send + Sync {
    fn name(&self) -> &'static str;
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
