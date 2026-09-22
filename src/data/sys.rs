//! The `sys` Data Source: live machine load (CPU, memory, system drive,
//! battery, uptime). One shared sample, taken at most once per ~second no
//! matter how many widgets ask, since CPU load is a delta between samples.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{Cadence, DataSource, SourceCx};
use crate::value::Value;

struct Sampled {
    at: Instant,
    /// (idle, busy+idle) system times, 100 ns ticks, for the next CPU delta.
    cpu: (u64, u64),
    value: Value,
}

// ponytail: one shared sample so N widgets cost one set of syscalls; per-widget
// rates would need a sampler per Instance.
#[derive(Default)]
pub struct Sys {
    last: Mutex<Option<Sampled>>,
}

impl Sys {
    pub fn sample(&self) -> Value {
        let mut g = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = g.as_ref().filter(|s| s.at.elapsed() < Duration::from_millis(800)) {
            return s.value.clone();
        }
        let (cpu, value) = sample_sys(g.as_ref().map(|s| s.cpu));
        *g = Some(Sampled { at: Instant::now(), cpu, value: value.clone() });
        value
    }
}

impl DataSource for Sys {
    fn name(&self) -> &'static str {
        "sys"
    }

    fn value(&self, _cx: &SourceCx) -> Value {
        self.sample()
    }

    fn cadence(&self, _field: &str) -> Option<Cadence> {
        Some(Cadence::Second)
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sys_gauges_are_percentages_and_uptime_reads_well() {
        let v = Sys::default().sample();
        let Some(Value::List(g)) = v.get("gauges") else { panic!("no gauges") };
        assert!(g.len() >= 3, "cpu, ram and disk are always there");
        for x in g {
            let n = x.get("value").and_then(|v| v.as_f64()).unwrap();
            assert!((0.0..=100.0).contains(&n), "{x:?}");
        }
        assert_eq!(uptime_text(3 * 86_400_000 + 4 * 3_600_000), "3d 4h");
        assert_eq!(uptime_text(5 * 60_000), "0h 5m");
        assert_eq!(Sys::default().cadence("gauges"), Some(Cadence::Second));
    }
}
