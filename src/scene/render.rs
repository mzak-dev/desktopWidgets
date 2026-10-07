//! The verbs that draw: `scene render` (a PNG and its record per scene, and with `--dump` the
//! text dump, with `--sheet` a contact sheet) and `scene sheet` (one contact sheet, each tile
//! badged with what is the matter with it).
//!
//! Everything is drawn on the software adapter (WARP) and nothing else: the verbs have no
//! `--gpu`, `runner::Device` asks for the software adapter by name and refuses any other, and a
//! machine with no software adapter is exit 3 (infrastructure, not a finding). The record beside
//! each PNG (`<id>.env.json`) is the one `render` writes for `--render-widget`, with the adapter.

use std::path::{Path, PathBuf};

use super::baseline::{Against, Store, against};
use super::bless::{PixelCheck, Tol, pixel_check, write_png};
use super::runner::{Device, Outcome, Overrides, Pixels};
use super::sheet::{self, Tile, Tone};
use super::{Row, Selection, Totals, exit_of, find, judge, run_all_with, write};
use crate::render::{Failure, env::fnv};

fn size_of(res: &Outcome) -> String {
    match &res.pixels {
        Pixels::Drawn(d) => format!("{}x{}", d.image.width(), d.image.height()),
        _ => String::new(),
    }
}

/// `scene render`: the PNG and record of each scene under `.look/`.
pub(super) fn render_cmd(select: &Selection, out: Option<PathBuf>, quiet: bool, o: &Overrides, dump: bool, with_sheet: bool) -> i32 {
    let found = find(select, o);
    if !found.problems.is_empty() {
        found.problems.iter().for_each(|p| eprintln!("wayfinder: {p}"));
        eprintln!("wayfinder: nothing was run");
        return 2;
    }
    let mut device = Device::new();
    let runs = run_all_with(&found.scenes, o, &|_| true, &mut device);
    let (mut rows, mut dump_hashes, mut tiles) = (Vec::new(), Vec::new(), Vec::new());
    let (mut bad, mut infra, mut findings, mut hermetic) = (false, false, false, true);
    let mut fonts = String::new();
    let mut adapter: Option<String> = None;
    let look = |s: &super::file::Scene| out.clone().unwrap_or_else(|| s.root.join(".look"));
    let first_root = found.scenes.first().map(|s| look(s)).unwrap_or_default();
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
                row.dump_hash = Some(hash);
                dump_hashes.push(row.dump_hash.clone().unwrap_or_default());
                let j = judge(s, &res);
                (row.verdict, row.detail, findings) = (j.verdict, j.detail.clone(), findings || j.finding);
                let base = look(s).join(&s.id);
                let mut files = vec![(PathBuf::from(format!("{}.env.json", base.display())), serde_json::to_string_pretty(&res.sidecar).unwrap_or_default() + "\n")];
                if dump {
                    files.push((PathBuf::from(format!("{}.dump.txt", base.display())), text));
                }
                match &res.pixels {
                    Pixels::Drawn(d) => {
                        adapter = Some(d.adapter.name.clone());
                        let png = PathBuf::from(format!("{}.png", base.display()));
                        match write_png(&png, &d.image) {
                            Ok(()) => {
                                if !quiet {
                                    println!("{}  {}  {}", s.id, size_of(&res), png.display());
                                }
                            }
                            Err(e) => {
                                eprintln!("wayfinder: cannot write the output: {e}");
                                infra = true;
                            }
                        }
                        let (badge, tone) = badge_of(&j.verdict, &j.lines.len());
                        tiles.push(Tile { title: s.id.clone(), badge: format!("{badge}  {}  @{}", size_of(&res), res.dump.header.scale), tone, image: d.image.clone() });
                    }
                    Pixels::Failed(e) => {
                        eprintln!("wayfinder: {}: no pixels: {e}", s.id);
                        infra = true;
                    }
                    Pixels::Off => {}
                }
                for (path, body) in files {
                    if let Err(e) = write(&path, &body) {
                        eprintln!("wayfinder: cannot write the output: {e}");
                        infra = true;
                    }
                }
            }
        }
        rows.push(row);
    }
    let mut code = exit_of(bad, infra, findings);
    if with_sheet && !tiles.is_empty() {
        let header = format!("wayfinder scene render   {} scenes   fonts {fonts}", tiles.len());
        match write_sheet(&mut device, &tiles, &header, None, &first_root.join("sheet.png"), quiet) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("wayfinder: cannot make the sheet: {e}");
                code = exit_of(bad, true, findings);
            }
        }
    }
    let totals = Totals { rows, dump_hashes, hermetic, fonts, reason: None, adapter };
    let (summary, last) = totals.summaries(code, Some(20), false);
    for (name, text) in [("summary.txt", summary.clone()), ("last.json", last)] {
        if let Err(e) = write(&first_root.join(name), &text) {
            eprintln!("wayfinder: cannot write the output: {e}");
            return exit_of(bad, true, findings);
        }
    }
    print!("{}{summary}", if quiet { "" } else { "\n" });
    code
}

