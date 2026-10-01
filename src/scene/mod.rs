//! Scenes: named, reproducible frames of a widget, and the `wayfinder scene` commands that
//! list them and dump what they look like as text, with no GPU.
//!
//! A scene (`file`) names a target and the world it is shown in; the runner (`runner`) builds
//! that frame in the hermetic environment of `render`; the dump (`dump`) describes it node by
//! node, and the views (`views`) let a reader take a part of it. Output goes to
//! `<scene root>/.look/` (`summary.txt`, `last.json`, one dump and record per scene), so a
//! caller whose stdout is lost can still read the result.
//!
//! Exit codes: 0 clean, 1 findings (a widget error card, a failed `[expect]`, a code source
//! that never answered), 2 bad arguments or scene file (nothing is run if a file is bad), 3
//! the output could not be written. The scene file, the dump and these verbs are tooling:
//! they carry `format = 1` and may change in minor releases.
//!
//! Fonts are the machine's, so a dump is exact on the machine and font set it was made on
//! (the header's `fonts=` says which); it is not a cross-machine artefact.

pub mod dump;
pub mod file;
pub mod runner;
pub mod views;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value as Json, json};

use self::file::{Scene, Size};
use self::runner::Overrides;
use self::views::{Query, View};
use crate::render::{Content, Failure, env::fnv};
use crate::suggest::suggest;
use crate::widgets::WidgetMeta;

pub const USAGE: &str = "usage:
  wayfinder scene list [<set|glob>...] [--sets] [--root DIR]
  wayfinder scene dump <id|glob|set|file>... [--view outline|full|texts|hits|flags] [--under KEY] [--depth N]
                       [--find TEXT] [--format text|json] [--out DIR] [--root DIR] [-q]
                       [--env key=value]... [--now ISO] [--time HH:MM] [--real seams] [--scale N] [--palette NAME]
                       [--transparent] [--size WxH | --tier NAME] [--param k=v]... [--state k=v]... [--hide ID]...
                       [--hover KEY] [--content-root DIR] [--wait secs]
  wayfinder scene dump --widget <id | file.toml> [same flags]
exit: 0 clean, 1 findings, 2 bad arguments or scene, 3 output not written";

/// `v` as JSON when it is (numbers, booleans, lists), else as text.
pub fn loose(v: &str) -> Json {
    serde_json::from_str(v).unwrap_or_else(|_| Json::String(v.to_string()))
}

/// `name=value` with a loose value.
pub fn pair(s: &str, what: &str) -> Result<(String, Json), String> {
    let (k, v) = s.split_once('=').ok_or_else(|| format!("{what} `{s}`: write it as name=value"))?;
    Ok((k.trim().to_string(), loose(v)))
}

/// Which scenes a command names.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    /// Ids, globs, sets, scene files or folders; none = every scene under the root.
    pub patterns: Vec<String>,
    /// The folder ids are relative to; `./scenes` by default.
    pub root: Option<PathBuf>,
    /// One widget instead of scene files.
    pub widget: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Text,
    Json,
}

#[derive(Debug, PartialEq)]
pub enum Invocation {
    List { select: Selection, sets: bool },
    Dump { select: Selection, query: Query, format: Format, out: Option<PathBuf>, quiet: bool, overrides: Overrides },
}

