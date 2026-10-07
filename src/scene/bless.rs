//! The baseline verbs: `scene diff` and `scene bless`. Both run the scenes again (a stale
//! `.look/` file is never compared or blessed), compare each dump with the baseline made with
//! the same fonts (`baseline`) and name what changed (`diff`).
//!
//! `diff` is exit 0 only when every scene matches its baseline and nothing is flagged; a scene
//! with no baseline is a finding (`NEW`), so a forgotten bless cannot pass. Baselines made with
//! other fonts are not compared at all (`FONTS`, exit 3): a dump is exact on the machine and
//! font set that made it, and a diff across font sets would be false geometry changes.
//!
//! The pixel tier (`pixel`) joins the dump tier for the scenes tagged `golden`, whose renders
//! are kept as PNG baselines beside the dumps (`--pixels` asks for it of every scene, which
//! then has a baseline only if one exists; `--no-pixels` leaves it out and needs no device).
//! Pixels are drawn on the software adapter only (WARP). `diff` writes `<id>.new.png`,
//! `<id>.diff.png` and `<id>.cmp.png` (baseline | new | diff) for a scene whose pixels changed.
//!
//! `bless` needs `--reason`, takes no environment overrides and has no environment variable
//! that updates baselines. It refuses (exit 1, nothing written) when any selected run is not
//! hermetic, shows an error card, fails an `[expect]` or carries an error-level layout flag:
//! fix the scene or loosen its `[expect]` in the same change, visibly. `--new-only` writes only
//! scenes that have no baseline yet. A golden scene (or any with `--png`) also gets its PNG
//! baseline and the record of the render written, when there is none or the pixels changed.

use std::path::{Path, PathBuf};

use image::RgbaImage;

use super::baseline::{Against, Store, against};
use super::diff::Diff;
use super::file::Scene;
use super::pixel::{self, Fingerprint, Pair, Tolerances};
use super::runner::{Device, Outcome, Overrides, Pixels};
use super::sheet::{self, Tile, Tone};
use super::{Row, Selection, Totals, exit_of, find, judge, matches, run_all_with, write};
use crate::render::{Failure, env::fnv};

/// Whether a scene's pixels are part of a command.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PixelMode {
    /// The scenes tagged `golden` (the default).
    #[default]
    Golden,
    /// Every scene (`--pixels`).
    All,
    /// None: dumps only, no device (`--no-pixels`).
    Off,
}

/// A scene whose render is kept as a PNG baseline.
pub fn is_golden(s: &Scene) -> bool {
    s.tags.iter().any(|t| t == "golden")
}

impl PixelMode {
    pub fn wants(self, s: &Scene) -> bool {
        match self {
            PixelMode::Golden => is_golden(s),
            PixelMode::All => true,
            PixelMode::Off => false,
        }
    }
}

/// What `diff` and `bless` share on the command line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Opts {
    /// Where `.look/` output goes; the scene root's `.look` when none.
    pub out: Option<PathBuf>,
    pub quiet: bool,
    /// The baselines folder instead of `<scene root>/baselines`.
    pub baselines: Option<PathBuf>,
    /// `diff`: which scenes' pixels are compared.
    pub pixels: PixelMode,
    /// `bless`: also keep the PNG of scenes that are not golden.
    pub png: bool,
}

/// Lines of one scene's diff the console shows; the whole diff is in `<id>.diff.txt`.
const SHOWN: usize = 40;

fn look_of(s: &Scene, opts: &Opts) -> PathBuf {
    opts.out.clone().unwrap_or_else(|| s.root.join(".look"))
}

fn headline(id: &str, verdict: &str, summary: &str) -> String {
    format!("{id}  {verdict}  {summary}")
}

/// The diff as the console prints it: capped.
fn console_block(head: &str, lines: &[String], file: &Path) -> String {
    let mut out = format!("{head}\n");
    for l in lines.iter().take(SHOWN) {
        out.push_str(l);
        out.push('\n');
    }
    if lines.len() > SHOWN {
        out.push_str(&format!("  ... {} more lines in {}\n", lines.len() - SHOWN, file.display()));
    }
    out
}

