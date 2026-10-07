//! `scene selfcheck`: is the render deterministic on this machine, and would the check notice
//! if it were not? It is the run-twice checklist of research/18 section 7 as a command.
//!
//! Every run renders the selected scenes in a fresh child process of this very exe
//! (`scene render --dump`, software adapter only, the working directory a new empty folder,
//! output in a temp folder), so nothing is shared between runs but the machine. The runs:
//!
//! - `A` and `B` (and more with `--runs`), `--gap` apart: with the default 1 s it is quick, and
//!   `--gap 61s` is the leaked-wall-clock check (a seconds hand cannot hide behind a gap that is
//!   longer than a minute);
//! - with `--perturb`, `C` with the process on one CPU (`WAYFINDER_SCENE_AFFINITY`, applied by
//!   the child to itself before it has made a thread, so WARP's thread pool is the one the
//!   child gets) and `D` at below-normal priority;
//! - with `--controls`, runs that MUST differ from `A`: another `now`, another `sys.cpu`, the
//!   real wall clock and another scale. A control that changes nothing means the comparison is
//!   blind, and that is a failure.
//!
//! Each run is compared with `A` scene by scene: the dump text (must be equal), the PNG bytes
//! (expected equal on one machine; when not, how many pixels, by how much, and how many fail
//! the diff threshold: the datum for `scenes/wayfinder-render.toml`) and the sidecar (equal but
//! for paths). Every sidecar is asserted: hermetic, drawn on the software adapter, `en-US`,
//! system fonts with one hash for the whole check, and no scene errored or timed out. There
//! are no bundled fonts, so the old "every face is bundled" assertion is the font hash being
//! the same everywhere; path independence and the cross-machine run stay manual, and the
//! report says so.
//!
//! The parent never creates a device. Exit 0: everything equal, every control differed, every
//! assertion held. Exit 1: something differed run to run (or a control or assertion failed).
//! Exit 3: a child could not run or died.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value as Json, json};

use super::pixel::{self, Pair};
use super::{Selection, find, write};

/// Scenes rendered when none are named: the golden set and the checklist's extra cases.
pub const DEFAULT_SET: [&str; 2] = ["golden", "selfcheck"];
/// The scene the `real clock` control renders: a clock that pins nothing (a scene that pins the
/// clock cannot also be told to read the machine's).
pub const LIVE_CLOCK: &str = "selfcheck/checklist/clock-live";
/// The one adapter name a run may report.
const WARP: &str = "Microsoft Basic Render Driver";
/// A child that has not finished by then is killed.
const CHILD_LIMIT: Duration = Duration::from_secs(30 * 60);
/// Read by the child: the CPU mask to put itself on.
pub const AFFINITY_VAR: &str = "WAYFINDER_SCENE_AFFINITY";
/// Read by the child: `below-normal` lowers its priority.
pub const PRIORITY_VAR: &str = "WAYFINDER_SCENE_PRIORITY";

/// What `scene selfcheck` was asked.
#[derive(Clone, Debug, PartialEq)]
pub struct Opts {
    /// Plain runs, `A`, `B`...
    pub runs: usize,
    pub gap: Duration,
    pub perturb: bool,
    pub controls: bool,
    /// Where `summary.txt` and `last.json` go; `<scene root>/.look/selfcheck`.
    pub out: Option<PathBuf>,
    pub quiet: bool,
    /// Keep the runs' PNGs and dumps (in a temp folder, named in the report) instead of deleting them.
    pub keep: bool,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts { runs: 2, gap: Duration::from_secs(1), perturb: false, controls: false, out: None, quiet: false, keep: false }
    }
}

/// `61s`, `61`, `1500ms`, `2m`.
pub fn parse_gap(v: &str) -> Result<Duration, String> {
    let bad = || format!("--gap is a time like 1s, 61s, 500ms or 2m, not `{v}`");
    let v = v.trim();
    let (num, unit) = match v.find(|c: char| !(c.is_ascii_digit() || c == '.')) {
        Some(i) => v.split_at(i),
        None => (v, "s"),
    };
    let n: f64 = num.parse().map_err(|_| bad())?;
    let secs = match unit {
        "s" => n,
        "ms" => n / 1000.0,
        "m" => n * 60.0,
        _ => return Err(bad()),
    };
    if !secs.is_finite() || !(0.0..=3600.0).contains(&secs) {
        return Err(bad());
    }
    Ok(Duration::from_secs_f64(secs))
}

/// What a child does to itself before it starts rendering, from its environment.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Limits {
    pub affinity: Option<usize>,
    pub below_normal: bool,
}

impl Limits {
    pub fn parse(affinity: Option<&str>, priority: Option<&str>) -> Result<Limits, String> {
        let affinity = match affinity.map(str::trim).filter(|a| !a.is_empty()) {
            None => None,
            Some(a) => Some(usize::from_str_radix(a.trim_start_matches("0x"), 16).ok().filter(|m| *m != 0).ok_or_else(|| format!("{AFFINITY_VAR} is a hexadecimal CPU mask like 1, not `{a}`"))?),
        };
        let below_normal = match priority.map(str::trim).filter(|p| !p.is_empty()) {
            None => false,
            Some("below-normal") => true,
            Some(p) => return Err(format!("{PRIORITY_VAR} is `below-normal`, not `{p}`")),
        };
        Ok(Limits { affinity, below_normal })
    }