/// The command in `args` (what follows `scene`).
pub fn parse(args: &[String]) -> Result<Invocation, String> {
    let verbs = ["list", "dump", "render", "sheet", "check", "diff", "bless", "selfcheck", "guide"];
    let verb = args.first().map(String::as_str).ok_or("scene needs a verb: list or dump")?;
    if !verbs.contains(&verb) {
        return Err(format!("unknown scene verb `{verb}`{} (this build has list and dump)", suggest(verb, &[&verbs])));
    }
    if !["list", "dump"].contains(&verb) {
        return Err(format!("`scene {verb}` is not in this build yet; `scene list` and `scene dump` are"));
    }
    let mut select = Selection::default();
    let (mut query, mut format, mut out, mut quiet, mut sets) = (Query::default(), Format::Text, None, false, false);
    let mut o = Overrides::default();
    let pin = |o: &mut Overrides, k: &str, v: Json| o.pins.push((k.to_string(), v));
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--sets" if verb == "list" => sets = true,
            "--root" => select.root = Some(val()?.into()),
            "--widget" if verb == "dump" => select.widget = Some(val()?),
            "--view" if verb == "dump" => query.view = View::parse(&val()?)?,
            "--under" if verb == "dump" => query.under = Some(val()?),
            "--depth" if verb == "dump" => query.depth = Some(val()?.parse().map_err(|_| "--depth is a whole number")?),
            "--find" if verb == "dump" => query.find = Some(val()?),
            "--format" if verb == "dump" => {
                format = match val()?.as_str() {
                    "text" => Format::Text,
                    "json" => Format::Json,
                    other => return Err(format!("--format is text or json, not `{other}`")),
                }
            }
            "--out" if verb == "dump" => out = Some(val()?.into()),
            "-q" if verb == "dump" => quiet = true,
            "--env" if verb == "dump" => {
                let (k, v) = pair(&val()?, "--env")?;
                pin(&mut o, &k, v);
            }
            "--now" if verb == "dump" => pin(&mut o, "now", Json::String(val()?)),
            "--real" if verb == "dump" => pin(&mut o, "real", Json::String(val()?)),
            "--scale" if verb == "dump" => pin(&mut o, "scale", loose(&val()?)),
            "--palette" if verb == "dump" => pin(&mut o, "palette", Json::String(val()?)),
            "--transparent" if verb == "dump" => pin(&mut o, "transparent", Json::Bool(true)),
            "--time" if verb == "dump" => {
                let v = val()?;
                let (h, m) = v.split_once(':').ok_or("--time is HH:MM")?;
                o.time = Some((h.parse().map_err(|_| "--time is HH:MM")?, m.parse().map_err(|_| "--time is HH:MM")?));
            }
            "--size" if verb == "dump" => {
                let (w, h) = file::parse_size(&val()?).map_err(|e| format!("--size: {e}"))?;
                o.size = Some(Size::Card(w, h));
            }
            "--tier" if verb == "dump" => o.size = Some(Size::Tier(val()?)),
            "--param" if verb == "dump" => o.params.push(pair(&val()?, "--param")?),
            "--state" if verb == "dump" => o.state.push(pair(&val()?, "--state")?),
            "--hide" if verb == "dump" => o.hide.push(val()?),
            "--hover" if verb == "dump" => o.hover = Some(val()?),
            "--content-root" if verb == "dump" => o.plugin = Some(val()?.into()),
            "--wait" if verb == "dump" => o.wait = Some(val()?.parse().map_err(|_| "--wait is seconds")?),
            "--gpu" | "--allow-hardware" => return Err(format!("{a} is not accepted: scene commands never take a hardware adapter (a dump needs none)")),
            flag if flag.starts_with('-') => return Err(format!("unknown option `{flag}` for `scene {verb}`")),
            pattern => select.patterns.push(pattern.to_string()),
        }
    }
    if select.widget.is_some() && !select.patterns.is_empty() {
        return Err("--widget names one widget; it takes no scene ids".into());
    }
    Ok(match verb {
        "list" => Invocation::List { select, sets },
        _ => Invocation::Dump { select, query, format, out, quiet, overrides: o },
    })
}

/// `*` (any run of characters) and `?` (one character).
fn glob(pat: &[char], text: &[char]) -> bool {
    match pat.split_first() {
        None => text.is_empty(),
        Some(('*', rest)) => (0..=text.len()).any(|i| glob(rest, &text[i..])),
        Some(('?', rest)) => !text.is_empty() && glob(rest, &text[1..]),
        Some((c, rest)) => text.first() == Some(c) && glob(rest, &text[1..]),
    }
}

