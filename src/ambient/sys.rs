//! What the system monitor reads from the machine, as plain data: one `Reading` per
//! sample. The counters are raw (CPU times, byte totals); turning them into rates, gauges
//! and history is the `sys` Data Source's job, so it can be tested on scripted readings.

/// Physical memory and the commit charge.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Memory {
    /// Percent of physical memory in use, as Windows reports it.
    pub load_pct: u32,
    pub total_phys: u64,
    pub avail_phys: u64,
    pub total_page: u64,
    pub avail_page: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Battery {
    pub percent: u8,
    /// On AC power.
    pub charging: bool,
}

/// A graphics adapter and its load right now.
#[derive(Clone, Debug, PartialEq)]
pub struct GpuLoad {
    pub label: String,
    pub id: &'static str,
    /// Unique among adapters: `gpu`, `gpu2`, `igpu`.
    pub key: String,
    pub name: String,
    /// Percent busy, 0-100.
    pub load: f64,
}

/// The machine at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    /// (idle, busy + idle) system times, 100 ns ticks since boot.
    pub cpu: (u64, u64),
    pub mem: Memory,
    /// (label, used, total) bytes of every fixed drive, the system drive first.
    pub drives: Vec<(String, u64, u64)>,
    /// (received, sent) bytes on hardware interfaces since boot, when readable.
    pub net: Option<(u64, u64)>,
    pub processes: u32,
    /// `None` on a machine without a battery.
    pub battery: Option<Battery>,
    pub uptime_ms: u64,
    pub gpus: Vec<GpuLoad>,
}

impl Reading {
    /// A busy-looking desktop: 16 GB of RAM with 9.5 GB used, a system drive and a data
    /// drive, one GPU at 20 %, three days and four hours up, no battery. The CPU counters
    /// stand at a fixed point; a `ScriptedProbe::demo` moves them on to make 37 %.
    pub fn demo() -> Reading {
        const GB: u64 = 1 << 30;
        Reading {
            cpu: (63_000_000, 100_000_000),
            mem: Memory { load_pct: 59, total_phys: 16 * GB, avail_phys: 16 * GB - 9 * GB - GB / 2, total_page: 32 * GB, avail_page: 18 * GB },
            drives: vec![("C:".into(), 220 * GB, 460 * GB), ("D:".into(), 710 * GB, 1000 * GB)],
            net: Some((1_000_000_000, 200_000_000)),
            processes: 212,
            battery: None,
            uptime_ms: 3 * 86_400_000 + 4 * 3_600_000,
            gpus: vec![GpuLoad { label: "GPU".into(), id: "gpu", key: "gpu".into(), name: "8 GB VRAM".into(), load: 20.0 }],
        }
    }
}

/// Reads the machine. `read` may be called from any thread; the `sys` source serializes its
/// samples.
pub trait SysProbe: Send + Sync {
    fn read(&self) -> Reading;
}
