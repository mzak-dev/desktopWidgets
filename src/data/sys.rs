//! One shared sample, at most every 800 ms however many widgets ask: CPU load and
//! network speed are deltas between samples, and the last minute is kept for graphs. What
//! the machine says comes from a `SysProbe`; turning it into gauges is pure.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{Cadence, DataSource, SourceCx};
use crate::ambient::{Reading, SysProbe};
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
    /// Per adapter, in `Reading::gpus` order.
    gpu_history: Vec<VecDeque<f64>>,
}

struct Sampled {
    at: Instant,
    state: State,
    value: Value,
}

pub struct Sys {
    probe: Arc<dyn SysProbe>,
    last: Mutex<Option<Sampled>>,
}

impl Sys {
    pub fn new(probe: Arc<dyn SysProbe>) -> Sys {
        Sys { probe, last: Mutex::new(None) }
    }

    pub fn sample(&self, now: Instant) -> Value {
        let mut g = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = g.as_ref().filter(|s| now.saturating_duration_since(s.at) < Duration::from_millis(800)) {
            return s.value.clone();
        }
        let prev = g.as_ref().map(|s| s.state.clone()).unwrap_or_default();
        let (state, value) = sample_sys(prev, &self.probe.read(), now);
        *g = Some(Sampled { at: now, state, value: value.clone() });
        value
    }
}

impl DataSource for Sys {
    fn name(&self) -> &str {
        "sys"
    }

    fn value(&self, cx: &SourceCx) -> Value {
        self.sample(cx.now())
    }