    pub fn from_env() -> Result<Limits, String> {
        Limits::parse(std::env::var(AFFINITY_VAR).ok().as_deref(), std::env::var(PRIORITY_VAR).ok().as_deref())
    }
}

/// Puts this process (a selfcheck child) on the CPUs and priority its environment asks for,
/// before any thread of its own exists, and says so on stderr (the parent reads it back).
/// Nothing else on the machine is touched.
pub fn apply_limits_from_env() {
    use windows::Win32::System::Threading::{BELOW_NORMAL_PRIORITY_CLASS, GetCurrentProcess, GetProcessAffinityMask, SetPriorityClass, SetProcessAffinityMask};
    let limits = match Limits::from_env() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("wayfinder: {e}");
            return;
        }
    };
    // SAFETY: calls on the pseudo-handle of this process with plain integers
    unsafe {
        let me = GetCurrentProcess();
        if let Some(mask) = limits.affinity {
            let (mut now, mut system) = (0usize, 0usize);
            let _ = GetProcessAffinityMask(me, &mut now, &mut system);
            match SetProcessAffinityMask(me, if system == 0 { mask } else { mask & system }) {
                Ok(()) => {
                    let mut after = 0usize;
                    let _ = GetProcessAffinityMask(me, &mut after, &mut system);
                    eprintln!("selfcheck: affinity mask {now:#x} -> {after:#x} (system {system:#x})");
                }
                Err(e) => eprintln!("selfcheck: could not set the affinity mask: {e}"),
            }
        }
        if limits.below_normal {
            match SetPriorityClass(me, BELOW_NORMAL_PRIORITY_CLASS) {
                Ok(()) => eprintln!("selfcheck: priority class below normal"),
                Err(e) => eprintln!("selfcheck: could not lower the priority: {e}"),
            }
        }
    }
}

