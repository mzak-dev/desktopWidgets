//! Data Sources: `clock` and `shortcuts`. Each declares how often its fields
//! change (`Cadence`), which is what lets the scheduler wake a window exactly
//! when a bound value can differ and never otherwise (decision 16).

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use crate::value::Value;

/// Broken-down local time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tm {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    /// 0 = Sunday.
    pub dow: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub ms: u32,
}

pub fn now_local() -> Tm {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    Tm {
        year: t.wYear as i32,
        month: t.wMonth as u32,
        day: t.wDay as u32,
        dow: t.wDayOfWeek as u32,
        hour: t.wHour as u32,
        minute: t.wMinute as u32,
        second: t.wSecond as u32,
        ms: t.wMilliseconds as u32,
    }
}

/// Localised date text via the user's Windows locale ("niedziela", "wrzesień"...).
fn fmt_date(tm: &Tm, pattern: &str) -> String {
    use windows::Win32::Foundation::SYSTEMTIME;
    use windows::Win32::Globalization::{ENUM_DATE_FORMATS_FLAGS, GetDateFormatEx};
    use windows::core::{HSTRING, PCWSTR};
    let st = SYSTEMTIME {
        wYear: tm.year as u16,
        wMonth: tm.month as u16,
        wDayOfWeek: tm.dow as u16,
        wDay: tm.day as u16,
        ..Default::default()
    };
    let mut buf = [0u16; 96];
    let n = unsafe { GetDateFormatEx(PCWSTR::null(), ENUM_DATE_FORMATS_FLAGS(0), Some(&st as *const _), &HSTRING::from(pattern), Some(&mut buf), PCWSTR::null()) };
    if n <= 1 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..(n - 1) as usize])
}

/// The `clock` data source as an object value.
pub fn clock_value(tm: &Tm) -> Value {
    let h12 = if tm.hour % 12 == 0 { 12 } else { tm.hour % 12 };
    let (h, m, s) = (tm.hour as f64, tm.minute as f64, tm.second as f64);
    Value::obj([
        ("hour", (tm.hour as i32).into()),
        ("hour12", (h12 as i32).into()),
        ("minute", (tm.minute as i32).into()),
        ("second", (tm.second as i32).into()),
        ("ms", (tm.ms as i32).into()),
        ("is_pm", (tm.hour >= 12).into()),
        ("ampm", (if tm.hour >= 12 { "PM" } else { "AM" }).into()),
        ("day", (tm.day as i32).into()),
        ("month", (tm.month as i32).into()),
        ("year", tm.year.into()),
        ("weekday", fmt_date(tm, "dddd").into()),
        ("weekday_short", fmt_date(tm, "ddd").into()),
        ("month_name", fmt_date(tm, "MMMM").into()),
        ("date", fmt_date(tm, "dddd, d MMMM").into()),
        ("date_short", fmt_date(tm, "d MMM").into()),
        // angles, degrees clockwise from 12
        ("hour_angle", ((h % 12.0) * 30.0 + m * 0.5).into()),
        // steps every 10 s (0.1 deg/s * 10): smooth to the eye, 6x cheaper than per-second
        ("minute_angle", (m * 6.0 + (s / 10.0).floor()).into()),
        ("second_angle", (s * 6.0).into()),
        ("second_smooth", (s * 6.0 + tm.ms as f64 * 0.006).into()),
    ])
}

/// How often a bound `clock.*` field can change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cadence {
    Frame,
    Second,
    TenSecond,
    Minute,
}

pub fn cadence_of(path: &str) -> Option<Cadence> {
    let field = path.strip_prefix("clock.")?;
    Some(match field {
        "ms" | "second_smooth" => Cadence::Frame,
        "second" | "second_angle" => Cadence::Second,
        "minute_angle" => Cadence::TenSecond,
        _ => Cadence::Minute,
    })
}

