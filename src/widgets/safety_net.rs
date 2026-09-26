//! A temporary, local safety net for the modularization refactor (removed once the scene
//! baselines land). Opt-in: the tests are `#[ignore]` and do nothing without `WF_NET`.
//!
//! Record on an untouched commit, compare after every refactor step:
//!
//! ```text
//! WF_NET=record  cargo test --lib safety_net -- --ignored
//! WF_NET=compare cargo test --lib safety_net -- --ignored
//! ```
//!
//! Each dump is a deterministic text listing of a laid-out `ui::Node` tree (key, kind, rect,
//! text, clip). Baselines go to `<target dir>/safety-net/`, untracked and per machine: text
//! metrics depend on the installed fonts. Gaps: `drawer` is built without a Host, and
//! `icon_folder` reads no real folder (its items come from the Instance).

use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::{Inputs, ParamType, Registry, WidgetMeta};
use crate::anim::Anim;
use crate::data::{Cadence, Clock, DataSource, DataSources, Shortcut, Shortcuts, SourceCx, Tm};
use crate::modules::Arrange;
use crate::text::TextEngine;
use crate::theme::{Library, Selection, Theme};
use crate::ui::{self, Env, Frame, Kind, Node};
use crate::value::Value;
use crate::workspace::InstanceCfg;

/// The instant every dump shows.
pub(crate) const TM: Tm = Tm { year: 2026, month: 9, day: 23, dow: 3, hour: 23, minute: 58, second: 58, ms: 0 };

/// The real clock, always at `TM`.
struct FixedClock;

impl DataSource for FixedClock {
    fn name(&self) -> &str {
        "clock"
    }
    fn value(&self, cx: &SourceCx) -> Value {
        Clock.value(&SourceCx { cfg: cx.cfg, params: cx.params, tm: TM, icon_pack: cx.icon_pack })
    }
    fn cadence(&self, f: &str, cx: &SourceCx) -> Option<Cadence> {
        Clock.cadence(f, cx)
    }
}

/// `sys` with the shape of the real one and fixed numbers.
struct ScriptedSys;

impl DataSource for ScriptedSys {
    fn name(&self) -> &str {
        "sys"
    }
    fn cadence(&self, _: &str, _: &SourceCx) -> Option<Cadence> {
        Some(Cadence::Second)
    }
    fn value(&self, _: &SourceCx) -> Value {
        let list = |n: usize, f: &dyn Fn(usize) -> f64| Value::List((0..n).map(|i| Value::Num(f(i))).collect());
        let history = |seed: usize| list(60, &|i| ((i * 7 + seed * 13) % 90) as f64 + 5.0);
        let gauge = |id: &str, key: &str, label: &str, value: f64, detail: &str| Value::obj([("id", id.into()), ("key", key.into()), ("label", label.into()), ("value", value.into()), ("detail", detail.into())]);
        let graph = |key: &str, label: &str, text: &str, values: Value| Value::obj([("key", key.into()), ("label", label.into()), ("text", text.into()), ("values", values)]);
        let (cpu, ram, disk, gpu, commit, drive) = (gauge("cpu", "cpu", "CPU", 23.0, "212 processes"), gauge("ram", "ram", "RAM", 61.0, "9.8 / 16 GB"), gauge("disk", "disk", "C:", 48.0, "220 / 460 GB"), gauge("gpu", "gpu0", "GPU", 12.0, "Scripted GPU"), gauge("commit", "commit", "Commit", 44.0, "14 / 32 GB"), gauge("drive", "drive:D:", "D:", 71.0, "710 / 1000 GB"));
        let battery = gauge("battery", "battery", "Battery", 80.0, "On battery");
        Value::obj([
            ("graphs", Value::List(vec![graph("cpu", "CPU", "23%", history(1)), graph("ram", "Memory", "61%", history(2)), graph("net", "Download", "1.2 MB/s", history(3)), graph("gpu0", "GPU", "12%", history(4))])),
            ("cpu", 23.into()),
            ("gpu_count", 1.into()),
            ("gpus", Value::List(vec![Value::obj([("label", "GPU".into()), ("value", 12.0.into()), ("history", history(4))])])),
            ("ram", 61.into()),
            ("ram_text", "9.8 / 16 GB".into()),
            ("disk", 48.0.into()),
            ("disk_text", "220 / 460 GB".into()),
            ("disk_name", "C:".into()),
            ("processes", 212.into()),
            ("uptime", "3d 4h".into()),
            ("has_battery", true.into()),
            ("battery", 80.into()),
            ("charging", false.into()),
            ("net_down", "1.2 MB/s".into()),
            ("net_up", "48 KB/s".into()),
            ("cpu_history", history(1)),
            ("ram_history", history(2)),
            ("net_history", history(3)),
            ("gauges", Value::List(vec![cpu.clone(), ram.clone(), gpu.clone(), disk.clone(), battery.clone()])),
            ("gauges_all", Value::List(vec![cpu, ram, gpu, commit, disk, drive, battery])),
        ])
    }
}

