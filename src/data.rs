//! Data Sources: `clock`, `sys` and `shortcuts`. Each declares how often its fields
//! change (`Cadence`), which is what lets the scheduler wake a window exactly
//! when a bound value can differ and never otherwise (decision 16).

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

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
    if path.starts_with("sys.") {
        return Some(Cadence::Second);
    }
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

// ---- sys ---------------------------------------------------------------------

struct Sampled {
    at: Instant,
    /// (idle, busy+idle) system times, 100 ns ticks, for the next CPU delta.
    cpu: (u64, u64),
    value: Value,
}

// ponytail: one shared sample so N widgets cost one set of syscalls; per-widget
// rates would need a sampler per Instance.
static SYS: Mutex<Option<Sampled>> = Mutex::new(None);

fn cpu_times() -> (u64, u64) {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::GetSystemTimes;
    let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    if unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.is_err() {
        return (0, 0);
    }
    let t = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    (t(idle), t(kernel) + t(user)) // kernel time already includes idle
}

fn gb(bytes: u64) -> f64 {
    bytes as f64 / (1u64 << 30) as f64
}

fn used_of_total(used: u64, total: u64) -> String {
    format!("{:.1} / {:.0} GB", gb(used), gb(total))
}

fn uptime_text(ms: u64) -> String {
    let (d, h, m) = (ms / 86_400_000, ms / 3_600_000 % 24, ms / 60_000 % 60);
    if d > 0 { format!("{d}d {h}h") } else { format!("{h}h {m}m") }
}

fn sample_sys(prev_cpu: Option<(u64, u64)>) -> ((u64, u64), Value) {
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows::Win32::System::Power::GetSystemPowerStatus;
    use windows::Win32::System::ProcessStatus::EnumProcesses;
    use windows::Win32::System::SystemInformation::{GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows::core::HSTRING;

    let cpu_now = cpu_times();
    let cpu = prev_cpu
        .filter(|p| cpu_now.1 > p.1)
        .map_or(0.0, |p| 100.0 * (1.0 - (cpu_now.0 - p.0) as f64 / (cpu_now.1 - p.1) as f64));

    let mut mem = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    let _ = unsafe { GlobalMemoryStatusEx(&mut mem) };
    let ram_used = mem.ullTotalPhys.saturating_sub(mem.ullAvailPhys);

    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()) + "\\";
    let (mut total, mut free) = (0u64, 0u64);
    let _ = unsafe { GetDiskFreeSpaceExW(&HSTRING::from(drive.as_str()), None, Some(&mut total), Some(&mut free)) };
    let disk_used = total.saturating_sub(free);

    let mut pids = [0u32; 4096];
    let mut needed = 0u32;
    let _ = unsafe { EnumProcesses(pids.as_mut_ptr(), std::mem::size_of_val(&pids) as u32, &mut needed) };

    let mut bat = Default::default();
    let has_battery = unsafe { GetSystemPowerStatus(&mut bat) }.is_ok() && bat.BatteryFlag & 128 == 0 && bat.BatteryLifePercent <= 100;

    let pct = |used: u64, total: u64| if total == 0 { 0.0 } else { (100.0 * used as f64 / total as f64).round() };
    let gauge = |id: &str, label: &str, value: f64, detail: String| {
        Value::obj([("id", id.into()), ("label", label.into()), ("value", value.into()), ("detail", detail.into())])
    };
    let mut gauges = vec![
        gauge("cpu", "CPU", cpu.clamp(0.0, 100.0).round(), format!("{} processes", needed / 4)),
        gauge("ram", "RAM", mem.dwMemoryLoad as f64, used_of_total(ram_used, mem.ullTotalPhys)),
        gauge("disk", drive.trim_end_matches('\\'),pct(disk_used, total), used_of_total(disk_used, total)),
    ];
    if has_battery {
        gauges.push(gauge("battery", "Battery", bat.BatteryLifePercent as f64, (if bat.ACLineStatus == 1 { "Charging" } else { "On battery" }).into()));
    }
    let v = Value::obj([
        ("cpu", cpu.clamp(0.0, 100.0).round().into()),
        ("ram", (mem.dwMemoryLoad as i32).into()),
        ("ram_text", used_of_total(ram_used, mem.ullTotalPhys).into()),
        ("disk", pct(disk_used, total).into()),
        ("disk_text", used_of_total(disk_used, total).into()),
        ("disk_name", drive.trim_end_matches('\\').into()),
        ("processes", ((needed / 4) as i32).into()),
        ("uptime", uptime_text(unsafe { GetTickCount64() }).into()),
        ("has_battery", has_battery.into()),
        ("battery", (if has_battery { bat.BatteryLifePercent as i32 } else { 0 }).into()),
        ("charging", (has_battery && bat.ACLineStatus == 1).into()),
        ("gauges", Value::List(gauges)),
    ]);
    (cpu_now, v)
}

/// The `sys` data source: live machine load. Sampled at most once per ~second
/// no matter how many widgets ask, since CPU load is a delta between samples.
pub fn sys_value() -> Value {
    let mut g = SYS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = g.as_ref().filter(|s| s.at.elapsed() < Duration::from_millis(800)) {
        return s.value.clone();
    }
    let (cpu, value) = sample_sys(g.as_ref().map(|s| s.cpu));
    *g = Some(Sampled { at: Instant::now(), cpu, value: value.clone() });
    value
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
    fn sys_gauges_are_percentages_and_uptime_reads_well() {
        let v = sys_value();
        let Some(Value::List(g)) = v.get("gauges") else { panic!("no gauges") };
        assert!(g.len() >= 3, "cpu, ram and disk are always there");
        for x in g {
            let n = x.get("value").and_then(|v| v.as_f64()).unwrap();
            assert!((0.0..=100.0).contains(&n), "{x:?}");
        }
        assert_eq!(uptime_text(3 * 86_400_000 + 4 * 3_600_000), "3d 4h");
        assert_eq!(uptime_text(5 * 60_000), "0h 5m");
        assert_eq!(cadence_of("sys.gauges"), Some(Cadence::Second));
    }
    #[test]
    fn shortcut_round_trip_and_name_fallback() {
        let s = Shortcut { name: "".into(), target: "C:\\Apps\\Foo Bar.exe".into(), icon: "".into() };
        let back = Shortcut::from_value(&s.to_value()).unwrap();
        assert_eq!(back.name, "Foo Bar");
        assert!(Shortcut::from_value(&Value::obj([("name", "x".into())])).is_none()); // no target
    }
}