    fn cadence(&self, _field: &str, _cx: &SourceCx) -> Option<Cadence> {
        Some(Cadence::Second)
    }
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


/// The gauges, graphs and history for the machine as `r` reads now, given what the previous
/// sample left in `st`. Pure: no machine access, so the same readings give the same values.
fn sample_sys(mut st: State, r: &Reading, now: Instant) -> (State, Value) {
    let gpus = &r.gpus;
    let mem = &r.mem;
    let cpu_now = r.cpu;
    let prev_cpu = st.cpu;
    let cpu = if cpu_now.1 > prev_cpu.1 && prev_cpu.1 > 0 { 100.0 * (1.0 - (cpu_now.0 - prev_cpu.0) as f64 / (cpu_now.1 - prev_cpu.1) as f64) } else { 0.0 };
    let cpu = cpu.clamp(0.0, 100.0).round();
    st.cpu = cpu_now;

    let ram_used = mem.total_phys.saturating_sub(mem.avail_phys);
    let commit_used = mem.total_page.saturating_sub(mem.avail_page);

    let drives = &r.drives;
    let (sys_label, disk_used, total) = drives.first().cloned().unwrap_or_else(|| ("C:".into(), 0, 0));

    let net = r.net;
    let (down, up) = match (net, st.net) {
        (Some((i, o)), Some((pi, po, at))) => {
            let secs = now.saturating_duration_since(at).as_secs_f64().max(0.1);
            (i.saturating_sub(pi) as f64 / secs, o.saturating_sub(po) as f64 / secs)
        }
        _ => (0.0, 0.0),
    };
    st.net = net.map(|(i, o)| (i, o, now));

    let battery = r.battery.as_ref();
    let has_battery = battery.is_some();
    let charging = battery.is_some_and(|b| b.charging);

    push_history(&mut st.cpu_history, cpu);
    push_history(&mut st.ram_history, mem.load_pct as f64);
    push_history(&mut st.down_history, down);
    st.gpu_history.resize_with(gpus.len(), Default::default);
    for (h, g) in st.gpu_history.iter_mut().zip(gpus) {
        push_history(h, g.load);
    }

    let pct = |used: u64, total: u64| if total == 0 { 0.0 } else { (100.0 * used as f64 / total as f64).round() };
    let gauge = |id: &str, key: &str, label: &str, value: f64, detail: String| {
        Value::obj([("id", id.into()), ("key", key.into()), ("label", label.into()), ("value", value.into()), ("detail", detail.into())])
    };
    let cpu_g = gauge("cpu", "cpu", "CPU", cpu, format!("{} processes", r.processes));
    let ram_g = gauge("ram", "ram", "RAM", mem.load_pct as f64, used_of_total(ram_used, mem.total_phys));
    let disk_g = gauge("disk", "disk", &sys_label, pct(disk_used, total), used_of_total(disk_used, total));
    let battery_g = battery.map(|b| gauge("battery", "battery", "Battery", b.percent as f64, (if b.charging { "Charging" } else { "On battery" }).into()));

    let gpu_gs: Vec<Value> = gpus.iter().map(|g| gauge(g.id, &g.key, &g.label, g.load, g.name.clone())).collect();
    let mut gauges = vec![cpu_g.clone(), ram_g.clone()];
    gauges.extend(gpu_gs.clone());
    gauges.push(disk_g.clone());
    gauges.extend(battery_g.clone());
    // the large tier: memory commit and every fixed drive too
    let mut all = vec![cpu_g, ram_g];
    all.extend(gpu_gs);
    all.extend([gauge("commit", "commit", "Commit", pct(commit_used, mem.total_page), used_of_total(commit_used, mem.total_page)), disk_g]);
    all.extend(drives.iter().skip(1).map(|(l, u, t)| gauge("drive", &format!("drive:{l}"), l, pct(*u, *t), used_of_total(*u, *t))));
    all.extend(battery_g);

    let list = |h: &VecDeque<f64>| Value::List(h.iter().map(|v| Value::Num(*v)).collect());
    // the network graph is scaled to its own busiest second, never below 1 MB/s
    let down_peak = st.down_history.iter().copied().fold(1_048_576.0, f64::max);
    let down_pct = Value::List(st.down_history.iter().map(|v| Value::Num((100.0 * v / down_peak).round())).collect());
    // one card per adapter for the large monitor's graphs
    let gpu_cards = Value::List(
        gpus.iter()
            .zip(&st.gpu_history)
            .map(|(g, h)| Value::obj([("label", g.label.as_str().into()), ("value", g.load.into()), ("history", list(h))]))
            .collect(),
    );
    // one card per graph of the large monitor, keyed like the gauges they sit beside
    let graph = |key: &str, label: &str, text: String, values: Value| Value::obj([("key", key.into()), ("label", label.into()), ("text", text.into()), ("values", values)]);
    let mut graphs = vec![
        graph("cpu", "CPU", format!("{cpu}%"), list(&st.cpu_history)),
        graph("ram", "Memory", format!("{}%", mem.load_pct), list(&st.ram_history)),
        graph("net", "Download", rate_text(down), down_pct.clone()),
    ];
    graphs.extend(gpus.iter().zip(&st.gpu_history).map(|(g, h)| graph(&g.key, &g.label, format!("{}%", g.load), list(h))));
    let v = Value::obj([
        ("graphs", Value::List(graphs)),
        ("cpu", cpu.into()),
        ("gpu_count", (gpus.len() as i32).into()),
        ("gpus", gpu_cards),
        ("ram", (mem.load_pct as i32).into()),
        ("ram_text", used_of_total(ram_used, mem.total_phys).into()),
        ("disk", pct(disk_used, total).into()),
        ("disk_text", used_of_total(disk_used, total).into()),
        ("disk_name", sys_label.as_str().into()),
        ("processes", (r.processes as i32).into()),
        ("uptime", uptime_text(r.uptime_ms).into()),
        ("has_battery", has_battery.into()),
        ("battery", battery.map_or(0, |b| b.percent as i32).into()),
        ("charging", charging.into()),
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
    use crate::ambient::{Battery, ScriptedProbe};

    /// A source over the demo desktop, and the value after its second sample (the first has
    /// no earlier counters to compare with), a second apart.
    fn demo_sample() -> (Sys, Value, Instant) {
        let sys = Sys::new(Arc::new(ScriptedProbe::demo()));
        let t0 = Instant::now();
        sys.sample(t0);
        let t1 = t0 + Duration::from_secs(1);
        let v = sys.sample(t1);
        (sys, v, t1)
    }

    fn num(v: &Value, k: &str) -> f64 {
        v.get(k).and_then(|v| v.as_f64()).unwrap_or_else(|| panic!("no {k}"))
    }

    #[test]
    fn sys_gauges_are_percentages_and_uptime_reads_well() {
        let (_, v, _) = demo_sample();
        let Some(Value::List(g)) = v.get("gauges") else { panic!("no gauges") };
        let seen: Vec<(String, f64, String)> = g.iter().map(|x| (x.get("key").unwrap().to_string(), num(x, "value"), x.get("detail").unwrap().to_string())).collect();
        assert_eq!(
            seen,
            [("cpu".into(), 37.0, "212 processes".into()), ("ram".into(), 59.0, "9.5 / 16 GB".into()), ("gpu".into(), 20.0, "8 GB VRAM".into()), ("disk".into(), 48.0, "220.0 / 460 GB".into())]
        );
        assert_eq!((v.get("uptime"), v.get("disk_name")), (Some(&Value::Str("3d 4h".into())), Some(&Value::Str("C:".into()))));
        assert_eq!(uptime_text(3 * 86_400_000 + 4 * 3_600_000), "3d 4h");
        assert_eq!(uptime_text(5 * 60_000), "0h 5m");
        let (cfg, params) = (std::collections::BTreeMap::new(), std::collections::BTreeMap::new());
        let cx = SourceCx::new(crate::data::InstanceRef::new("", &cfg), &params, crate::data::Tm::new(2026, 9, 21, 1, 12, 0, 0, 0), "Default");
        assert_eq!(Sys::new(Arc::new(ScriptedProbe::demo())).cadence("gauges", &cx), Some(Cadence::Second));
    }

    #[test]
    fn the_first_sample_has_no_cpu_load_and_a_second_inside_800_ms_is_the_first_again() {
        let sys = Sys::new(Arc::new(ScriptedProbe::demo()));
        let t0 = Instant::now();
        let first = sys.sample(t0);
        assert_eq!(num(&first, "cpu"), 0.0, "a load is a change between two readings");
        assert_eq!(sys.sample(t0 + Duration::from_millis(799)), first, "cached, the probe is not read again");
        assert_eq!(num(&sys.sample(t0 + Duration::from_millis(800)), "cpu"), 37.0);
    }

    #[test]
    fn the_big_monitor_gets_every_drive_commit_network_and_history() {
        let (_, v, _) = demo_sample();
        let Some(Value::List(all)) = v.get("gauges_all") else { panic!("no gauges_all") };
        let seen: Vec<(String, f64)> = all.iter().map(|g| (g.get("key").unwrap().to_string(), num(g, "value"))).collect();
        assert_eq!(seen, [("cpu".into(), 37.0), ("ram".into(), 59.0), ("gpu".into(), 20.0), ("commit".into(), 44.0), ("disk".into(), 48.0), ("drive:D:".into(), 71.0)]);
        for k in ["cpu_history", "ram_history"] {
            assert!(matches!(v.get(k), Some(Value::List(h)) if h.len() == 2), "{k}: one point per sample");
        }
        assert_eq!((v.get("net_down"), v.get("net_up")), (Some(&Value::Str("0 KB/s".into())), Some(&Value::Str("0 KB/s".into()))));
    }

    #[test]
    fn network_speed_is_the_change_in_bytes_over_the_seconds_between_samples() {
        let (a, b) = (Reading { net: Some((0, 0)), ..Reading::demo() }, Reading { net: Some((3 * 1_048_576, 2048)), ..Reading::demo() });
        let sys = Sys::new(Arc::new(ScriptedProbe::new([a, b])));
        let t0 = Instant::now();
        sys.sample(t0);
        let v = sys.sample(t0 + Duration::from_secs(1));
        assert_eq!((v.get("net_down"), v.get("net_up")), (Some(&Value::Str("3.0 MB/s".into())), Some(&Value::Str("2 KB/s".into()))));
    }

    #[test]
    fn gpu_adapters_are_percentages_with_history() {
        let (_, v, _) = demo_sample();
        assert_eq!(num(&v, "gpu_count"), 1.0);
        let Some(Value::List(cards)) = v.get("gpus") else { panic!("no gpus") };
        assert_eq!(cards.len(), 1);
        assert_eq!(num(&cards[0], "value"), 20.0);
        assert!(matches!(cards[0].get("history"), Some(Value::List(h)) if h.len() == 2));
    }

    #[test]
    fn a_battery_shows_only_when_there_is_one() {
        let with = Reading { battery: Some(Battery { percent: 80, charging: true }), ..Reading::demo() };
        let v = sample_sys(State::default(), &with, Instant::now()).1;
        assert_eq!((v.get("has_battery"), v.get("charging"), num(&v, "battery")), (Some(&Value::Bool(true)), Some(&Value::Bool(true)), 80.0));
        let Some(Value::List(g)) = v.get("gauges") else { panic!("no gauges") };
        assert_eq!(g.last().unwrap().get("detail"), Some(&Value::Str("Charging".into())));
        let v = sample_sys(State::default(), &Reading::demo(), Instant::now()).1;
        assert_eq!((v.get("has_battery"), v.get("charging"), num(&v, "battery")), (Some(&Value::Bool(false)), Some(&Value::Bool(false)), 0.0));
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