/// Every source the built-in Widgets read, with fixed values and no I/O. `media` and `audio`
/// are absent: no built-in Widget reads them.
pub(crate) fn scripted_sources() -> DataSources {
    DataSources::new(vec![Box::new(FixedClock), Box::new(ScriptedSys), Box::new(Shortcuts::default())])
}

pub(crate) fn sample_items() -> Vec<Shortcut> {
    ["Notepad", "Calculator", "Explorer", "Terminal", "Paint"].iter().map(|n| Shortcut { name: (*n).into(), target: format!("C:\\Apps\\{n}.exe"), icon: String::new() }).collect()
}

pub(crate) fn theme() -> Theme {
    Theme::compose(&Library::load(Path::new("nope")), &Selection::default(), &[])
}

/// One line per node, depth first: key, kind, rect, clip and scroll flags.
pub(crate) fn dump(root: &Node, frame: &Frame, out: &mut String) {
    fn walk(n: &Node, depth: usize, rects: &BTreeMap<&str, [f32; 4]>, out: &mut String) {
        let kind = match &n.kind {
            Kind::Box => "box".to_string(),
            Kind::Text(t) => format!("text {:?} size {:.2} wrap {}", t.text, t.size, t.wrap),
            Kind::Image(i) => format!("image {:?}", i.id),
            Kind::Shape(_) => "shape".to_string(),
        };
        let rect = rects.get(n.key.as_str()).map_or("no rect".to_string(), |r| format!("[{:.2} {:.2} {:.2} {:.2}]", r[0], r[1], r[2], r[3]));
        let flags = [(n.clip, "clip"), (n.scroll_offset.is_some(), "scroll-y"), (n.scroll_offset_x.is_some(), "scroll-x"), (n.overlay, "overlay")].iter().filter(|f| f.0).map(|f| f.1).collect::<Vec<_>>().join(",");
        let _ = writeln!(out, "{}{} | {kind} | {rect} | {flags}", "  ".repeat(depth), n.key);
        n.children.iter().for_each(|c| walk(c, depth + 1, rects, out));
    }
    let rects: BTreeMap<&str, [f32; 4]> = frame.rects.iter().map(|(k, r)| (k.as_str(), *r)).collect();
    walk(root, 0, &rects, out);
}

/// `dump` of `root` laid out at `size`, under a `## case` header.
pub(crate) fn dump_case(case: &str, root: &Node, size: (f32, f32), text: &mut TextEngine, out: &mut String) {
    let mut anim = Anim::default();
    let mut env = Env { text, anim: &mut anim, hover: None, now: Instant::now(), scale: 1.0 };
    let frame = ui::layout(root, size, &mut env);
    let _ = writeln!(out, "## {case}");
    dump(root, &frame, out);
}

enum Mode {
    Record,
    Compare,
}

fn mode() -> Option<Mode> {
    match std::env::var("WF_NET").as_deref() {
        Ok("record") => Some(Mode::Record),
        Ok("compare") => Some(Mode::Compare),
        Ok(other) => panic!("WF_NET must be `record` or `compare`, not `{other}`"),
        Err(_) => {
            eprintln!("safety net: set WF_NET=record or WF_NET=compare (skipped)");
            None
        }
    }
}

fn net_dir() -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target"));
    target.join("safety-net")
}

/// Records `dump` as `<name>.txt`, or compares against the recorded one, panicking with the
/// `## case` and node key of the first differences.
pub(crate) fn record_or_compare(name: &str, dump: &str) {
    let Some(mode) = mode() else { return };
    let path = net_dir().join(format!("{name}.txt"));
    match mode {
        Mode::Record => {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, dump).unwrap();
            eprintln!("safety net: recorded {} ({} lines)", path.display(), dump.lines().count());
        }
        Mode::Compare => {
            let base = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("no baseline {} ({e}); record on the untouched commit first", path.display()));
            let diffs = diff(&base, dump);
            assert!(diffs.is_empty(), "safety net: `{name}` differs from {}\n{}", path.display(), diffs.join("\n"));
            eprintln!("safety net: `{name}` matches ({} lines)", dump.lines().count());
        }
    }
}