/// What a run does differently from a plain one.
#[derive(Clone, Debug, PartialEq)]
enum Kind {
    Plain,
    /// One CPU.
    Affinity,
    BelowNormal,
    /// A change that must show: the label says which, the args make it.
    Control(&'static str),
}

#[derive(Clone, Debug)]
struct Plan {
    label: String,
    kind: Kind,
    args: Vec<String>,
    /// The scenes to render instead of the selection (the control that needs a particular one).
    only: Option<Vec<String>>,
    /// Wait this long (and say so) before the run.
    gap: Duration,
}

/// The runs, in order: plain ones a gap apart, then the perturbed ones, then the controls.
fn plan(o: &Opts, live_clock: bool) -> Vec<Plan> {
    let mut out = Vec::new();
    let letter = |n: usize| ((b'A' + n as u8) as char).to_string();
    for n in 0..o.runs.max(2) {
        out.push(Plan { label: letter(n), kind: Kind::Plain, args: vec![], only: None, gap: if n == 0 { Duration::ZERO } else { o.gap } });
    }
    let next = out.len();
    if o.perturb {
        out.push(Plan { label: letter(next), kind: Kind::Affinity, args: vec![], only: None, gap: Duration::ZERO });
        out.push(Plan { label: letter(next + 1), kind: Kind::BelowNormal, args: vec![], only: None, gap: Duration::ZERO });
    }
    if o.controls {
        // values no scene pins: the golden scenes sit on 2026-09-21 and cpu 42, the pinned world on 2026-01-15 and 37
        let mut controls: Vec<(&'static str, Vec<&str>, Option<Vec<String>>)> = vec![("now", vec!["--env", "now=2031-07-04T21:03:09"], None), ("sys.cpu", vec!["--env", "sys.cpu=77"], None), ("scale", vec!["--scale", "1.5"], None)];
        if live_clock {
            controls.insert(2, ("real clock", vec!["--real", "clock"], Some(vec![LIVE_CLOCK.to_string()])));
        }
        for (what, args, only) in controls {
            out.push(Plan { label: format!("control {what}"), kind: Kind::Control(what), args: args.iter().map(|s| s.to_string()).collect(), only, gap: Duration::ZERO });
        }
    }
    out
}

/// One scene as a run produced it.
#[derive(Clone, Debug)]
struct Shot {
    verdict: String,
    png: Vec<u8>,
    dump: String,
    sidecar: Json,
}

/// A finished run.
#[derive(Debug)]
struct Run {
    label: String,
    kind: Kind,
    secs: f64,
    gap_secs: f64,
    code: i32,
    shots: BTreeMap<String, Shot>,
    /// What the child said about its limits (`selfcheck: ...` lines on its stderr).
    notes: Vec<String>,
}

fn read_run(dir: &Path, label: &str, kind: Kind, secs: f64, gap_secs: f64, code: i32, notes: Vec<String>) -> Result<Run, String> {
    let out = dir.join("out");
    let last = std::fs::read_to_string(out.join("last.json")).map_err(|e| format!("run {label}: no last.json in the child's output ({e}); the child printed: {}", notes.join(" | ")))?;
    let last: Json = serde_json::from_str(&last).map_err(|e| format!("run {label}: last.json: {e}"))?;
    let mut shots = BTreeMap::new();
    for s in last["scenes"].as_array().into_iter().flatten() {
        let id = s["id"].as_str().unwrap_or_default().to_string();
        let verdict = s["verdict"].as_str().unwrap_or("?").to_string();
        let read = |ext: &str| std::fs::read(out.join(format!("{id}.{ext}")));
        let (png, dump, side) = (read("png"), read("dump.txt"), read("env.json"));
        let (Ok(png), Ok(dump), Ok(side)) = (png, dump, side) else {
            // a scene that failed to run has no files: a finding of the run, not of the files
            shots.insert(id, Shot { verdict, png: vec![], dump: String::new(), sidecar: Json::Null });
            continue;
        };
        let sidecar = serde_json::from_slice(&side).map_err(|e| format!("run {label}: {id}.env.json: {e}"))?;
        shots.insert(id, Shot { verdict, png, dump: String::from_utf8_lossy(&dump).into_owned(), sidecar });
    }
    Ok(Run { label: label.to_string(), kind, secs, gap_secs, code, shots, notes })
}

/// A sidecar without what legitimately differs between runs: the paths of content roots.
fn normalised(side: &Json) -> Json {
    let mut s = side.clone();
    if let Some(roots) = s.pointer_mut("/content/roots").and_then(Json::as_array_mut) {
        roots.iter_mut().filter_map(Json::as_object_mut).for_each(|r| {
            r.remove("path");
        });
    }
    s
}

/// The top-level sidecar keys that differ.
fn sidecar_keys_differing(a: &Json, b: &Json) -> Vec<String> {
    let (a, b) = (normalised(a), normalised(b));
    let (Some(x), Some(y)) = (a.as_object(), b.as_object()) else { return vec!["(not objects)".into()] };
    let mut keys: Vec<&String> = x.keys().chain(y.keys()).collect();
    keys.sort();
    keys.dedup();
    keys.into_iter().filter(|k| x.get(*k) != y.get(*k)).cloned().collect()
}

/// How two renders of one scene differ in pixels.
#[derive(Clone, Debug, PartialEq)]
struct PixelDelta {
    /// Set when the sizes differ: nothing else is measured.
    size: Option<((u32, u32), (u32, u32))>,
    total: u64,
    /// Pixels that differ in any channel at all.
    differing: u64,
    /// The largest difference in one channel, 0..255.
    max_channel: u8,
    /// The largest YIQ distance (what `threshold` is compared with).
    max_yiq: f32,
    /// Pixels over the threshold the diff uses.
    failing: u64,
    /// Failing pixels at each of `pixel::SWEEP`'s thresholds.
    sweep: Vec<(f32, u64)>,
}

fn pixel_delta(a: &[u8], b: &[u8], threshold: f32) -> Result<PixelDelta, String> {
    let (ia, ib) = (image::load_from_memory(a).map_err(|e| format!("png: {e}"))?.to_rgba8(), image::load_from_memory(b).map_err(|e| format!("png: {e}"))?.to_rgba8());
    if ia.dimensions() != ib.dimensions() {
        return Ok(PixelDelta { size: Some((ia.dimensions(), ib.dimensions())), total: 0, differing: 0, max_channel: 0, max_yiq: 0.0, failing: 0, sweep: vec![] });
    }
    let mut differing = 0u64;
    let mut max_channel = 0u8;
    for (p, q) in ia.pixels().zip(ib.pixels()) {
        if p != q {
            differing += 1;
            max_channel = max_channel.max(p.0.iter().zip(q.0.iter()).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0));
        }
    }
    let total = u64::from(ia.width()) * u64::from(ia.height());
    match pixel::compare(&ia, &ib, threshold) {
        Pair::Same(c) => Ok(PixelDelta { size: None, total, differing, max_channel, max_yiq: c.max_delta, failing: c.failing, sweep: c.sweep }),
        Pair::Size { base, new } => Ok(PixelDelta { size: Some((base, new)), total: 0, differing: 0, max_channel: 0, max_yiq: 0.0, failing: 0, sweep: vec![] }),
    }
}

/// One scene of one run set against run `A`'s.
#[derive(Clone, Debug)]
struct Cmp {
    id: String,
    run: String,
    dump_equal: bool,
    /// The PNG files are byte for byte the same.
    png_equal: bool,
    sidecar_diff: Vec<String>,
    px: Option<PixelDelta>,
    missing: bool,
}

fn compare_runs(a: &Run, b: &Run, threshold: f32) -> Vec<Cmp> {
    let mut out = Vec::new();
    for (id, x) in &a.shots {
        let Some(y) = b.shots.get(id) else {
            out.push(Cmp { id: id.clone(), run: b.label.clone(), dump_equal: false, png_equal: false, sidecar_diff: vec![], px: None, missing: true });
            continue;
        };
        let png_equal = x.png == y.png;
        let px = if png_equal || x.png.is_empty() || y.png.is_empty() { None } else { pixel_delta(&x.png, &y.png, threshold).ok() };
        out.push(Cmp { id: id.clone(), run: b.label.clone(), dump_equal: x.dump == y.dump, png_equal, sidecar_diff: sidecar_keys_differing(&x.sidecar, &y.sidecar), px, missing: false });
    }
    out
}

impl Cmp {
    fn identical(&self) -> bool {
        !self.missing && self.dump_equal && self.png_equal && self.sidecar_diff.is_empty()
    }

