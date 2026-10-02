//! The real system probe: CPU times, memory, drives, network counters, processes, battery
//! and the GPU engines' load, all from Win32.

use std::sync::Mutex;

use super::sys::{Battery, GpuLoad, Memory, Reading, SysProbe};

/// Reads the machine. The GPU query is opened at the first read and kept.
#[derive(Default)]
pub struct WinProbe {
    gpus: Mutex<Gpus>,
}

impl SysProbe for WinProbe {
    fn read(&self) -> Reading {
        use windows::Win32::System::Power::GetSystemPowerStatus;
        use windows::Win32::System::ProcessStatus::EnumProcesses;
        use windows::Win32::System::SystemInformation::{GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX};

        let gpus = self.gpus.lock().unwrap_or_else(|e| e.into_inner()).sample();
        let cpu = cpu_times();

        let mut mem = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        let _ = unsafe { GlobalMemoryStatusEx(&mut mem) };

        let mut pids = [0u32; 4096];
        let mut needed = 0u32;
        let _ = unsafe { EnumProcesses(pids.as_mut_ptr(), std::mem::size_of_val(&pids) as u32, &mut needed) };

        let mut bat = Default::default();
        let has_battery = unsafe { GetSystemPowerStatus(&mut bat) }.is_ok() && bat.BatteryFlag & 128 == 0 && bat.BatteryLifePercent <= 100;

        Reading {
            cpu,
            mem: Memory { load_pct: mem.dwMemoryLoad, total_phys: mem.ullTotalPhys, avail_phys: mem.ullAvailPhys, total_page: mem.ullTotalPageFile, avail_page: mem.ullAvailPageFile },
            drives: drives(),
            net: net_bytes(),
            processes: needed / 4,
            battery: has_battery.then(|| Battery { percent: bat.BatteryLifePercent, charging: bat.ACLineStatus == 1 }),
            uptime_ms: unsafe { GetTickCount64() },
            gpus,
        }
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

/// PDH handles are process-wide and only touched under the probe's lock.
struct Query(windows::Win32::System::Performance::PDH_HQUERY, windows::Win32::System::Performance::PDH_HCOUNTER);
unsafe impl Send for Query {}

/// (luid as counter instances spell it, VRAM detail line, integrated)
type Adapter = (String, String, bool);

/// The adapters DXGI lists, and the "GPU Engine" performance counters that say how busy
/// each one is. Counters are rates, so the first sample after opening reads 0.
#[derive(Default)]
struct Gpus {
    /// Software adapters and virtual duplicates of a real one left out.
    adapters: Option<Vec<Adapter>>,
    query: Option<Query>,
}

fn list_adapters() -> Vec<Adapter> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1};
    let Ok(f) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else { return vec![] };
    let mut seen = vec![];
    (0..)
        .map_while(|i| unsafe { f.EnumAdapters1(i) }.ok())
        .filter_map(|a| {
            let d = unsafe { a.GetDesc1() }.ok()?;
            // ponytail: a virtual display driver (Parsec) shows up as a second copy of the real GPU
            // with the same hardware ids; the first listed is the real one. Two identical cards lose one.
            let hw = (d.VendorId, d.DeviceId, d.SubSysId, d.Revision);
            if d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 || seen.contains(&hw) {
                return None;
            }
            seen.push(hw);
            let vram = d.DedicatedVideoMemory as u64;
            let name = if vram >= 1 << 30 { format!("{:.0} GB VRAM", gb(vram)) } else { format!("{} MB VRAM", vram >> 20) };
            let luid = format!("luid_0x{:08x}_0x{:08x}", d.AdapterLuid.HighPart as u32, d.AdapterLuid.LowPart);
            // ponytail: integrated = under 1 GiB of dedicated memory; a big UMA carve-out reads as discrete
            Some((luid, name, d.DedicatedVideoMemory < 1 << 30))
        })
        .collect()
}