fn diff_text(head: &str, lines: &[String]) -> String {
    let mut out = format!("{head}\n");
    lines.iter().for_each(|l| {
        out.push_str(l);
        out.push('\n');
    });
    out
}

/// Baselines without a scene under the folder of `store`: the scene was renamed or deleted.
fn orphans(store: &Store, fonts: &str, select: &Selection, found: &[Scene]) -> Vec<String> {
    store.ids(fonts).into_iter().filter(|id| !found.iter().any(|s| &s.id == id)).filter(|id| select.patterns.is_empty() || select.patterns.iter().any(|p| matches(p, id))).collect()
}

/// `wayfinder-render.toml` of each scene root met, read once.
#[derive(Default)]
pub(super) struct Tol {
    by_root: Vec<(PathBuf, Tolerances)>,
}

impl Tol {
    pub(super) fn of(&mut self, root: &Path) -> Result<&Tolerances, String> {
        if let Some(i) = self.by_root.iter().position(|(r, _)| r == root) {
            return Ok(&self.by_root[i].1);
        }
        self.by_root.push((root.to_path_buf(), Tolerances::load(root)?));
        Ok(&self.by_root.last().expect("pushed").1)
    }
}

/// How a scene's render stands against its PNG baseline.
pub(super) struct PixelCheck {
    pub verdict: &'static str,
    /// Counts as a finding (exit 1).
    pub finding: bool,
    pub detail: String,
    /// The report: count, fingerprint, regions.
    pub lines: Vec<String>,
    /// `.new.png`, `.diff.png` and `.cmp.png`, by suffix.
    pub images: Vec<(&'static str, RgbaImage)>,
    /// The baseline picture, when the pixels were compared.
    pub base: Option<RgbaImage>,
}

impl PixelCheck {
    fn ok(detail: impl Into<String>) -> PixelCheck {
        PixelCheck { verdict: "ok", finding: false, detail: detail.into(), lines: vec![], images: vec![], base: None }
    }