/// The badge of a judged scene: what is wrong, or `ok`.
fn badge_of(verdict: &str, lines: &usize) -> (String, Tone) {
    match verdict {
        "ok" => ("ok".into(), Tone::Ok),
        "WARN" => (format!("WARN {lines}"), Tone::Note),
        "FLAGS" => (format!("FLAGS {lines}"), Tone::Bad),
        other => (other.to_string(), Tone::Bad),
    }
}

/// Draws the tiles and writes the pages (`sheet.png`, `sheet-2.png`...).
fn write_sheet(device: &mut Device, tiles: &[Tile], header: &str, cols: Option<usize>, path: &Path, quiet: bool) -> Result<(), String> {
    let pages = device.with_gpu(|gpu| sheet::compose(gpu, tiles, header, cols, sheet::MIN_FACTOR))?;
    for (n, p) in pages.iter().enumerate() {
        let file = sheet::page_name(path, n + 1);
        write_png(&file, &p.image)?;
        if !quiet {
            println!("sheet {}  {}x{}  x{:.2}  {} tile{}", file.display(), p.image.width(), p.image.height(), p.factor, p.tiles.len(), if p.tiles.len() == 1 { "" } else { "s" });
        }
    }
    Ok(())
}

/// What a sheet says of one scene.
struct Eval {
    /// The summary verdict.
    verdict: &'static str,
    tile: Tile,
    /// Baseline, new and diff, when the pixels changed and `--diff` wants them.
    triptych: Option<[Tile; 3]>,
}