/// Busy percent per LUID: engines of one type add up across processes, and an adapter is
/// as busy as its busiest engine type (what Task Manager shows).
fn gpu_loads(q: &Query) -> Vec<(String, f64)> {
    use windows::Win32::System::Performance::*;
    let mut size = 0u32;
    let mut count = 0u32;
    unsafe {
        PdhCollectQueryData(q.0);
        // the size probe answers PDH_MORE_DATA, the real call fills the buffer
        PdhGetFormattedCounterArrayW(q.1, PDH_FMT_DOUBLE, &mut size, &mut count, None);
        if size == 0 {
            return vec![];
        }
        let mut buf = vec![0u64; (size as usize).div_ceil(8)]; // 8-aligned for the items
        let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
        if PdhGetFormattedCounterArrayW(q.1, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items)) != 0 {
            return vec![];
        }
        let mut by: std::collections::HashMap<(String, String), f64> = Default::default();
        for it in std::slice::from_raw_parts(items, count as usize) {
            let name = it.szName.to_string().unwrap_or_default();
            let (Some(l), Some(t)) = (name.find("luid_"), name.find("engtype_")) else { continue };
            let luid = name[l..].split("_phys").next().unwrap_or("").to_lowercase(); // instances spell the hex in capitals
            *by.entry((luid, name[t..].into())).or_default() += it.FmtValue.Anonymous.doubleValue;
        }
        let mut out: std::collections::HashMap<String, f64> = Default::default();
        for ((luid, _), v) in by {
            let e = out.entry(luid).or_default();
            *e = e.max(v);
        }
        out.into_iter().collect()
    }
}

impl Gpus {
    fn sample(&mut self) -> Vec<GpuLoad> {
        use windows::Win32::System::Performance::*;
        use windows::core::w;
        let adapters = self.adapters.get_or_insert_with(list_adapters).clone();
        if adapters.is_empty() {
            return vec![];
        }
        if self.query.is_none() {
            let mut q = PDH_HQUERY::default();
            let mut c = PDH_HCOUNTER::default();
            let ok = unsafe { PdhOpenQueryW(None, 0, &mut q) } == 0
                && unsafe { PdhAddEnglishCounterW(q, w!("\\GPU Engine(*)\\Utilization Percentage"), 0, &mut c) } == 0;
            if !ok {
                self.adapters = Some(vec![]); // no GPU counters on this machine: don't retry every second
                return vec![];
            }
            self.query = Some(Query(q, c));
        }
        let loads = gpu_loads(self.query.as_ref().unwrap());
        let discrete = adapters.iter().filter(|a| !a.2).count();
        let mut n = 0;
        adapters
            .iter()
            .map(|(luid, name, integrated)| {
                n += (!integrated) as usize;
                let load = loads.iter().find(|(l, _)| l == luid).map_or(0.0, |(_, v)| v.clamp(0.0, 100.0).round());
                let label = if *integrated { "iGPU".into() } else if discrete > 1 { format!("GPU {n}") } else { "GPU".into() };
                let key = if *integrated { "igpu".into() } else if n == 1 { "gpu".into() } else { format!("gpu{n}") };
                GpuLoad { label, id: if *integrated { "igpu" } else { "gpu" }, key, name: name.clone(), load }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_machine_reads_as_a_plausible_reading() {
        let probe = WinProbe::default();
        let r = probe.read();
        assert!(r.cpu.1 >= r.cpu.0 && r.cpu.1 > 0, "idle time is part of the total");
        assert!(r.mem.total_phys > 0 && r.mem.avail_phys <= r.mem.total_phys && r.mem.load_pct <= 100);
        assert!(!r.drives.is_empty() && r.drives.iter().all(|(_, used, total)| used <= total));
        assert!(r.uptime_ms > 0);
        for g in &r.gpus {
            assert!((0.0..=100.0).contains(&g.load), "{}", g.label);
        }
    }
}