/// Line by line, at most 30 differences, each under the `## case` it is in.
fn diff(base: &str, now: &str) -> Vec<String> {
    let (mut case, mut out) = (String::new(), Vec::new());
    let (mut a, mut b) = (base.lines(), now.lines());
    while out.len() < 30 {
        let (x, y) = (a.next(), b.next());
        if x.is_none() && y.is_none() {
            break;
        }
        if let Some(h) = y.filter(|l| l.starts_with("## ")) {
            case = h.to_string();
        }
        if x != y {
            out.push(format!("{case}\n  was: {}\n  now: {}", x.map_or("(end)", str::trim), y.map_or("(end)", str::trim)));
        }
    }
    out
}

/// Sizes to build at: the default and a 4x4 grid from min to max, as `fits.rs` does.
fn sizes_of(meta: &WidgetMeta) -> Vec<(String, (f32, f32))> {
    let (min, max) = (meta.min_card_size, meta.max_card_size.unwrap_or(meta.default_card_size));
    let mut out = vec![("default".to_string(), meta.default_card_size)];
    for i in 0..4 {
        for j in 0..4 {
            let (tx, ty) = (i as f32 / 3.0, j as f32 / 3.0);
            out.push((format!("{i}/3,{j}/3"), ((min.0 + (max.0 - min.0) * tx).round(), (min.1 + (max.1 - min.1) * ty).round())));
        }
    }
    out
}

#[test]
#[ignore = "opt-in: WF_NET=record|compare, see the module docs"]
fn safety_net_widgets() {
    if mode().is_none() {
        return;
    }
    let reg = Registry::load(Path::new("no-such-dir"));
    let (theme, sources, mut text) = (theme(), scripted_sources(), TextEngine::new());
    let mut ids = reg.ids();
    ids.sort();
    let mut out = String::new();
    for id in ids {
        let Some(Ok(w)) = reg.get(&id) else { continue };
        let meta = w.meta();
        let all_on: BTreeMap<String, Value> = meta.params.iter().filter(|p| p.ty == ParamType::Bool).map(|p| (p.name.clone(), Value::Bool(true))).collect();
        for (variant, extra) in [("defaults", BTreeMap::new()), ("every switch on", all_on)] {
            let mut cfg = InstanceCfg { id: format!("{id}-1"), widget: id.clone(), ..Default::default() };
            meta.seed_params(&mut cfg);
            if cfg.items().is_empty() {
                cfg.set_items(&sample_items());
            }
            extra.iter().for_each(|(k, v)| cfg.set_param(k, v));
            let params = meta.effective_params(&cfg.params_map());
            let cx = SourceCx { cfg: &cfg, params: &params, tm: TM, icon_pack: "Default" };
            let read = |n: &str| sources.value(n, &cx);
            let state = BTreeMap::new();
            // the size grid (the tier the size falls in), then each tier at its own size
            let mut cases: Vec<(String, (f32, f32), Option<&str>)> = sizes_of(meta).into_iter().map(|(n, s)| (format!("at {n} {s:?}"), s, None)).collect();
            cases.extend(meta.tiers.iter().map(|t| (format!("tier {} {:?}", t.name, t.size), t.size, Some(t.name.as_str()))));
            for (label, size, tier) in cases {
                let arrange = tier.map(|t| Arrange { layout: &cfg.layout, tier: Some(t), preview: false });
                let inp = Inputs { params: &params, state: &state, card_size: size, key_prefix: &cfg.id, read_source: &read, arrange };
                let case = format!("{id} {label}, {variant}");
                match w.build(&inp, &theme, &|_| None) {
                    Ok(b) => {
                        dump_case(&case, &b.root, size, &mut text, &mut out);
                        let _ = writeln!(out, "warnings {:?} deps {:?}", b.warnings, b.deps);
                    }
                    Err(e) => {
                        let _ = writeln!(out, "## {case}\nbuild error: {e}");
                    }
                }
            }
        }
    }
    record_or_compare("widgets", &out);
}