    fn describe(&self) -> String {
        if self.missing {
            return "missing from the run".into();
        }
        let mut parts = Vec::new();
        if !self.dump_equal {
            parts.push("dump differs".to_string());
        }
        if !self.sidecar_diff.is_empty() {
            parts.push(format!("sidecar differs ({})", self.sidecar_diff.join(", ")));
        }
        if !self.png_equal {
            parts.push(match &self.px {
                Some(p) if p.size.is_some() => format!("png size {}x{} vs {}x{}", p.size.unwrap().0.0, p.size.unwrap().0.1, p.size.unwrap().1.0, p.size.unwrap().1.1),
                Some(p) if p.differing == 0 => "png bytes differ, pixels equal".to_string(),
                Some(p) => format!("png: {} of {} px differ ({:.4} %), max channel {}, max yiq {:.2}, {} over the threshold", p.differing, p.total, p.differing as f64 * 100.0 / p.total.max(1) as f64, p.max_channel, p.max_yiq, p.failing),
                None => "png differs".to_string(),
            });
        }
        parts.join("; ")
    }
}

/// A fact the sidecars of a run must show, and the scenes that do not show it.
#[derive(Clone, Debug)]
struct Assertion {
    what: &'static str,
    failed: Vec<String>,
}

/// The sidecar assertions over the plain and perturbed runs.
fn assertions(runs: &[&Run]) -> (Vec<Assertion>, Option<String>) {
    let mut a: Vec<Assertion> = ["hermetic: true", "drawn on the software adapter (Microsoft Basic Render Driver)", "locale en-US", "system fonts, one font hash everywhere", "every scene rendered and ran clean (no error card, no code source timed out)"].into_iter().map(|what| Assertion { what, failed: vec![] }).collect();
    let mut hashes: BTreeMap<String, usize> = BTreeMap::new();
    for r in runs {
        for (id, s) in &r.shots {
            let at = format!("{} in run {}", id, r.label);
            let side = &s.sidecar;
            if side["hermetic"].as_bool() != Some(true) {
                a[0].failed.push(format!("{at}: {}", side["not_hermetic_because"]));
            }
            if side["adapter"]["name"].as_str() != Some(WARP) || side["adapter"]["software"].as_bool() != Some(true) {
                a[1].failed.push(format!("{at}: adapter {}", side["adapter"]["name"]));
            }
            if side["pins"]["locale"].as_str() != Some("en-US") {
                a[2].failed.push(format!("{at}: locale {}", side["pins"]["locale"]));
            }
            match (side["fonts"]["mode"].as_str(), side["fonts"]["hash"].as_str()) {
                (Some("system"), Some(h)) => *hashes.entry(h.to_string()).or_default() += 1,
                _ => a[3].failed.push(format!("{at}: fonts {}", side["fonts"])),
            }
            if matches!(s.verdict.as_str(), "FAILED" | "BAD" | "ERROR" | "EXPECT") || s.png.is_empty() {
                a[4].failed.push(format!("{at}: {}", s.verdict));
            }
        }
    }
    if hashes.len() > 1 {
        a[3].failed.push(format!("{} different font hashes: {}", hashes.len(), hashes.keys().cloned().collect::<Vec<_>>().join(", ")));
    }
    (a, hashes.into_iter().next().map(|(h, _)| h))
}

/// What a control did to the output.
#[derive(Clone, Debug)]
struct Control {
    what: &'static str,
    scenes: usize,
    dump_changed: usize,
    png_changed: usize,
}

impl Control {
    fn ok(&self) -> bool {
        self.dump_changed > 0 && self.png_changed > 0
    }
}

fn control_of(a: &Run, c: &Run, what: &'static str) -> Control {
    let (mut dump_changed, mut png_changed) = (0, 0);
    for (id, y) in &c.shots {
        if let Some(x) = a.shots.get(id) {
            dump_changed += usize::from(x.dump != y.dump);
            png_changed += usize::from(x.png != y.png);
        }
    }
    Control { what, scenes: c.shots.len(), dump_changed, png_changed }
}

/// Starts the child of one run and waits for it. Returns its exit code and what it said on
/// stderr. A child that does not finish within `CHILD_LIMIT` is killed.
fn spawn_run(exe: &Path, root: &Path, all: &[String], dir: &Path, p: &Plan) -> Result<(i32, Vec<String>), String> {
    let patterns = p.only.as_deref().unwrap_or(all);
    let (cwd, out, errfile) = (dir.join("cwd"), dir.join("out"), dir.join("stderr.txt"));
    for d in [&cwd, &out] {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let err = std::fs::File::create(&errfile).map_err(|e| format!("{}: {e}", errfile.display()))?;
    let mut cmd = Command::new(exe);
    // the literal `scene render` verb has no --gpu: it draws on the software adapter and refuses any other
    cmd.arg("scene").arg("render").args(patterns).arg("--dump").arg("--root").arg(root).arg("--out").arg(&out).arg("-q").args(&p.args);
    cmd.current_dir(&cwd).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::from(err));
    cmd.env_remove(AFFINITY_VAR).env_remove(PRIORITY_VAR).env_remove("WAYFINDER_GPU");
    match p.kind {
        Kind::Affinity => {
            cmd.env(AFFINITY_VAR, "1");
        }
        Kind::BelowNormal => {
            cmd.env(PRIORITY_VAR, "below-normal");
        }
        _ => {}
    }
    let mut child = cmd.spawn().map_err(|e| format!("cannot start {}: {e}", exe.display()))?;
    let started = Instant::now();
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code().unwrap_or(-1),
            Ok(None) if started.elapsed() > CHILD_LIMIT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("run {} did not finish in {} minutes and was killed", p.label, CHILD_LIMIT.as_secs() / 60));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(e) => return Err(format!("run {}: {e}", p.label)),
        }
    };
    let notes = std::fs::read_to_string(&errfile).unwrap_or_default().lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect();
    Ok((code, notes))
}