/// Whether `pattern` selects the scene `id`: the id itself, a glob over ids, a set (a folder
/// of ids) or every variant of a scene.
pub fn matches(pattern: &str, id: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), id.chars().collect());
    let pattern = pattern.trim_end_matches('/');
    glob(&p, &t) || id.strip_prefix(pattern).is_some_and(|r| r.starts_with(['/', '@', '#']))
}

/// Whether the scene file with base id `base` may hold a scene `pattern` selects (so a widget
/// is loaded to expand a sweep only when it might).
fn may_hold(pattern: &str, base: &str) -> bool {
    match pattern.find(['*', '?']) {
        Some(i) => base.starts_with(&pattern[..i]) || pattern[..i].starts_with(base),
        None => matches(pattern, base) || pattern.strip_prefix(base).is_some_and(|r| r.starts_with(['@', '#'])),
    }
}

fn scene_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if p.is_dir() {
            if !name.starts_with('.') && name != "baselines" {
                scene_files(&p, out);
            }
        } else if file::is_scene_file(&p) {
            out.push(p);
        }
    }
}

/// What a command found: the scenes and the folder their output goes under.
struct Found {
    scenes: Vec<Scene>,
    /// Scene files that could not be read: each problem, in full.
    problems: Vec<String>,
}

fn meta_of(s: &Scene) -> Result<WidgetMeta, String> {
    let r = runner::request(s, &Overrides::default())?;
    let c = Content::load(&r)?;
    c.meta().cloned().ok_or_else(|| format!("{} ({}): widget `{}` does not load", s.file.display(), s.id, c.id))
}

fn adhoc(widget: &str, o: &Overrides) -> Scene {
    let file = widget.ends_with(".toml");
    let (name, widget_ref) = if file {
        let abs = std::path::absolute(widget).unwrap_or_else(|_| PathBuf::from(widget));
        (abs.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), abs.to_string_lossy().into_owned())
    } else {
        (widget.to_string(), widget.to_string())
    };
    let root = std::env::current_dir().unwrap_or_default();
    Scene {
        id: format!("adhoc/{name}"),
        name: name.clone(),
        tags: vec![],
        file: root.join(format!("{name}.scene.toml")),
        root,
        target: file::Target { widget: widget_ref, plugin: o.plugin.clone(), size: Size::Default, params: vec![], state: vec![], hide: vec![], items: None, hover: None, hover_at: None },
        pins: vec![],
        expect: file::Expect::default(),
        source_hash: fnv([widget.as_bytes()]),
    }
}