/// Time until the next moment any dependency can change; `None` when the
/// widget does not depend on the clock at all (it then never wakes on time).
pub fn next_wake(deps: &BTreeSet<String>, tm: &Tm) -> Option<Duration> {
    let fastest = deps.iter().filter_map(|d| cadence_of(d)).min()?;
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
pub fn is_continuous(deps: &BTreeSet<String>) -> bool {
    deps.iter().any(|d| cadence_of(d) == Some(Cadence::Frame))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shortcut {
    pub name: String,
    pub target: String,
    /// Optional explicit icon image path.
    pub icon: String,
}

impl Shortcut {
    pub fn from_value(v: &Value) -> Option<Shortcut> {
        let s = |k: &str| v.get(k).map(|x| x.to_string()).unwrap_or_default();
        let target = s("target");
        if target.is_empty() {
            return None;
        }
        let name = if s("name").is_empty() { file_stem(&target) } else { s("name") };
        Some(Shortcut { name, target, icon: s("icon") })
    }

    pub fn to_value(&self) -> Value {
        Value::obj([("name", self.name.as_str().into()), ("target", self.target.as_str().into()), ("icon", self.icon.as_str().into())])
    }
}

pub fn file_stem(target: &str) -> String {
    let t = target.trim_end_matches(['\\', '/']);
    Path::new(t).file_stem().and_then(|s| s.to_str()).unwrap_or(t).to_string()
}

/// Entries of a folder as shortcuts (`.lnk`, `.exe`, `.url`, files), capped.
pub fn folder_items(dir: &str, cap: usize) -> Vec<Shortcut> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<Shortcut> = rd
        .filter_map(|e| e.ok())
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.') && e.file_name().to_string_lossy() != "desktop.ini")
        .map(|e| {
            let p = e.path();
            Shortcut { name: file_stem(&p.to_string_lossy()), target: p.to_string_lossy().into_owned(), icon: String::new() }
        })
        .collect();
    v.sort_by_key(|s| s.name.to_lowercase());
    v.truncate(cap);
    v
}

/// Separates the parts of an image id; not a legal path character.
pub const ID_SEP: char = '\u{1f}';

/// `icon:<pack>SEP<target>SEP<explicit icon path>`: what the icon service resolves.
pub fn icon_id(pack: &str, s: &Shortcut) -> String {
    format!("icon:{pack}{ID_SEP}{}{ID_SEP}{}", s.target, s.icon)
}

/// The `shortcuts` data source: an explicit list, or a folder mirrored live.
pub fn shortcuts_value(items: &[Shortcut], pack: &str) -> Value {
    let list = items
        .iter()
        .map(|s| {
            let Value::Obj(mut m) = s.to_value() else { unreachable!() };
            m.insert("icon_id".into(), Value::Str(icon_id(pack, s)));
            Value::Obj(m)
        })
        .collect();
    Value::obj([("items", Value::List(list)), ("count", (items.len() as i32).into())])
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
    fn clock_fields_and_angles() {
        let v = clock_value(&tm(15, 30, 20, 500));
        assert_eq!(v.get("hour12"), Some(&Value::Num(3.0)));
        assert_eq!(v.get("ampm"), Some(&Value::Str("PM".into())));
        assert_eq!(v.get("hour_angle"), Some(&Value::Num(105.0))); // 3:30 -> 90 + 15
        assert_eq!(v.get("minute_angle"), Some(&Value::Num(182.0))); // 30*6 + floor(20/10)
        assert_eq!(v.get("second_angle"), Some(&Value::Num(120.0)));
        assert_eq!(clock_value(&tm(0, 0, 0, 0)).get("hour12"), Some(&Value::Num(12.0)));
    }

    #[test]
    fn wakes_exactly_when_a_bound_value_can_change() {
        // no clock dependency: never wakes on time
        assert_eq!(next_wake(&deps(&["param.x", "shortcuts.items"]), &tm(1, 2, 3, 0)), None);
        // minute-level: next minute boundary (+2ms guard), not next second
        assert_eq!(next_wake(&deps(&["clock.minute"]), &tm(12, 0, 30, 500)), Some(Duration::from_millis(29_500 + 2)));
        // second-level beats minute-level when both are present
        assert_eq!(next_wake(&deps(&["clock.minute", "clock.second"]), &tm(12, 0, 30, 250)), Some(Duration::from_millis(750 + 2)));
        // ten-second minute-hand step
        assert_eq!(next_wake(&deps(&["clock.minute_angle"]), &tm(12, 0, 23, 0)), Some(Duration::from_millis(7_000 + 2)));
        // smooth second hand needs frames
        let d = deps(&["clock.second_smooth"]);
        assert!(is_continuous(&d));
        assert!(!is_continuous(&deps(&["clock.second"])));
    }

    #[test]
    fn shortcut_round_trip_and_name_fallback() {
        let s = Shortcut { name: "".into(), target: "C:\\Apps\\Foo Bar.exe".into(), icon: "".into() };
        let back = Shortcut::from_value(&s.to_value()).unwrap();
        assert_eq!(back.name, "Foo Bar");
        assert!(Shortcut::from_value(&Value::obj([("name", "x".into())])).is_none()); // no target
    }
}