/// The whole report: one table for the scenes, one for the controls, the assertions and a
/// verdict, plus the tolerances the data supports.
struct Report {
    text: String,
    json: Json,
    exit: i32,
}

#[allow(clippy::too_many_arguments)]
fn report(opts: &Opts, runs: &[Run], cmps: &[Cmp], controls: &[Control], asserts: &[Assertion], fonts: Option<&str>, tmp: &Path) -> Report {
    let a = &runs[0];
    let compared: Vec<&Run> = runs.iter().filter(|r| !matches!(r.kind, Kind::Control(_))).collect();
    let mut t = String::new();
    t.push_str(&format!("wayfinder scene selfcheck   {} scenes   {} runs + {} controls   fonts {}   adapter {}\n", a.shots.len(), compared.len(), controls.len(), fonts.unwrap_or("?"), WARP));
    t.push_str("\nruns\n");
    for r in runs {
        let how = match &r.kind {
            Kind::Plain => "fresh process".to_string(),
            Kind::Affinity => "fresh process on one CPU".to_string(),
            Kind::BelowNormal => "fresh process, below-normal priority".to_string(),
            Kind::Control(w) => format!("negative control: {w}"),
        };
        let gap = if r.gap_secs > 0.0 { format!("  after a {:.1} s gap", r.gap_secs) } else { String::new() };
        let notes = if r.notes.is_empty() { String::new() } else { format!("  [{}]", r.notes.join("; ")) };
        t.push_str(&format!("  {:<18}{:>8.1} s   exit {}   {how}{gap}{notes}\n", r.label, r.secs, r.code));
    }
    // per scene against A
    let wide = a.shots.keys().map(String::len).max().unwrap_or(5).max(5) + 2;
    t.push_str(&format!("\nscenes against run A (the other runs: {})\n", compared.iter().skip(1).map(|r| r.label.as_str()).collect::<Vec<_>>().join(", ")));
    t.push_str(&format!("  {:<wide$}{:<7}{:<7}{:<9}{}\n", "scene", "dump", "png", "sidecar", "max delta"));
    for id in a.shots.keys() {
        let mine: Vec<&Cmp> = cmps.iter().filter(|c| &c.id == id && !c.run.starts_with("control")).collect();
        let dump = mine.iter().all(|c| c.dump_equal && !c.missing);
        let png = mine.iter().all(|c| c.png_equal && !c.missing);
        let side = mine.iter().all(|c| c.sidecar_diff.is_empty() && !c.missing);
        let delta = mine.iter().filter_map(|c| c.px.as_ref()).max_by(|x, y| x.differing.cmp(&y.differing));
        let word = |ok: bool| if ok { "same" } else { "DIFF" };
        let d = match delta {
            None => "0".to_string(),
            Some(p) if p.size.is_some() => "size".to_string(),
            Some(p) => format!("{} px, channel {}, yiq {:.2}", p.differing, p.max_channel, p.max_yiq),
        };
        t.push_str(&format!("  {:<wide$}{:<7}{:<7}{:<9}{d}\n", id, word(dump), word(png), word(side)));
    }
    let bad: Vec<&Cmp> = cmps.iter().filter(|c| !c.run.starts_with("control") && !c.identical()).collect();
    if !bad.is_empty() {
        t.push_str("\ndifferences\n");
        for c in &bad {
            t.push_str(&format!("  run {}  {}  {}\n", c.run, c.id, c.describe()));
        }
    }
    t.push_str("\nnegative controls (each must change the output)\n");
    if controls.is_empty() {
        t.push_str("  not run (--controls)\n");
    }
    for c in controls {
        t.push_str(&format!("  {:<18}{} of {} dumps changed, {} PNGs changed   {}\n", c.what, c.dump_changed, c.scenes, c.png_changed, if c.ok() { "differs, as it must" } else { "CHANGED NOTHING: the check is blind" }));
    }
    t.push_str("\nsidecar assertions\n");
    for x in asserts {
        t.push_str(&format!("  {:<80}{}\n", x.what, if x.failed.is_empty() { "ok".to_string() } else { format!("FAILED ({} scenes)", x.failed.len()) }));
        x.failed.iter().take(5).for_each(|f| t.push_str(&format!("      {f}\n")));
    }
    // what the data supports
    let max_failing = cmps.iter().filter(|c| !c.run.starts_with("control")).filter_map(|c| c.px.as_ref()).map(|p| p.failing).max().unwrap_or(0);
    let max_differing = cmps.iter().filter(|c| !c.run.starts_with("control")).filter_map(|c| c.px.as_ref()).map(|p| p.differing).max().unwrap_or(0);
    let max_yiq = cmps.iter().filter(|c| !c.run.starts_with("control")).filter_map(|c| c.px.as_ref()).map(|p| p.max_yiq).fold(0.0f32, f32::max);
    let all_equal = bad.is_empty();
    t.push_str("\nmeasured\n");
    t.push_str(&format!("  PNG bytes: {} of {} scene comparisons identical; most pixels differing in one scene {max_differing}, most over the threshold {max_failing}, largest YIQ distance {max_yiq:.2}\n", cmps.iter().filter(|c| !c.run.starts_with("control") && c.png_equal && !c.missing).count(), cmps.iter().filter(|c| !c.run.starts_with("control")).count()));
    t.push_str(&format!("  suggested tolerances: threshold 0.6, failed_pixels {max_failing} (matching fingerprint). failed_percent (another fingerprint) cannot be measured on one machine: keep the placeholder until a cross-machine run\n"));
    t.push_str(&format!("  gap between the first two runs: {:.1} s{}\n", runs.get(1).map_or(0.0, |r| r.gap_secs), if opts.gap >= Duration::from_secs(61) { " (long enough for the leaked-wall-clock check)" } else { " (shorter than 61 s: this is not the leaked-wall-clock check; use --gap 61s)" }));
    t.push_str("  not run here: path independence (render again from another checkout or CARGO_TARGET_DIR) and the cross-machine comparison (research/18 section 7, steps 7 and 8)\n");
    let controls_ok = controls.iter().all(Control::ok);
    let asserts_ok = asserts.iter().all(|x| x.failed.is_empty());
    let infra_ok = runs.iter().all(|r| r.code == 0 || r.code == 1);
    let exit = if !infra_ok { 3 } else if all_equal && controls_ok && asserts_ok { 0 } else { 1 };
    let verdict = match exit {
        0 => "deterministic: every dump, PNG and sidecar is identical across the runs, every control differed, every assertion held",
        3 => "a child process failed to run",
        _ if !all_equal => "NOT deterministic: something differed run to run (see differences)",
        _ if !controls_ok => "a negative control changed nothing: the comparison is blind",
        _ => "a sidecar assertion failed",
    };
    t.push_str(&format!("\n{verdict}. exit {exit}\n"));
    if opts.keep {
        t.push_str(&format!("runs kept in {}\n", tmp.display()));
    }
    let json = json!({
        "format": 1,
        "exit": exit,
        "verdict": verdict,
        "fonts": fonts,
        "adapter": WARP,
        "gap_secs": opts.gap.as_secs_f64(),
        "runs": runs.iter().map(|r| json!({ "label": r.label, "kind": format!("{:?}", r.kind), "secs": r.secs, "gap_secs": r.gap_secs, "exit": r.code, "scenes": r.shots.len(), "notes": r.notes })).collect::<Vec<_>>(),
        "comparisons": cmps.iter().filter(|c| !c.run.starts_with("control")).map(|c| json!({
            "scene": c.id, "run": c.run, "dump_equal": c.dump_equal, "png_equal": c.png_equal, "sidecar_differs": c.sidecar_diff,
            "pixels": c.px.as_ref().map(|p| json!({ "total": p.total, "differing": p.differing, "max_channel": p.max_channel, "max_yiq": p.max_yiq, "failing": p.failing, "sweep": p.sweep })),
        })).collect::<Vec<_>>(),
        "controls": controls.iter().map(|c| json!({ "control": c.what, "dumps_changed": c.dump_changed, "pngs_changed": c.png_changed, "scenes": c.scenes, "ok": c.ok() })).collect::<Vec<_>>(),
        "assertions": asserts.iter().map(|x| json!({ "what": x.what, "failed": x.failed })).collect::<Vec<_>>(),
        "measured": { "max_failing_pixels": max_failing, "max_differing_pixels": max_differing, "max_yiq": max_yiq, "all_equal": all_equal },
    });
    Report { text: t, json, exit }
}