    /// Nothing was compared.
    pub(super) fn none() -> PixelCheck {
        PixelCheck::ok("")
    }
}

/// Compares a drawn scene with its PNG baseline under the limits of `wayfinder-render.toml`.
/// A golden scene with no baseline is `NEW` (a finding); another scene with none is fine, and
/// its render is kept as `.new.png` so it can be looked at.
pub(super) fn pixel_check(s: &Scene, res: &Outcome, store: &Store, fonts: &str, tol: &Tolerances, device: &mut Device) -> PixelCheck {
    let Pixels::Drawn(drawn) = &res.pixels else { return PixelCheck::ok("") };
    let new = &drawn.image;
    let (base_img, record) = match store.read_png(fonts, &s.id) {
        Ok(Some(b)) => b,
        Ok(None) => {
            let mut c = PixelCheck::ok("");
            c.images.push(("new.png", new.clone()));
            if is_golden(s) {
                (c.verdict, c.finding) = ("NEW", true);
                c.detail = format!("no PNG baseline (wayfinder scene bless {} --reason \"...\")", s.id);
            }
            return c;
        }
        Err(e) => return PixelCheck { verdict: "BASELINE", finding: true, detail: e, ..PixelCheck::ok("") },
    };
    let limits = tol.for_scene(&s.id);
    let c = match pixel::compare(&base_img, new, limits.threshold) {
        Pair::Size { base, new: n } => {
            let line = format!("  pixels: the size changed, {}x{} -> {}x{}: no pixel compare", base.0, base.1, n.0, n.1);
            return PixelCheck { verdict: "SIZE", finding: true, detail: format!("SIZE {}x{} -> {}x{}", base.0, base.1, n.0, n.1), lines: vec![line], images: vec![("new.png", new.clone())], base: Some(base_img) };
        }
        Pair::Same(c) => c,
    };
    if c.failing == 0 {
        return PixelCheck::ok("");
    }
    let (new_fp, base_fp) = (Fingerprint::of(&res.sidecar), record.as_ref().and_then(Fingerprint::of));
    let (same, fp_line) = match (&new_fp, &base_fp) {
        (Some(n), Some(b)) if n.differs(b).is_empty() => (true, format!("same ({})", n.describe())),
        (Some(n), Some(b)) => (false, format!("differs in {} (now {}; baseline {})", n.differs(b).join(", "), n.describe(), b.describe())),
        _ => (false, "unknown: the baseline has no record of its render".to_string()),
    };
    let budget = limits.budget(same, c.total);
    let (regions, more) = pixel::regions(&c);
    let mut lines = pixel::report(&c, &regions, more, &res.dump.nodes, res.dump.header.scale, &fp_line, budget);
    if c.failing <= budget {
        return PixelCheck { verdict: "ok", finding: false, detail: format!("{} px differ, within the budget of {budget}", c.failing), lines, images: vec![], base: None };
    }
    if !same {
        lines.insert(2, format!("  the looser budget of {} % of the pixels applies because the fingerprint differs", limits.failed_percent));
    }
    let diff = pixel::diff_image(new, &c, &regions);
    let mut images = vec![("new.png", new.clone()), ("diff.png", diff.clone())];
    let count = regions.len() + more;
    let head = format!("{}   baseline | new | diff", s.id);
    let tiles = vec![
        Tile { title: "baseline".into(), badge: format!("{}x{}", base_img.width(), base_img.height()), tone: Tone::Ok, image: base_img.clone() },
        Tile { title: "new".into(), badge: format!("{} px differ ({:.2} %)", c.failing, c.percent()), tone: Tone::Bad, image: new.clone() },
        Tile { title: "diff".into(), badge: format!("{count} region{}", if count == 1 { "" } else { "s" }), tone: Tone::Note, image: diff },
    ];
    match device.with_gpu(|gpu| sheet::compose(gpu, &tiles, &head, Some(3), 0.0)) {
        Ok(mut pages) if !pages.is_empty() => images.push(("cmp.png", pages.remove(0).image)),
        Ok(_) => {}
        Err(e) => eprintln!("wayfinder: {}: no cmp.png: {e}", s.id),
    }
    PixelCheck { verdict: "PIXELS", finding: true, detail: format!("{} px differ ({:.2} %), {count} region{}", c.failing, c.percent(), if count == 1 { "" } else { "s" }), lines, images, base: Some(base_img) }
}

/// A scene `diff` has something to show for.
struct Shown {
    verdict: &'static str,
    head: String,
    lines: Vec<String>,
    file: PathBuf,
}

/// `scene diff`: each scene's dump against its baseline, and the golden scenes' pixels
/// against theirs.
pub(super) fn diff_cmd(select: &Selection, opts: &Opts, changed_only: bool) -> i32 {
    let o = Overrides::default();
    let found = find(select, &o);
    if !found.problems.is_empty() {
        found.problems.iter().for_each(|p| eprintln!("wayfinder: {p}"));
        eprintln!("wayfinder: nothing was run");
        return 2;
    }
    let mut tol = Tol::default();
    for s in &found.scenes {
        if let Err(e) = tol.of(&s.root) {
            eprintln!("wayfinder: {e}");
            eprintln!("wayfinder: nothing was run");
            return 2;
        }
    }
    let mode = opts.pixels;
    let mut device = Device::new();
    let runs = run_all_with(&found.scenes, &o, &|s| mode.wants(s), &mut device);
    let (mut rows, mut dump_hashes) = (Vec::new(), Vec::new());
    let (mut bad, mut infra, mut findings, mut hermetic) = (false, false, false, true);
    let mut fonts = String::new();
    let mut adapter: Option<String> = None;
    let mut shown: Vec<Shown> = Vec::new();
    let mut files: Vec<(PathBuf, String)> = Vec::new();
    let mut images: Vec<(PathBuf, RgbaImage)> = Vec::new();
    let mut stores: Vec<Store> = Vec::new();
    let mut other_fonts: Vec<String> = Vec::new();
    for (s, run) in found.scenes.iter().zip(runs) {
        let mut row = Row { id: s.id.clone(), verdict: "ok", detail: String::new(), scene_hash: s.source_hash.clone(), dump_hash: None };
        match run {
            Err(Failure::Bad(e)) => {
                eprintln!("wayfinder: {}: {e}", s.id);
                (row.verdict, row.detail, bad) = ("BAD", e, true);
            }
            Err(Failure::Run(e)) => {
                eprintln!("wayfinder: {}: {e}", s.id);
                (row.verdict, row.detail, findings) = ("FAILED", e, true);
            }
            Ok(res) => {
                res.notes.iter().for_each(|n| eprintln!("{}: {n}", s.id));
                let text = res.dump.to_text();
                let hash = fnv([text.as_bytes()]);
                hermetic &= res.dump.header.hermetic;
                fonts = res.dump.header.fonts.clone();
                row.dump_hash = Some(hash.clone());
                dump_hashes.push(hash);
                let store = Store::new(&s.root, opts.baselines.as_deref());
                let j = judge(s, &res);
                let ag = against(&store, &fonts, &s.id, &text);
                let base = look_of(s, opts).join(&s.id);
                let with_judged = |what: String| if j.finding { format!("{}; {what}", j.detail) } else { what };
                let mut dump_lines: Vec<String> = Vec::new();
                let (mut verdict, mut detail, mut finding, mut keep) = match &ag {
                    Against::Same => (j.verdict, j.detail.clone(), j.finding, j.verdict != "ok"),
                    Against::Changed(d) => {
                        let v = if j.finding {
                            j.verdict
                        } else if d.counts.only_colour() {
                            "COLOUR"
                        } else {
                            "DUMP"
                        };
                        dump_lines = d.lines.clone();
                        (v, with_judged(d.counts.summary()), true, true)
                    }
                    Against::New => (if j.finding { j.verdict } else { "NEW" }, with_judged(format!("no baseline (wayfinder scene bless {} --reason \"...\")", s.id)), true, true),
                    Against::Fonts(others) => {
                        infra = true;
                        other_fonts.extend(others.iter().cloned());
                        ("FONTS", format!("baseline made with fonts {}, this machine has {fonts}: not compared", others.join(", ")), false, true)
                    }
                    Against::Broken(e) => ("BASELINE", e.clone(), true, true),
                };
                // the pixel tier, where there are pixels and baselines made with these fonts
                let mut pixel_lines: Vec<String> = Vec::new();
                match &res.pixels {
                    Pixels::Off => {}
                    Pixels::Failed(e) => {
                        eprintln!("wayfinder: {}: no pixels: {e}", s.id);
                        infra = true;
                    }
                    Pixels::Drawn(d) => {
                        adapter = Some(d.adapter.name.clone());
                        if !matches!(ag, Against::Fonts(_)) {
                            let t = tol.of(&s.root).expect("read above").clone();
                            let px = pixel_check(s, &res, &store, &fonts, &t, &mut device);
                            for (suffix, img) in px.images {
                                images.push((PathBuf::from(format!("{}.{suffix}", base.display())), img));
                            }
                            pixel_lines = px.lines;
                            if px.finding {
                                finding = true;
                                keep = true;
                                if verdict == "ok" {
                                    verdict = px.verdict;
                                    detail = px.detail;
                                } else {
                                    detail = format!("{detail}; pixels: {}", px.detail);
                                }
                            } else if !px.detail.is_empty() && verdict == "ok" {
                                detail = px.detail;
                            }
                        }
                    }
                }
                if !dump_lines.is_empty() || !pixel_lines.is_empty() {
                    let file = PathBuf::from(format!("{}.diff.txt", base.display()));
                    let summary = match &ag {
                        Against::Changed(d) => d.counts.summary(),
                        _ => detail.clone(),
                    };
                    let head = headline(&s.id, verdict, &summary);
                    let mut lines = dump_lines;
                    lines.extend(pixel_lines);
                    files.push((file.clone(), diff_text(&head, &lines)));
                    shown.push(Shown { verdict, head, lines, file });
                }
                (row.verdict, row.detail) = (verdict, detail);
                findings |= finding;
                if keep {
                    files.push((PathBuf::from(format!("{}.dump.txt", base.display())), text));
                    files.push((PathBuf::from(format!("{}.env.json", base.display())), serde_json::to_string_pretty(&res.sidecar).unwrap_or_default() + "\n"));
                }
                if !stores.iter().any(|st| st == &store) {
                    stores.push(store);
                }
            }
        }
        rows.push(row);
    }
    if !other_fonts.is_empty() {
        other_fonts.sort();
        other_fonts.dedup();
        eprintln!(
            "wayfinder: the baselines were made with fonts {}, this machine's are {fonts}. A dump is exact only on the machine and font set that made it, so nothing was compared (it would report text metrics as geometry changes). Compare on the machine that made them, or `wayfinder scene bless <set> --reason \"...\"` records baselines for these fonts beside the others.",
            other_fonts.join(", ")
        );
    }
    for store in &stores {
        for id in orphans(store, &fonts, select, &found.scenes) {
            eprintln!("note: baseline `{id}` in {} has no scene (renamed or removed?)", store.fonts_dir(&fonts).display());
        }
    }
    let colour = shown.iter().filter(|s| s.verdict == "COLOUR").count();
    if !opts.quiet {
        for s in &shown {
            if colour > 1 && s.verdict == "COLOUR" {
                continue;
            }
            print!("{}", console_block(&s.head, &s.lines, &s.file));
        }
    }
    let code = exit_of(bad, infra, findings);
    let totals = Totals { rows, dump_hashes, hermetic, fonts, reason: None, adapter };
    let (full, last) = totals.summaries(code, None, false);
    let (console, _) = totals.summaries(code, Some(if changed_only { 0 } else { 20 }), true);
    let dir = opts.out.clone().unwrap_or_else(|| found.scenes.first().map(|s| s.root.join(".look")).unwrap_or_default());
    files.push((dir.join("summary.txt"), full));
    files.push((dir.join("last.json"), last));
    let mut wrote = true;
    for (path, text) in files {
        if let Err(e) = write(&path, &text) {
            eprintln!("wayfinder: cannot write the output: {e}");
            wrote = false;
        }
    }
    for (path, img) in images {
        if let Err(e) = write_png(&path, &img) {
            eprintln!("wayfinder: cannot write the output: {e}");
            wrote = false;
        }
    }
    print!("{}{console}", if opts.quiet || shown.is_empty() { "" } else { "\n" });
    if wrote { code } else { exit_of(bad, true, findings) }
}

/// Saves `img` as a PNG, making its folder.
pub(super) fn write_png(path: &Path, img: &RgbaImage) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    img.save(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// What a bless will do with one scene.
enum Do {
    /// No baseline for these fonts yet.
    New,
    Update(Diff),
    Same,
    /// `--new-only` and a baseline exists.
    Keep,
}

/// The PNG baseline a bless will write, and why.
struct PngPlan {
    image: RgbaImage,
    record: serde_json::Value,
    /// "new", or "N px changed".
    what: String,
}

struct Planned {
    id: String,
    store: Store,
    fonts: String,
    faces: u64,
    text: String,
    what: Do,
    png: Option<PngPlan>,
}

/// `scene bless`: re-run, refuse anything unsafe, write the baselines.
pub(super) fn bless_cmd(select: &Selection, opts: &Opts, reason: &str, new_only: bool) -> i32 {
    let o = Overrides::default();
    let found = find(select, &o);
    if !found.problems.is_empty() {
        found.problems.iter().for_each(|p| eprintln!("wayfinder: {p}"));
        eprintln!("wayfinder: nothing was run");
        return 2;
    }
    let mut tol = Tol::default();
    for s in &found.scenes {
        if let Err(e) = tol.of(&s.root) {
            eprintln!("wayfinder: {e}");
            eprintln!("wayfinder: nothing was run");
            return 2;
        }
    }
    let reason = reason.replace(['\r', '\n'], " ");
    let png = opts.png;
    let mut device = Device::new();
    let runs = run_all_with(&found.scenes, &o, &|s| is_golden(s) || png, &mut device);
    let (mut planned, mut refused): (Vec<Planned>, Vec<(String, String)>) = (Vec::new(), Vec::new());
    let (mut bad, mut hermetic, mut no_device) = (false, true, false);
    let mut fonts = String::new();
    let mut adapter: Option<String> = None;
    let mut rows: Vec<Row> = Vec::new();
    let mut dump_hashes = Vec::new();
    for (s, run) in found.scenes.iter().zip(runs) {
        let mut row = Row { id: s.id.clone(), verdict: "ok", detail: String::new(), scene_hash: s.source_hash.clone(), dump_hash: None };
        match run {
            Err(Failure::Bad(e)) => {
                eprintln!("wayfinder: {}: {e}", s.id);
                (row.verdict, row.detail, bad) = ("BAD", e, true);
            }
            Err(Failure::Run(e)) => {
                (row.verdict, row.detail) = ("REFUSED", format!("the run failed: {e}"));
                refused.push((s.id.clone(), format!("the run failed: {e}")));
            }
            Ok(res) => {
                res.notes.iter().for_each(|n| eprintln!("{}: {n}", s.id));
                let text = res.dump.to_text();
                let hash = fnv([text.as_bytes()]);
                (row.dump_hash, hermetic) = (Some(hash.clone()), hermetic && res.dump.header.hermetic);
                dump_hashes.push(hash);
                fonts = res.dump.header.fonts.clone();
                let j = judge(s, &res);
                let mut why: Vec<String> = Vec::new();
                if !res.dump.header.hermetic {
                    why.push(format!("not hermetic ({})", res.dump.header.not_hermetic_because.join("; ")));
                }
                why.extend(j.lines.iter().filter(|(finding, _)| *finding).map(|(_, l)| l.clone()));
                if !why.is_empty() {
                    (row.verdict, row.detail) = ("REFUSED", why.join("; "));
                    refused.push((s.id.clone(), why.join("; ")));
                } else {
                    let store = Store::new(&s.root, opts.baselines.as_deref());
                    let what = match against(&store, &fonts, &s.id, &text) {
                        Against::Same => Do::Same,
                        Against::Changed(_) if new_only => Do::Keep,
                        Against::Changed(d) => Do::Update(d),
                        Against::Broken(_) => Do::Update(Diff::default()),
                        Against::New | Against::Fonts(_) => Do::New,
                    };
                    let faces = res.sidecar["fonts"]["faces"].as_u64().unwrap_or(0);
                    let mut png_plan = None;
                    match &res.pixels {
                        Pixels::Off => {}
                        Pixels::Failed(e) => {
                            eprintln!("wayfinder: {}: no pixels: {e}", s.id);
                            no_device = true;
                        }
                        Pixels::Drawn(d) => {
                            adapter = Some(d.adapter.name.clone());
                            let limits = tol.of(&s.root).expect("read above").for_scene(&s.id);
                            let what = match store.read_png(&fonts, &s.id) {
                                Ok(None) => Some("new".to_string()),
                                Ok(Some(_)) if new_only => None,
                                Ok(Some((b, _))) => match pixel::compare(&b, &d.image, limits.threshold) {
                                    Pair::Size { .. } => Some("size changed".to_string()),
                                    Pair::Same(c) if c.failing == 0 && b == d.image => None,
                                    Pair::Same(c) if c.failing == 0 => Some("changed below the threshold".to_string()),
                                    Pair::Same(c) => Some(format!("{} px changed", c.failing)),
                                },
                                Err(_) => Some("replaced an unreadable baseline".to_string()),
                            };
                            png_plan = what.map(|what| PngPlan { image: d.image.clone(), record: res.sidecar.clone(), what });
                        }
                    }
                    planned.push(Planned { id: s.id.clone(), store, fonts: fonts.clone(), faces, text, what, png: png_plan });
                }
            }
        }
        rows.push(row);
    }
    if bad {
        eprintln!("wayfinder: nothing was blessed");
        return 2;
    }
    if no_device {
        eprintln!("wayfinder: nothing was blessed: the software adapter is needed to render golden scenes (see the message above)");
        return 3;
    }
    let dir = opts.out.clone().unwrap_or_else(|| found.scenes.first().map(|s| s.root.join(".look")).unwrap_or_default());
    if !refused.is_empty() {
        for (id, why) in &refused {
            eprintln!("wayfinder: refused {id}: {why}");
        }
        eprintln!("wayfinder: nothing was blessed ({} of {} scenes refused; fix the scene, or loosen its [expect] in the same change)", refused.len(), found.scenes.len());
        for r in rows.iter_mut().filter(|r| r.verdict == "ok") {
            r.detail = "not written: another scene was refused".into();
        }
        let totals = Totals { rows, dump_hashes, hermetic, fonts, reason: Some(reason), adapter };
        let (summary, last) = totals.summaries(1, None, false);
        for (name, text) in [("summary.txt", summary), ("last.json", last)] {
            if let Err(e) = write(&dir.join(name), &text) {
                eprintln!("wayfinder: cannot write the output: {e}");
                return 3;
            }
        }
        return 1;
    }
    let (mut new, mut changed, mut same, mut kept, mut pngs) = (0, 0, 0, 0, 0);
    let mut infra = false;
    let mut report = String::new();
    for p in &planned {
        let (verdict, mut detail, log) = match &p.what {
            Do::Same => {
                same += 1;
                ("ok", "unchanged".to_string(), None)
            }
            Do::Keep => {
                kept += 1;
                ("ok", "baseline kept (--new-only)".to_string(), None)
            }
            Do::New => {
                new += 1;
                ("ok", "blessed, new".to_string(), Some("new".to_string()))
            }
            Do::Update(d) => {
                changed += 1;
                let sum = if d.is_empty() { "replaced an unreadable baseline".to_string() } else { d.counts.summary() };
                if !d.is_empty() {
                    report.push_str(&console_block(&headline(&p.id, "BLESSED", &d.counts.summary()), &d.lines, &p.store.path(&p.fonts, &p.id)));
                }
                ("ok", format!("blessed, {sum}"), Some(sum))
            }
        };
        if let Some(what) = log {
            let done = p.store.write(&p.fonts, p.faces, &p.id, &p.text).and_then(|()| p.store.log(&p.fonts, &format!("{}\t{reason}\t{what}", p.id)));
            if let Err(e) = done {
                eprintln!("wayfinder: cannot write the baseline: {e}");
                infra = true;
            }
        }
        if let Some(png) = &p.png {
            pngs += 1;
            let done = p.store.write_png(&p.fonts, &p.id, &png.image, &png.record).and_then(|()| p.store.log(&p.fonts, &format!("{}\t{reason}\tpng {}", p.id, png.what)));
            if let Err(e) = done {
                eprintln!("wayfinder: cannot write the PNG baseline: {e}");
                infra = true;
            }
            report.push_str(&format!("{}  PNG  {}\n", p.id, png.what));
            detail = format!("{detail}; png {}", png.what);
        }
        if let Some(r) = rows.iter_mut().find(|r| r.id == p.id) {
            (r.verdict, r.detail) = (verdict, detail);
        }
    }
    let code = if infra { 3 } else { 0 };
    let store_dir = planned.first().map(|p| p.store.fonts_dir(&p.fonts)).unwrap_or_default();
    let line = format!("blessed {} of {} scenes ({new} new, {changed} changed), {same} unchanged, {kept} kept; {pngs} PNG baseline{} written. fonts {fonts}, baselines in {}. reason: {reason}", new + changed, planned.len(), if pngs == 1 { "" } else { "s" }, store_dir.display());
    let totals = Totals { rows, dump_hashes, hermetic, fonts, reason: Some(reason), adapter };
    let (summary, last) = totals.summaries(code, None, false);
    for (name, text) in [("summary.txt", summary), ("last.json", last)] {
        if let Err(e) = write(&dir.join(name), &text) {
            eprintln!("wayfinder: cannot write the output: {e}");
            return 3;
        }
    }
    if !opts.quiet {
        print!("{report}");
    }
    println!("{line}");
    code
}