/// `scene sheet`: every selected scene on one contact sheet. A tile says `ok`, `NEW` (no
/// baseline), `DUMP` (the layout changed), `PIXELS` (the picture changed), `FLAGS n` or
/// `ERROR`; the tiles that need a look come first. With `--diff` a scene whose pixels changed
/// is three tiles, baseline | new | diff. Baselines are only consulted when no environment
/// option changed the scenes.
pub(super) fn sheet_cmd(select: &Selection, out: Option<PathBuf>, quiet: bool, o: &Overrides, cols: Option<usize>, diff: bool) -> i32 {
    let found = find(select, o);
    if !found.problems.is_empty() {
        found.problems.iter().for_each(|p| eprintln!("wayfinder: {p}"));
        eprintln!("wayfinder: nothing was run");
        return 2;
    }
    let mut tol = Tol::default();
    let judged_against_baselines = o == &Overrides::default();
    if judged_against_baselines {
        for s in &found.scenes {
            if let Err(e) = tol.of(&s.root) {
                eprintln!("wayfinder: {e}");
                eprintln!("wayfinder: nothing was run");
                return 2;
            }
        }
    }
    let mut device = Device::new();
    let runs = run_all_with(&found.scenes, o, &|_| true, &mut device);
    let (mut rows, mut dump_hashes, mut evals) = (Vec::new(), Vec::new(), Vec::new());
    let (mut bad, mut infra, mut findings, mut hermetic) = (false, false, false, true);
    let mut fonts = String::new();
    let mut adapter: Option<String> = None;
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
                row.dump_hash = Some(fnv([text.as_bytes()]));
                dump_hashes.push(row.dump_hash.clone().unwrap_or_default());
                hermetic &= res.dump.header.hermetic;
                fonts = res.dump.header.fonts.clone();
                let j = judge(s, &res);
                let Pixels::Drawn(d) = &res.pixels else {
                    if let Pixels::Failed(e) = &res.pixels {
                        eprintln!("wayfinder: {}: no pixels: {e}", s.id);
                    }
                    infra = true;
                    rows.push(row);
                    continue;
                };
                adapter = Some(d.adapter.name.clone());
                let (mut verdict, mut detail, finding) = (j.verdict, j.detail.clone(), j.finding);
                let mut triptych = None;
                if judged_against_baselines {
                    let store = Store::new(&s.root, None);
                    let ag = against(&store, &fonts, &s.id, &text);
                    let t = tol.of(&s.root).expect("read above").clone();
                    let px: PixelCheck = if matches!(ag, Against::Fonts(_)) { PixelCheck::none() } else { pixel_check(s, &res, &store, &fonts, &t, &mut device) };
                    let dump_note = match &ag {
                        Against::Changed(dd) => Some(("DUMP", dd.counts.summary())),
                        Against::New => Some(("NEW", "no baseline".to_string())),
                        Against::Fonts(f) => Some(("FONTS", format!("baselines were made with fonts {}", f.join(", ")))),
                        Against::Broken(e) => Some(("BASELINE", e.clone())),
                        Against::Same => None,
                    };
                    if verdict == "ok" {
                        if px.finding && px.verdict != "NEW" {
                            (verdict, detail) = (px.verdict, px.detail.clone());
                        } else if let Some((v, dd)) = dump_note {
                            (verdict, detail) = (v, dd);
                        } else if px.finding {
                            (verdict, detail) = (px.verdict, px.detail.clone());
                        }
                    }
                    if diff && px.verdict == "PIXELS" {
                        let pick = |name: &str| px.images.iter().find(|(n, _)| *n == name).map(|(_, i)| i.clone());
                        if let (Some(b), Some(n), Some(df)) = (px.base.clone(), pick("new.png"), pick("diff.png")) {
                            triptych = Some([
                                Tile { title: format!("{}  baseline", s.id), badge: format!("{}x{}", b.width(), b.height()), tone: Tone::Ok, image: b },
                                Tile { title: format!("{}  new", s.id), badge: px.detail.clone(), tone: Tone::Bad, image: n },
                                Tile { title: format!("{}  diff", s.id), badge: "magenta = differs".into(), tone: Tone::Note, image: df },
                            ]);
                        }
                    }
                }
                findings |= finding;
                (row.verdict, row.detail) = (verdict, detail.clone());
                let (badge, tone) = match verdict {
                    "ok" => ("ok".to_string(), Tone::Ok),
                    "FLAGS" => (format!("FLAGS {}", j.lines.iter().filter(|(f, _)| *f).count()), Tone::Bad),
                    "WARN" => (format!("WARN {}", j.lines.len()), Tone::Note),
                    "NEW" | "DUMP" | "COLOUR" | "FONTS" => (verdict.to_string(), Tone::Note),
                    other => (other.to_string(), Tone::Bad),
                };
                let tile = Tile { title: s.id.clone(), badge: format!("{badge}  {}  @{}", size_of(&res), res.dump.header.scale), tone, image: d.image.clone() };
                evals.push(Eval { verdict, tile, triptych });
            }
        }
        rows.push(row);
    }
    // the scenes that need a look first, the rest in the order they were named
    let mut order: Vec<usize> = (0..evals.len()).collect();
    order.sort_by_key(|i| evals[*i].verdict == "ok");
    let mut tiles: Vec<Tile> = Vec::new();
    let mut taken: Vec<Option<Eval>> = evals.into_iter().map(Some).collect();
    for i in order {
        let e = taken[i].take().expect("each once");
        match e.triptych {
            Some(three) if diff => tiles.extend(three),
            _ => tiles.push(e.tile),
        }
    }
    let first = found.scenes.first().map(|s| s.root.join(".look")).unwrap_or_default();
    let path = out.clone().unwrap_or_else(|| first.join("sheet.png"));
    let dir = path.parent().map(Path::to_path_buf).unwrap_or(first);
    let mut code = exit_of(bad, infra, findings);
    if !tiles.is_empty() {
        let header = format!("wayfinder scene sheet   {} scene{}   fonts {fonts}   adapter {}", rows.len(), if rows.len() == 1 { "" } else { "s" }, adapter.as_deref().unwrap_or("none"));
        if let Err(e) = write_sheet(&mut device, &tiles, &header, cols, &path, quiet) {
            eprintln!("wayfinder: cannot make the sheet: {e}");
            code = exit_of(bad, true, findings);
        }
    }
    let totals = Totals { rows, dump_hashes, hermetic, fonts, reason: None, adapter };
    let (full, last) = totals.summaries(code, None, false);
    let (console, _) = totals.summaries(code, Some(20), false);
    for (name, text) in [("summary.txt", full), ("last.json", last)] {
        if let Err(e) = write(&dir.join(name), &text) {
            eprintln!("wayfinder: cannot write the output: {e}");
            return exit_of(bad, true, findings);
        }
    }
    print!("{}{console}", if quiet { "" } else { "\n" });
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_judged_scene_gets_the_badge_of_what_is_the_matter_with_it() {
        assert_eq!(badge_of("ok", &0), ("ok".to_string(), Tone::Ok));
        assert_eq!(badge_of("FLAGS", &3), ("FLAGS 3".to_string(), Tone::Bad));
        assert_eq!(badge_of("WARN", &1), ("WARN 1".to_string(), Tone::Note));
        assert_eq!(badge_of("ERROR", &1), ("ERROR".to_string(), Tone::Bad));
    }
}