/// The scenes `select` names, sweeps expanded; files that cannot be read are problems.
fn find(select: &Selection, o: &Overrides) -> Found {
    if let Some(w) = &select.widget {
        return Found { scenes: vec![adhoc(w, o)], problems: vec![] };
    }
    let root = select.root.clone().unwrap_or_else(|| PathBuf::from("scenes"));
    // scene files and folders named on the command line are read whole; the rest are ids
    // (globs, sets) looked up under the root
    let mut named: Vec<PathBuf> = Vec::new();
    let mut patterns: Vec<String> = Vec::new();
    for p in &select.patterns {
        let path = Path::new(p);
        if path.is_file() {
            named.push(path.to_path_buf());
        } else if path.is_dir() && !root.join(p).is_dir() {
            scene_files(path, &mut named);
        } else {
            patterns.push(p.clone());
        }
    }
    let mut problems = Vec::new();
    let mut files: Vec<(PathBuf, Option<&[String]>)> = named.into_iter().map(|f| (f, None)).collect();
    let all = ["*".to_string()];
    if !patterns.is_empty() || select.patterns.is_empty() {
        if !root.is_dir() {
            problems.push(format!("no scenes folder at {} (run from a folder that has `scenes/`, or pass --root DIR, a scene file or a folder)", root.display()));
        }
        let mut under = Vec::new();
        scene_files(&root, &mut under);
        let pats: &[String] = if patterns.is_empty() { &all } else { &patterns };
        files.extend(under.into_iter().map(|f| (f, Some(pats))));
    }
    let mut scenes: Vec<Scene> = Vec::new();
    for (f, pats) in files {
        let bases = match file::read(&f) {
            Ok(b) => b,
            Err(e) => {
                problems.push(e);
                continue;
            }
        };
        for b in bases {
            if pats.is_some_and(|ps| !ps.iter().any(|p| may_hold(p, &b.scene.id))) {
                continue;
            }
            match file::expand(&b, &mut |s| meta_of(s)) {
                Ok(list) => scenes.extend(list.into_iter().filter(|s| pats.is_none_or(|ps| ps.iter().any(|p| matches(p, &s.id))))),
                Err(e) => problems.push(e),
            }
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    scenes.retain(|s| seen.insert(s.id.clone()));
    if scenes.is_empty() && problems.is_empty() {
        problems.push(format!("no scene matches `{}`; `wayfinder scene list` shows the ids{}", select.patterns.join(" "), hint(select)));
    }
    Found { scenes, problems }
}

fn hint(select: &Selection) -> String {
    let root = select.root.clone().unwrap_or_else(|| PathBuf::from("scenes"));
    let mut files = Vec::new();
    scene_files(&root, &mut files);
    let mut ids: Vec<String> = Vec::new();
    for f in files {
        if let Ok(bases) = file::read(&f) {
            ids.extend(bases.into_iter().map(|b| b.scene.id));
        }
    }
    let pool: Vec<&str> = ids.iter().map(String::as_str).collect();
    select.patterns.iter().map(|p| suggest(p, &[&pool])).find(|s| !s.is_empty()).unwrap_or_default()
}

/// A verdict for one scene.
#[derive(Debug)]
struct Row {
    id: String,
    verdict: &'static str,
    detail: String,
    scene_hash: String,
    dump_hash: Option<String>,
}

/// The exit code of a run: bad arguments beat an unwritable output beat findings.
fn exit_of(bad: bool, infra: bool, findings: bool) -> i32 {
    if bad {
        2
    } else if infra {
        3
    } else {
        i32::from(findings)
    }
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Runs the `scene` command and returns the process exit code.
pub fn run(inv: Invocation) -> i32 {
    match inv {
        Invocation::List { select, sets } => list(&select, sets),
        Invocation::Dump { select, query, format, out, quiet, overrides } => dump_cmd(&select, &query, format, out, quiet, &overrides),
    }
}

fn list(select: &Selection, sets: bool) -> i32 {
    let found = find(select, &Overrides::default());
    found.problems.iter().for_each(|p| eprintln!("wayfinder: {p}"));
    if sets {
        let mut by: BTreeMap<String, usize> = BTreeMap::new();
        for s in &found.scenes {
            let mut parts: Vec<&str> = s.id.split('/').collect();
            parts.pop();
            for i in 1..=parts.len() {
                *by.entry(parts[..i].join("/")).or_default() += 1;
            }
        }
        by.iter().for_each(|(set, n)| println!("{set}   {n} scene{}", if *n == 1 { "" } else { "s" }));
    } else {
        found.scenes.iter().for_each(|s| println!("{}", s.id));
    }
    exit_of(!found.problems.is_empty(), false, false)
}

fn dump_cmd(select: &Selection, query: &Query, format: Format, out: Option<PathBuf>, quiet: bool, o: &Overrides) -> i32 {
    let found = find(select, o);
    if !found.problems.is_empty() {
        found.problems.iter().for_each(|p| eprintln!("wayfinder: {p}"));
        eprintln!("wayfinder: nothing was run");
        return 2;
    }
    let many = found.scenes.len() > 1;
    let (mut rows, mut bad, mut infra, mut findings) = (Vec::new(), false, false, false);
    let mut hermetic = true;
    let mut fonts = String::new();
    let mut dump_hashes = Vec::new();
    let mut docs: Vec<Json> = Vec::new();
    let first_root = found.scenes.first().map(|s| s.root.clone()).unwrap_or_default();
    let look = |s: &Scene| out.clone().unwrap_or_else(|| s.root.join(".look"));
    for s in &found.scenes {
        let mut row = Row { id: s.id.clone(), verdict: "ok", detail: String::new(), scene_hash: s.source_hash.clone(), dump_hash: None };
        match runner::run(s, o) {
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
                dump_hashes.push(hash.clone());
                row.dump_hash = Some(hash);
                if let Some(e) = &res.error {
                    (row.verdict, row.detail, findings) = ("ERROR", e.clone(), true);
                } else if !res.unmet.is_empty() {
                    (row.verdict, row.detail, findings) = ("EXPECT", res.unmet.join("; "), true);
                }
                match format {
                    Format::Text => match views::render(&res.dump, query) {
                        Ok(t) if !quiet => {
                            if many {
                                println!("# {}", s.id);
                            }
                            print!("{t}");
                        }
                        Ok(_) => {}
                        Err(e) => {
                            eprintln!("wayfinder: {e}");
                            return 2;
                        }
                    },
                    Format::Json => match views::render_json(&res.dump, query) {
                        Ok(j) => docs.push(j),
                        Err(e) => {
                            eprintln!("wayfinder: {e}");
                            return 2;
                        }
                    },
                }
                let dir = look(s);
                let base = dir.join(&s.id);
                let mut files = vec![(PathBuf::from(format!("{}.dump.txt", base.display())), text), (PathBuf::from(format!("{}.env.json", base.display())), serde_json::to_string_pretty(&res.sidecar).unwrap_or_default() + "\n")];
                if format == Format::Json {
                    files.push((PathBuf::from(format!("{}.dump.json", base.display())), serde_json::to_string_pretty(&res.dump.to_json()).unwrap_or_default() + "\n"));
                }
                for (path, text) in files {
                    if let Err(e) = write(&path, &text) {
                        eprintln!("wayfinder: cannot write the output: {e}");
                        infra = true;
                    }
                }
            }
        }
        rows.push(row);
    }
    let code = exit_of(bad, infra, findings);
    let run_id = fnv(dump_hashes.iter().map(String::as_bytes).chain(rows.iter().map(|r| r.id.as_bytes())))[..6].to_string();
    let ok = rows.iter().filter(|r| r.verdict == "ok").count();
    let mut summary = format!("{:<34}{:<9}{}\n", "scene", "verdict", "detail");
    for r in &rows {
        summary.push_str(&format!("{:<34}{:<9}{}\n", r.id, r.verdict, r.detail));
    }
    summary.push_str(&format!("{} scene{}, {ok} ok, {} need a look. exit {code}. run {run_id} hermetic={hermetic} adapter=none fonts={}\n", rows.len(), if rows.len() == 1 { "" } else { "s" }, rows.len() - ok, if fonts.is_empty() { "-" } else { &fonts }));
    let last = json!({
        "format": 1,
        "engine": env!("CARGO_PKG_VERSION"),
        "exit": code,
        "run": run_id,
        "hermetic": hermetic,
        "adapter": Json::Null,
        "fonts": fonts,
        "scenes": rows.iter().map(|r| json!({ "id": r.id, "verdict": r.verdict, "detail": r.detail, "scene_hash": r.scene_hash, "dump_hash": r.dump_hash })).collect::<Vec<_>>(),
    });
    let dir = out.clone().unwrap_or_else(|| first_root.join(".look"));
    for (name, text) in [("summary.txt", summary.clone()), ("last.json", serde_json::to_string_pretty(&last).unwrap_or_default() + "\n")] {
        if let Err(e) = write(&dir.join(name), &text) {
            eprintln!("wayfinder: cannot write the output: {e}");
            return exit_of(bad, true, findings);
        }
    }
    // stdout stays one JSON document (an array for several scenes) under --format json; the
    // summary is in the file as well
    if format == Format::Json {
        if !quiet {
            let doc = if docs.len() == 1 { docs.remove(0) } else { Json::Array(docs) };
            println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
        }
        eprint!("{summary}");
    } else {
        println!("{}{summary}", if quiet { "" } else { "\n" });
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn selection_takes_ids_globs_sets_and_every_variant_of_a_scene() {
        assert!(matches("widgets/clock/small", "widgets/clock/small"));
        assert!(matches("widgets", "widgets/clock/small") && matches("widgets/", "widgets/clock/small"), "a set");
        assert!(!matches("widget", "widgets/clock/small"), "a set is a whole folder name");
        assert!(matches("widgets/*", "widgets/clock/small") && matches("widgets/*/small", "widgets/clock/small") && matches("widgets/clock/s?all", "widgets/clock/small"));
        assert!(matches("widgets/system_monitor", "widgets/system_monitor@compact") && matches("widgets/system_monitor@compact", "widgets/system_monitor@compact#no-gauge"));
        assert!(!matches("widgets/system_monitor@large", "widgets/system_monitor@compact"));
        assert!(may_hold("widgets/system_monitor@compact", "widgets/system_monitor") && may_hold("widgets/*@compact", "widgets/system_monitor") && may_hold("fits", "fits/clock") && !may_hold("widgets/clock", "widgets/drawer"));
    }

    #[test]
    fn the_scene_commands_parse_and_unbuilt_verbs_and_a_gpu_flag_are_refused() {
        let Ok(Invocation::Dump { select, query, format, out, quiet, overrides }) = parse(&args(&["dump", "widgets/clock", "--view", "texts", "--under", "w/c", "--depth", "2", "--find", "Thu", "--format", "json", "--out", "o", "-q", "--env", "sys.cpu=42", "--now", "2026-03-08T15:42", "--size", "300x200", "--param", "ticks=false", "--hover", "w/c/b"])) else { panic!() };
        assert_eq!((select.patterns, query.view, query.under.as_deref(), query.depth, query.find.as_deref(), format, out, quiet), (vec!["widgets/clock".to_string()], View::Texts, Some("w/c"), Some(2), Some("Thu"), Format::Json, Some(PathBuf::from("o")), true));
        assert_eq!((overrides.pins.len(), overrides.size, overrides.params.len(), overrides.hover.as_deref()), (2, Some(Size::Card(300.0, 200.0)), 1, Some("w/c/b")));
        assert!(matches!(parse(&args(&["list", "--sets", "widgets"])), Ok(Invocation::List { sets: true, .. })));
        assert!(matches!(parse(&args(&["dump", "--widget", "clock"])), Ok(Invocation::Dump { select: Selection { widget: Some(_), .. }, .. })));
        for bad in [&["render", "x"][..], &["sheet"], &[], &["dmp"], &["dump", "x", "--gpu", "software"], &["dump", "x", "--allow-hardware"], &["dump", "x", "--view", "tree"], &["dump", "x", "--format", "xml"], &["dump", "x", "--depth", "deep"], &["dump", "x", "--nope"], &["dump", "x", "--find"], &["dump", "x", "--widget", "clock"], &["list", "--view", "full"]] {
            assert!(parse(&args(bad)).is_err(), "{bad:?}");
        }
        assert!(parse(&args(&["dmp"])).unwrap_err().contains("did you mean `dump`"));
        assert!(parse(&args(&["dump", "x", "--view", "tree"])).unwrap_err().contains("outline, full, texts, hits, flags"));
    }

    #[test]
    fn the_exit_code_ranks_bad_arguments_then_infrastructure_then_findings() {
        assert_eq!((exit_of(false, false, false), exit_of(false, false, true), exit_of(false, true, true), exit_of(true, true, true)), (0, 1, 3, 2));
    }
}
