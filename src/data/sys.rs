//! One shared sample, at most every 800 ms however many widgets ask: CPU load and
//! network speed are deltas between samples, and the last minute is kept for graphs.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{Cadence, DataSource, SourceCx};
use crate::value::Value;

/// Samples kept for the graphs: about a minute at one per second.
pub const HISTORY: usize = 60;

/// What the next sample needs from this one.
#[derive(Clone, Default)]
struct State {
    /// (idle, busy+idle) system times, 100 ns ticks.
    cpu: (u64, u64),
    /// (received, sent) bytes on hardware interfaces, and when.
    net: Option<(u64, u64, Instant)>,
    cpu_history: VecDeque<f64>,
    ram_history: VecDeque<f64>,
    down_history: VecDeque<f64>,
}

struct Sampled {
    at: Instant,
    state: State,
    value: Value,
}

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
        let prev = g.as_ref().map(|s| s.state.clone()).unwrap_or_default();
        let (state, value) = sample_sys(prev);
        *g = Some(Sampled { at: Instant::now(), state, value: value.clone() });
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

/// Bytes received and sent by hardware interfaces (virtual switches would count twice).
fn net_bytes() -> Option<(u64, u64)> {
    use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    if unsafe { GetIfTable2(&mut table) }.is_err() || table.is_null() {
        return None;
    }
    let rows = unsafe { std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize) };
    let hardware = rows.iter().filter(|r| r.InterfaceAndOperStatusFlags._bitfield & 1 != 0);
    let sums = hardware.fold((0u64, 0u64), |(i, o), r| (i + r.InOctets, o + r.OutOctets));
    unsafe { FreeMibTable(table as *const _) };
    Some(sums)
}

/// (label, used, total) of every fixed drive, the system drive first.
fn drives() -> Vec<(String, u64, u64)> {
    use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDriveStringsW};
    use windows::core::HSTRING;
    let mut buf = [0u16; 512];
    let n = unsafe { GetLogicalDriveStringsW(Some(&mut buf)) } as usize;
    let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()).to_uppercase();
    let mut out: Vec<(String, u64, u64)> = String::from_utf16_lossy(&buf[..n.min(buf.len())])
        .split('\0')
        .filter(|root| !root.is_empty() && unsafe { GetDriveTypeW(&HSTRING::from(*root)) } == 3) // DRIVE_FIXED
        .filter_map(|root| {
            let (mut total, mut free) = (0u64, 0u64);
            unsafe { GetDiskFreeSpaceExW(&HSTRING::from(root), None, Some(&mut total), Some(&mut free)) }.ok()?;
            Some((root.trim_end_matches('\\').to_uppercase(), total.saturating_sub(free), total))
        })
        .collect();
    out.sort_by_key(|(l, _, _)| (*l != system, l.clone()));
    out
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

/// "0 KB/s" under a megabyte a second, "3.5 MB/s" above.
pub fn rate_text(bytes_per_sec: f64) -> String {
    let mb = bytes_per_sec / 1_048_576.0;
    if mb >= 1.0 { format!("{mb:.1} MB/s") } else { format!("{:.0} KB/s", bytes_per_sec / 1024.0) }
}

pub fn push_history(h: &mut VecDeque<f64>, v: f64) {
    h.push_back(v);
    while h.len() > HISTORY {
        h.pop_front();
    }
}