/// `scene selfcheck`: runs, compares, writes `summary.txt` and `last.json` and returns the exit code.
pub(super) fn selfcheck_cmd(select: &Selection, opts: &Opts) -> i32 {
    let patterns: Vec<String> = if select.patterns.is_empty() && select.widget.is_none() { DEFAULT_SET.iter().map(|s| s.to_string()).collect() } else { select.patterns.clone() };
    if select.widget.is_some() {
        eprintln!("wayfinder: `scene selfcheck` takes scenes or sets, not --widget");
        return 2;
    }
    let sel = Selection { patterns: patterns.clone(), root: select.root.clone(), widget: None };
    let found = find(&sel, &Default::default());
    if !found.problems.is_empty() {
        found.problems.iter().for_each(|p| eprintln!("wayfinder: {p}"));
        eprintln!("wayfinder: nothing was run");
        return 2;
    }
    let live_clock = opts.controls && !find(&Selection { patterns: vec![LIVE_CLOCK.to_string()], root: select.root.clone(), widget: None }, &Default::default()).scenes.is_empty();
    let patterns: Vec<String> = if live_clock && !found.scenes.iter().any(|s| s.id == LIVE_CLOCK) { patterns.into_iter().chain([LIVE_CLOCK.to_string()]).collect() } else { patterns };
    let root = std::path::absolute(select.root.clone().unwrap_or_else(|| PathBuf::from("scenes"))).unwrap_or_else(|_| PathBuf::from("scenes"));
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("wayfinder: cannot find this executable to start the runs: {e}");
            return 3;
        }
    };
    let out = opts.out.clone().unwrap_or_else(|| root.join(".look").join("selfcheck"));
    let tmp = std::env::temp_dir().join(format!("wayfinder-selfcheck-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let say = |s: String| {
        if !opts.quiet {
            println!("{s}");
        }
    };
    say(format!("selfcheck: {} scenes, {} runs{}{}, gap {:.0} s, software adapter only", found.scenes.len(), opts.runs.max(2), if opts.perturb { " + 2 perturbed" } else { "" }, if opts.controls { " + controls" } else { "" }, opts.gap.as_secs_f64()));
    let mut runs: Vec<Run> = Vec::new();
    for p in plan(opts, live_clock) {
        if !p.gap.is_zero() {
            say(format!("selfcheck: waiting {:.0} s before run {}", p.gap.as_secs_f64(), p.label));
            std::thread::sleep(p.gap);
        }
        let dir = tmp.join(p.label.replace(' ', "-"));
        let started = Instant::now();
        let finished = spawn_run(&exe, &root, &patterns, &dir, &p).and_then(|(code, notes)| {
            let secs = started.elapsed().as_secs_f64();
            read_run(&dir, &p.label, p.kind.clone(), secs, p.gap.as_secs_f64(), code, notes)
        });
        match finished {
            Ok(r) => {
                say(format!("selfcheck: run {} done in {:.1} s, exit {}, {} scenes{}", r.label, r.secs, r.code, r.shots.len(), if r.notes.is_empty() { String::new() } else { format!(" ({})", r.notes.join("; ")) }));
                runs.push(r);
            }
            Err(e) => {
                eprintln!("wayfinder: {e}");
                if !opts.keep {
                    let _ = std::fs::remove_dir_all(&tmp);
                }
                return 3;
            }
        }
    }
    let threshold = pixel::Tolerances::load(&root).map(|t| t.for_scene("").threshold).unwrap_or(0.6);
    let (a, rest) = runs.split_first().expect("two runs at least");
    let mut cmps = Vec::new();
    let mut controls = Vec::new();
    for r in rest {
        match &r.kind {
            Kind::Control(w) => controls.push(control_of(a, r, w)),
            _ => cmps.extend(compare_runs(a, r, threshold)),
        }
    }
    let plain: Vec<&Run> = runs.iter().filter(|r| !matches!(r.kind, Kind::Control(_))).collect();
    let (asserts, fonts) = assertions(&plain);
    let rep = report(opts, &runs, &cmps, &controls, &asserts, fonts.as_deref(), &tmp);
    let mut exit = rep.exit;
    for (name, body) in [("summary.txt", rep.text.clone()), ("last.json", serde_json::to_string_pretty(&rep.json).unwrap_or_default() + "\n")] {
        if let Err(e) = write(&out.join(name), &body) {
            eprintln!("wayfinder: cannot write the output: {e}");
            exit = 3;
        }
    }
    // a scene that differed keeps its two PNGs beside the report, so the difference can be looked at
    for c in cmps.iter().filter(|c| !c.run.starts_with("control") && !c.png_equal && !c.missing) {
        if let (Some(x), Some(y)) = (a.shots.get(&c.id), runs.iter().find(|r| r.label == c.run).and_then(|r| r.shots.get(&c.id))) {
            let base = out.join("differing").join(&c.id);
            let _ = write_bytes(&PathBuf::from(format!("{}.A.png", base.display())), &x.png);
            let _ = write_bytes(&PathBuf::from(format!("{}.{}.png", base.display(), c.run)), &y.png);
        }
    }
    if !opts.keep {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    print!("{}{}", if opts.quiet { "" } else { "\n" }, rep.text);
    exit
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(png: &[u8], dump: &str, hermetic: bool) -> Shot {
        let sidecar = json!({
            "hermetic": hermetic, "not_hermetic_because": [], "pins": { "locale": "en-US" },
            "fonts": { "mode": "system", "hash": "abc" }, "adapter": { "name": WARP, "software": true },
            "content": { "roots": [{ "kind": "widget-folder", "path": "C:/tmp/x", "hash": "1" }] },
        });
        Shot { verdict: "ok".into(), png: png.to_vec(), dump: dump.into(), sidecar }
    }

    fn run(label: &str, kind: Kind, shots: Vec<(&str, Shot)>) -> Run {
        Run { label: label.into(), kind, secs: 1.0, gap_secs: 0.0, code: 0, shots: shots.into_iter().map(|(k, v)| (k.to_string(), v)).collect(), notes: vec![] }
    }

    fn png(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba(f(x, y)));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn a_gap_is_seconds_milliseconds_or_minutes() {
        assert_eq!(parse_gap("61s").unwrap(), Duration::from_secs(61));
        assert_eq!(parse_gap("61").unwrap(), Duration::from_secs(61));
        assert_eq!(parse_gap("1500ms").unwrap(), Duration::from_millis(1500));
        assert_eq!(parse_gap("2m").unwrap(), Duration::from_secs(120));
        for bad in ["", "s", "-1s", "1h", "abc", "1e3s"] {
            assert!(parse_gap(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_children_limits_are_read_from_their_environment() {
        assert_eq!(Limits::parse(None, None).unwrap(), Limits::default());
        assert_eq!(Limits::parse(Some("1"), Some("below-normal")).unwrap(), Limits { affinity: Some(1), below_normal: true });
        assert_eq!(Limits::parse(Some("0x3"), None).unwrap().affinity, Some(3));
        assert!(Limits::parse(Some("0"), None).is_err() && Limits::parse(Some("x"), None).is_err() && Limits::parse(None, Some("idle")).is_err());
    }

    #[test]
    fn the_plan_has_plain_runs_a_gap_apart_then_the_perturbed_runs_then_the_controls() {
        let p = plan(&Opts::default(), true);
        assert_eq!(p.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), ["A", "B"]);
        assert_eq!((p[0].gap, p[1].gap), (Duration::ZERO, Duration::from_secs(1)));
        let all = plan(&Opts { runs: 3, gap: Duration::from_secs(61), perturb: true, controls: true, ..Opts::default() }, true);
        let kinds: Vec<&Kind> = all.iter().map(|p| &p.kind).collect();
        assert_eq!(kinds[..5], [&Kind::Plain, &Kind::Plain, &Kind::Plain, &Kind::Affinity, &Kind::BelowNormal]);
        assert_eq!(all.len(), 9);
        assert_eq!(all[7].only.as_deref(), Some(&[LIVE_CLOCK.to_string()][..]));
        assert_eq!(plan(&Opts { controls: true, ..Opts::default() }, false).len(), 5, "without the live clock scene there is no real-clock control");
        assert!(all[5..].iter().all(|p| matches!(p.kind, Kind::Control(_)) && !p.args.is_empty()));
        assert!(all.iter().all(|p| !p.args.iter().any(|a| a.starts_with("--gpu") || a == "--allow-hardware")), "no run names an adapter: the verb draws on WARP only");
    }

    #[test]
    fn equal_runs_are_identical_and_paths_do_not_count() {
        let a = run("A", Kind::Plain, vec![("s", shot(b"x", "d", true))]);
        let mut s = shot(b"x", "d", true);
        s.sidecar["content"]["roots"][0]["path"] = json!("D:/elsewhere");
        let b = run("B", Kind::Plain, vec![("s", s)]);
        let c = compare_runs(&a, &b, 0.6);
        assert!(c[0].identical(), "{}", c[0].describe());
    }

    #[test]
    fn a_pixel_difference_is_counted_and_measured() {
        let base = png(10, 10, |_, _| [10, 20, 30, 255]);
        let one = png(10, 10, |x, y| if (x, y) == (3, 4) { [13, 20, 30, 255] } else { [10, 20, 30, 255] });
        let d = pixel_delta(&base, &one, 0.6).unwrap();
        assert_eq!((d.total, d.differing, d.max_channel, d.size), (100, 1, 3, None));
        assert!(d.max_yiq > 0.0 && d.failing <= 1);
        let a = run("A", Kind::Plain, vec![("s", shot(&base, "d", true))]);
        let b = run("B", Kind::Plain, vec![("s", shot(&one, "d", true))]);
        let c = &compare_runs(&a, &b, 0.6)[0];
        assert!(!c.identical() && c.dump_equal && c.describe().contains("1 of 100 px"), "{}", c.describe());
        let wide = png(11, 10, |_, _| [10, 20, 30, 255]);
        assert!(pixel_delta(&base, &wide, 0.6).unwrap().size.is_some());
    }

    #[test]
    fn a_different_dump_or_sidecar_is_named() {
        let a = run("A", Kind::Plain, vec![("s", shot(b"x", "one", true))]);
        let mut s = shot(b"x", "two", true);
        s.sidecar["windows"] = json!("10.0.1");
        let b = run("B", Kind::Plain, vec![("s", s)]);
        let d = compare_runs(&a, &b, 0.6)[0].describe();
        assert!(d.contains("dump differs") && d.contains("sidecar differs (windows)"), "{d}");
    }

    #[test]
    fn a_control_that_changes_nothing_is_blind() {
        let a = run("A", Kind::Plain, vec![("s", shot(b"x", "d", true))]);
        let same = run("c", Kind::Control("now"), vec![("s", shot(b"x", "d", true))]);
        let moved = run("c", Kind::Control("now"), vec![("s", shot(b"y", "e", true))]);
        assert!(!control_of(&a, &same, "now").ok());
        assert!(control_of(&a, &moved, "now").ok());
    }

    #[test]
    fn sidecar_assertions_name_the_scene_and_run_that_breaks_them() {
        let good = run("A", Kind::Plain, vec![("s", shot(b"x", "d", true))]);
        assert!(assertions(&[&good]).0.iter().all(|a| a.failed.is_empty()));
        let mut bad = shot(b"x", "d", false);
        bad.sidecar["adapter"]["name"] = json!("AMD Radeon");
        bad.sidecar["fonts"]["hash"] = json!("other");
        let bad = run("B", Kind::Plain, vec![("t", bad)]);
        let (a, fonts) = assertions(&[&good, &bad]);
        assert!(a[0].failed[0].contains("t in run B") && !a[1].failed.is_empty() && !a[3].failed.is_empty(), "{a:?}");
        assert!(fonts.is_some());
    }
}
