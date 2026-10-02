//! The baseline verbs: `scene diff` and `scene bless`. Both run the scenes again (a stale
//! `.look/` file is never compared or blessed), compare each dump with the baseline made with
//! the same fonts (`baseline`) and name what changed (`diff`).
//!
//! `diff` is exit 0 only when every scene matches its baseline and nothing is flagged; a scene
//! with no baseline is a finding (`NEW`), so a forgotten bless cannot pass. Baselines made with
//! other fonts are not compared at all (`FONTS`, exit 3): a dump is exact on the machine and
//! font set that made it, and a diff across font sets would be false geometry changes.
//!
//! `bless` needs `--reason`, takes no environment overrides and has no environment variable
//! that updates baselines. It refuses (exit 1, nothing written) when any selected run is not
//! hermetic, shows an error card, fails an `[expect]` or carries an error-level layout flag:
//! fix the scene or loosen its `[expect]` in the same change, visibly. `--new-only` writes only
//! scenes that have no baseline yet.

use std::path::{Path, PathBuf};

use super::baseline::{Against, Store, against};
use super::diff::Diff;
use super::file::Scene;
use super::runner::Overrides;
use super::{Row, Selection, Totals, exit_of, find, judge, matches, run_all, write};
use crate::render::{Failure, env::fnv};

/// What `diff` and `bless` share on the command line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Opts {
    /// Where `.look/` output goes; the scene root's `.look` when none.
    pub out: Option<PathBuf>,
    pub quiet: bool,
    /// The baselines folder instead of `<scene root>/baselines`.
    pub baselines: Option<PathBuf>,
}

/// Lines of one scene's diff the console shows; the whole diff is in `<id>.diff.txt`.
const SHOWN: usize = 40;

fn look_of(s: &Scene, opts: &Opts) -> PathBuf {
    opts.out.clone().unwrap_or_else(|| s.root.join(".look"))
}

fn headline(id: &str, verdict: &str, d: &Diff) -> String {
    format!("{id}  {verdict}  {}", d.counts.summary())
}

/// The diff as the console prints it: capped.
fn console_block(head: &str, d: &Diff, file: &Path) -> String {
    let mut out = format!("{head}\n");
    for l in d.lines.iter().take(SHOWN) {
        out.push_str(l);
        out.push('\n');
    }
    if d.lines.len() > SHOWN {
        out.push_str(&format!("  ... {} more lines in {}\n", d.lines.len() - SHOWN, file.display()));
    }
    out
}

fn diff_text(head: &str, d: &Diff) -> String {
    let mut out = format!("{head}\n");
    d.lines.iter().for_each(|l| {
        out.push_str(l);
        out.push('\n');
    });
    out
}

/// Baselines without a scene under the folder of `store`: the scene was renamed or deleted.
fn orphans(store: &Store, fonts: &str, select: &Selection, found: &[Scene]) -> Vec<String> {
    store.ids(fonts).into_iter().filter(|id| !found.iter().any(|s| &s.id == id)).filter(|id| select.patterns.is_empty() || select.patterns.iter().any(|p| matches(p, id))).collect()
}

/// `scene diff`: each scene's dump against its baseline.
pub(super) fn diff_cmd(select: &Selection, opts: &Opts, changed_only: bool) -> i32 {
    let o = Overrides::default();
    let found = find(select, &o);
    if !found.problems.is_empty() {
        found.problems.iter().for_each(|p| eprintln!("wayfinder: {p}"));
        eprintln!("wayfinder: nothing was run");
        return 2;
    }
    let runs = run_all(&found.scenes, &o);
    let (mut rows, mut dump_hashes) = (Vec::new(), Vec::new());
    let (mut bad, mut infra, mut findings, mut hermetic) = (false, false, false, true);
    let mut fonts = String::new();
    let mut shown: Vec<(String, &'static str, Diff, PathBuf)> = Vec::new();
    let mut files: Vec<(PathBuf, String)> = Vec::new();
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
                let (verdict, detail, finding, keep) = match &ag {
                    Against::Same => (j.verdict, j.detail.clone(), j.finding, j.verdict != "ok"),
                    Against::Changed(d) => {
                        let v = if j.finding {
                            j.verdict
                        } else if d.counts.only_colour() {
                            "COLOUR"
                        } else {
                            "DUMP"
                        };
                        let file = PathBuf::from(format!("{}.diff.txt", base.display()));
                        files.push((file.clone(), diff_text(&headline(&s.id, v, d), d)));
                        shown.push((s.id.clone(), v, Diff { counts: d.counts.clone(), lines: d.lines.clone() }, file));
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
    let colour = shown.iter().filter(|(_, v, ..)| *v == "COLOUR").count();
    if !opts.quiet {
        for (id, verdict, d, file) in &shown {
            if colour > 1 && *verdict == "COLOUR" {
                continue;
            }
            print!("{}", console_block(&headline(id, verdict, d), d, file));
        }
    }
    let code = exit_of(bad, infra, findings);
    let totals = Totals { rows, dump_hashes, hermetic, fonts, reason: None };
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
    print!("{}{console}", if opts.quiet || shown.is_empty() { "" } else { "\n" });
    if wrote { code } else { exit_of(bad, true, findings) }
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

struct Planned {
    id: String,
    store: Store,
    fonts: String,
    faces: u64,
    text: String,
    what: Do,
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
    let reason = reason.replace(['\r', '\n'], " ");
    let runs = run_all(&found.scenes, &o);
    let (mut planned, mut refused): (Vec<Planned>, Vec<(String, String)>) = (Vec::new(), Vec::new());
    let (mut bad, mut hermetic) = (false, true);
    let mut fonts = String::new();
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
                    planned.push(Planned { id: s.id.clone(), store, fonts: fonts.clone(), faces, text, what });
                }
            }
        }
        rows.push(row);
    }
    if bad {
        eprintln!("wayfinder: nothing was blessed");
        return 2;
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
        let totals = Totals { rows, dump_hashes, hermetic, fonts, reason: Some(reason) };
        let (summary, last) = totals.summaries(1, None, false);
        for (name, text) in [("summary.txt", summary), ("last.json", last)] {
            if let Err(e) = write(&dir.join(name), &text) {
                eprintln!("wayfinder: cannot write the output: {e}");
                return 3;
            }
        }
        return 1;
    }
    let (mut new, mut changed, mut same, mut kept) = (0, 0, 0, 0);
    let mut infra = false;
    let mut report = String::new();
    for p in &planned {
        let (verdict, detail, log) = match &p.what {
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
                    report.push_str(&console_block(&headline(&p.id, "BLESSED", d), d, &p.store.path(&p.fonts, &p.id)));
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
        if let Some(r) = rows.iter_mut().find(|r| r.id == p.id) {
            (r.verdict, r.detail) = (verdict, detail);
        }
    }
    let code = if infra { 3 } else { 0 };
    let store_dir = planned.first().map(|p| p.store.fonts_dir(&p.fonts)).unwrap_or_default();
    let line = format!("blessed {} of {} scenes ({new} new, {changed} changed), {same} unchanged, {kept} kept. fonts {fonts}, baselines in {}. reason: {reason}", new + changed, planned.len(), store_dir.display());
    let totals = Totals { rows, dump_hashes, hermetic, fonts, reason: Some(reason) };
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