fn sample_sys(mut st: State) -> (State, Value) {
    use windows::Win32::System::Power::GetSystemPowerStatus;
    use windows::Win32::System::ProcessStatus::EnumProcesses;
    use windows::Win32::System::SystemInformation::{GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let cpu_now = cpu_times();
    let prev_cpu = st.cpu;
    let cpu = if cpu_now.1 > prev_cpu.1 && prev_cpu.1 > 0 { 100.0 * (1.0 - (cpu_now.0 - prev_cpu.0) as f64 / (cpu_now.1 - prev_cpu.1) as f64) } else { 0.0 };
    let cpu = cpu.clamp(0.0, 100.0).round();
    st.cpu = cpu_now;

    let mut mem = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    let _ = unsafe { GlobalMemoryStatusEx(&mut mem) };
    let ram_used = mem.ullTotalPhys.saturating_sub(mem.ullAvailPhys);
    let commit_used = mem.ullTotalPageFile.saturating_sub(mem.ullAvailPageFile);

    let drives = drives();
    let (sys_label, disk_used, total) = drives.first().cloned().unwrap_or_else(|| ("C:".into(), 0, 0));

    let (now, net) = (Instant::now(), net_bytes());
    let (down, up) = match (net, st.net) {
        (Some((i, o)), Some((pi, po, at))) => {
            let secs = now.saturating_duration_since(at).as_secs_f64().max(0.1);
            (i.saturating_sub(pi) as f64 / secs, o.saturating_sub(po) as f64 / secs)
        }
        _ => (0.0, 0.0),
    };
    st.net = net.map(|(i, o)| (i, o, now));

    let mut pids = [0u32; 4096];
    let mut needed = 0u32;
    let _ = unsafe { EnumProcesses(pids.as_mut_ptr(), std::mem::size_of_val(&pids) as u32, &mut needed) };

    let mut bat = Default::default();
    let has_battery = unsafe { GetSystemPowerStatus(&mut bat) }.is_ok() && bat.BatteryFlag & 128 == 0 && bat.BatteryLifePercent <= 100;

    push_history(&mut st.cpu_history, cpu);
    push_history(&mut st.ram_history, mem.dwMemoryLoad as f64);
    push_history(&mut st.down_history, down);

    let pct = |used: u64, total: u64| if total == 0 { 0.0 } else { (100.0 * used as f64 / total as f64).round() };
    let gauge = |id: &str, label: &str, value: f64, detail: String| {
        Value::obj([("id", id.into()), ("label", label.into()), ("value", value.into()), ("detail", detail.into())])
    };
    let cpu_g = gauge("cpu", "CPU", cpu, format!("{} processes", needed / 4));
    let ram_g = gauge("ram", "RAM", mem.dwMemoryLoad as f64, used_of_total(ram_used, mem.ullTotalPhys));
    let disk_g = gauge("disk", &sys_label, pct(disk_used, total), used_of_total(disk_used, total));
    let battery_g = has_battery.then(|| gauge("battery", "Battery", bat.BatteryLifePercent as f64, (if bat.ACLineStatus == 1 { "Charging" } else { "On battery" }).into()));

    let mut gauges = vec![cpu_g.clone(), ram_g.clone(), disk_g.clone()];
    gauges.extend(battery_g.clone());
    // the large tier: memory commit and every fixed drive too
    let mut all = vec![cpu_g, ram_g, gauge("commit", "Commit", pct(commit_used, mem.ullTotalPageFile), used_of_total(commit_used, mem.ullTotalPageFile)), disk_g];
    all.extend(drives.iter().skip(1).map(|(l, u, t)| gauge("drive", l, pct(*u, *t), used_of_total(*u, *t))));
    all.extend(battery_g);

    let list = |h: &VecDeque<f64>| Value::List(h.iter().map(|v| Value::Num(*v)).collect());
    // the network graph is scaled to its own busiest second, never below 1 MB/s
    let down_peak = st.down_history.iter().copied().fold(1_048_576.0, f64::max);
    let down_pct = Value::List(st.down_history.iter().map(|v| Value::Num((100.0 * v / down_peak).round())).collect());
    let v = Value::obj([
        ("cpu", cpu.into()),
        ("ram", (mem.dwMemoryLoad as i32).into()),
        ("ram_text", used_of_total(ram_used, mem.ullTotalPhys).into()),
        ("disk", pct(disk_used, total).into()),
        ("disk_text", used_of_total(disk_used, total).into()),
        ("disk_name", sys_label.as_str().into()),
        ("processes", ((needed / 4) as i32).into()),
        ("uptime", uptime_text(unsafe { GetTickCount64() }).into()),
        ("has_battery", has_battery.into()),
        ("battery", (if has_battery { bat.BatteryLifePercent as i32 } else { 0 }).into()),
        ("charging", (has_battery && bat.ACLineStatus == 1).into()),
        ("net_down", rate_text(down).into()),
        ("net_up", rate_text(up).into()),
        ("cpu_history", list(&st.cpu_history)),
        ("ram_history", list(&st.ram_history)),
        ("net_history", down_pct),
        ("gauges", Value::List(gauges)),
        ("gauges_all", Value::List(all)),
    ]);
    (st, v)
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

    #[test]
    fn the_big_monitor_gets_every_drive_commit_network_and_history() {
        let v = Sys::default().sample();
        let Some(Value::List(all)) = v.get("gauges_all") else { panic!("no gauges_all") };
        let ids: Vec<String> = all.iter().map(|g| g.get("id").unwrap().to_string()).collect();
        assert!(ids.starts_with(&["cpu".into(), "ram".into(), "commit".into(), "disk".into()]), "{ids:?}");
        assert!(all.iter().all(|g| (0.0..=100.0).contains(&g.get("value").and_then(|v| v.as_f64()).unwrap())));
        for k in ["cpu_history", "ram_history"] {
            assert!(matches!(v.get(k), Some(Value::List(h)) if !h.is_empty() && h.len() <= HISTORY), "{k}");
        }
        assert!(v.get("net_down").is_some() && v.get("net_up").is_some());
    }

    #[test]
    fn history_keeps_the_last_minute_and_rates_read_well() {
        let mut h = std::collections::VecDeque::new();
        for i in 0..70 {
            push_history(&mut h, i as f64);
        }
        assert_eq!((h.len(), h.front().copied(), h.back().copied()), (HISTORY, Some(10.0), Some(69.0)));
        assert_eq!((rate_text(0.0), rate_text(1536.0), rate_text(3.5 * 1048576.0)), ("0 KB/s".to_string(), "2 KB/s".to_string(), "3.5 MB/s".to_string()));
    }
}
