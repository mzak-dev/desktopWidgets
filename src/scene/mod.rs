//! Scenes: named, reproducible frames of a widget, and the `wayfinder scene` commands that
//! list them and dump what they look like as text, with no GPU.
//!
//! A scene (`file`) names a target and the world it is shown in; the runner (`runner`) builds
//! that frame in the hermetic environment of `render`; the dump (`dump`) describes it node by
//! node, and the views (`views`) let a reader take a part of it. Output goes to
//! `<scene root>/.look/` (`summary.txt`, `last.json`, one dump and record per scene), so a
//! caller whose stdout is lost can still read the result.
//!
//! `scene check` is the same run without a view: it reads the layout flags (`flags`), the
//! `[expect]` assertions and the widget's own warnings, and nothing else, so a scene set (the
//! built-in `fits` set) can be a test and a command. A scene's `[expect].flags` says whether a
//! flag is an error (a finding), a warning or ignored.
//!
//! Exit codes: 0 clean, 1 findings (a widget error card, a failed `[expect]`, an error-level
//! flag, a code source that never answered), 2 bad arguments or scene file (nothing is run if a
//! file is bad), 3 the output could not be written. The scene file, the dump and these verbs are tooling:
//! they carry `format = 1` and may change in minor releases.
//!
//! Fonts are the machine's, so a dump is exact on the machine and font set it was made on
//! (the header's `fonts=` says which); it is not a cross-machine artefact.

pub mod dump;
pub mod file;
pub mod flags;
pub mod runner;
pub mod views;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value as Json, json};

use self::dump::SceneDump;
use self::file::{FlagLevel, Scene, Size};
use self::runner::{Outcome, Overrides};
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
  wayfinder scene check <id|glob|set|file>... [--out DIR] [--root DIR] [-q] [--widget <id | file.toml>] [same flags as dump's environment]
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
    Check { select: Selection, out: Option<PathBuf>, quiet: bool, overrides: Overrides },
}

/// The command in `args` (what follows `scene`).
pub fn parse(args: &[String]) -> Result<Invocation, String> {
    let verbs = ["list", "dump", "render", "sheet", "check", "diff", "bless", "selfcheck", "guide"];
    let verb = args.first().map(String::as_str).ok_or("scene needs a verb: list, dump or check")?;
    if !verbs.contains(&verb) {
        return Err(format!("unknown scene verb `{verb}`{} (this build has list, dump and check)", suggest(verb, &[&verbs])));
    }
    if !["list", "dump", "check"].contains(&verb) {
        return Err(format!("`scene {verb}` is not in this build yet; `scene list`, `scene dump` and `scene check` are"));
    }
    let mut select = Selection::default();
    let (mut query, mut format, mut out, mut quiet, mut sets) = (Query::default(), Format::Text, None, false, false);
    let mut o = Overrides::default();
    // the verbs that run scenes share the options that set the world they run in
    let runs = verb != "list";
    let pin = |o: &mut Overrides, k: &str, v: Json| o.pins.push((k.to_string(), v));
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--sets" if verb == "list" => sets = true,
            "--root" => select.root = Some(val()?.into()),
            "--widget" if runs => select.widget = Some(val()?),
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
            "--out" if runs => out = Some(val()?.into()),
            "-q" if runs => quiet = true,
            "--env" if runs => {
                let (k, v) = pair(&val()?, "--env")?;
                pin(&mut o, &k, v);
            }
            "--now" if runs => pin(&mut o, "now", Json::String(val()?)),
            "--real" if runs => pin(&mut o, "real", Json::String(val()?)),
            "--scale" if runs => pin(&mut o, "scale", loose(&val()?)),
            "--palette" if runs => pin(&mut o, "palette", Json::String(val()?)),
            "--transparent" if runs => pin(&mut o, "transparent", Json::Bool(true)),
            "--time" if runs => {
                let v = val()?;
                let (h, m) = v.split_once(':').ok_or("--time is HH:MM")?;
                o.time = Some((h.parse().map_err(|_| "--time is HH:MM")?, m.parse().map_err(|_| "--time is HH:MM")?));
            }
            "--size" if runs => {
                let (w, h) = file::parse_size(&val()?).map_err(|e| format!("--size: {e}"))?;
                o.size = Some(Size::Card(w, h));
            }
            "--tier" if runs => o.size = Some(Size::Tier(val()?)),
            "--param" if runs => o.params.push(pair(&val()?, "--param")?),
            "--state" if runs => o.state.push(pair(&val()?, "--state")?),
            "--hide" if runs => o.hide.push(val()?),
            "--hover" if runs => o.hover = Some(val()?),
            "--content-root" if runs => o.plugin = Some(val()?.into()),
            "--wait" if runs => o.wait = Some(val()?.parse().map_err(|_| "--wait is seconds")?),
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
        "check" => Invocation::Check { select, out, quiet, overrides: o },
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

/// What a run of scenes found, for the summary and `last.json`.
struct Totals {
    rows: Vec<Row>,
    dump_hashes: Vec<String>,
    hermetic: bool,
    fonts: String,
}

impl Totals {
    /// `summary.txt` and `last.json`: the summary lists every scene, or, with `only_bad`, the
    /// ones that need a look when there are more than that many.
    fn summaries(&self, code: i32, only_bad: Option<usize>) -> (String, String) {
        let rows = &self.rows;
        let run_id = fnv(self.dump_hashes.iter().map(String::as_bytes).chain(rows.iter().map(|r| r.id.as_bytes())))[..6].to_string();
        let ok = rows.iter().filter(|r| r.verdict == "ok").count();
        let mut summary = format!("{:<34}{:<9}{}
", "scene", "verdict", "detail");
        for r in rows.iter().filter(|r| only_bad.is_none_or(|n| rows.len() <= n || r.verdict != "ok")) {
            summary.push_str(&format!("{:<34}{:<9}{}
", r.id, r.verdict, r.detail));
        }
        summary.push_str(&format!("{} scene{}, {ok} ok, {} need a look. exit {code}. run {run_id} hermetic={} adapter=none fonts={}
", rows.len(), if rows.len() == 1 { "" } else { "s" }, rows.len() - ok, self.hermetic, if self.fonts.is_empty() { "-" } else { &self.fonts }));
        let last = json!({
            "format": 1,
            "engine": env!("CARGO_PKG_VERSION"),
            "exit": code,
            "run": run_id,
            "hermetic": self.hermetic,
            "adapter": Json::Null,
            "fonts": self.fonts,
            "scenes": rows.iter().map(|r| json!({ "id": r.id, "verdict": r.verdict, "detail": r.detail, "scene_hash": r.scene_hash, "dump_hash": r.dump_hash })).collect::<Vec<_>>(),
        });
        (summary, serde_json::to_string_pretty(&last).unwrap_or_default() + "
")
    }
}

/// What a run says about its scene: the verdict for the summary, whether it is a finding (exit
/// 1) and every line behind it.
struct Judged {
    verdict: &'static str,
    detail: String,
    finding: bool,
    /// Each problem, whether it is a finding, and its text.
    lines: Vec<(bool, String)>,
}

/// Judges a run: the widget's error card, then the scene's `[expect]`, then its flags and the
/// widget's build warnings at the level the scene gives them (`error` is a finding, `warn` is
/// shown and passes, `ignore` is not looked at).
fn judge(s: &Scene, res: &Outcome) -> Judged {
    let mut found: Vec<(&'static str, String)> = Vec::new();
    found.extend(res.error.iter().map(|e| ("ERROR", e.clone())));
    found.extend(res.unmet.iter().map(|u| ("EXPECT", u.clone())));
    let level = match s.expect.flags {
        FlagLevel::Error => Some("FLAGS"),
        FlagLevel::Warn => Some("WARN"),
        FlagLevel::Ignore => None,
    };
    if let Some(level) = level {
        found.extend(res.dump.flags.iter().filter(|f| !s.expect.allow.contains(&f.flag)).map(|f| (level, format!("{} {}  {}", f.flag, f.key, f.detail))));
        found.extend(res.warnings.iter().map(|w| (level, format!("warning: {w}"))));
    }
    let rank = |v: &str| ["ERROR", "EXPECT", "FLAGS", "WARN"].iter().position(|r| *r == v).unwrap_or(usize::MAX);
    let Some(verdict) = found.iter().map(|(v, _)| *v).min_by_key(|v| rank(v)) else {
        return Judged { verdict: "ok", detail: String::new(), finding: false, lines: vec![] };
    };
    let of: Vec<&str> = found.iter().filter(|(v, _)| *v == verdict).map(|(_, l)| l.as_str()).collect();
    let detail = match verdict {
        "FLAGS" | "WARN" => format!("{} {}{}", of.len(), of[0], if of.len() > 1 { format!(" (+{} more)", of.len() - 1) } else { String::new() }),
        _ => of.join("; "),
    };
    Judged { verdict, detail, finding: found.iter().any(|(v, _)| *v != "WARN"), lines: found.into_iter().map(|(v, l)| (v != "WARN", l)).collect() }
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
        Invocation::Check { select, out, quiet, overrides } => check_cmd(&select, out, quiet, &overrides),
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
                let j = judge(s, &res);
                (row.verdict, row.detail, findings) = (j.verdict, j.detail, findings || j.finding);
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
    let totals = Totals { rows, dump_hashes, hermetic, fonts };
    let (summary, last) = totals.summaries(code, None);
    let dir = out.clone().unwrap_or_else(|| first_root.join(".look"));
    for (name, text) in [("summary.txt", summary.clone()), ("last.json", last)] {
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

/// One scene of a check.
#[derive(Debug)]
pub struct CheckRow {
    pub id: String,
    /// The card, logical px (what `fits.rs` named a case by).
    pub card: [f64; 2],
    pub verdict: &'static str,
    pub detail: String,
    /// The scene has a finding (a warning alone is not one).
    pub finding: bool,
    /// Every problem the run found, each marked as a finding or not.
    pub lines: Vec<(bool, String)>,
    /// The request could not be made (exit 2), as against a run that went wrong (exit 1).
    bad: bool,
    root: PathBuf,
    scene_hash: String,
    dump_hash: Option<String>,
    /// The dump and the record of a scene that had something to say, for `.look/`.
    keep: Option<(String, Json)>,
}

/// What `check` found: a row per scene, or the problems that stopped it before anything ran.
#[derive(Debug, Default)]
pub struct Report {
    pub rows: Vec<CheckRow>,
    /// Scene files that could not be read, or no scene matching: nothing was run.
    pub problems: Vec<String>,
    hermetic: bool,
    fonts: String,
}

impl Report {
    /// Something ran, and nothing found a problem the scenes call an error.
    pub fn is_clean(&self) -> bool {
        self.problems.is_empty() && !self.rows.is_empty() && self.rows.iter().all(|r| !r.finding)
    }

    /// One line per finding, `scene id (card size): what`, the way `fits.rs` named its cases.
    pub fn failures(&self) -> Vec<String> {
        let mut out = self.problems.clone();
        for r in &self.rows {
            for (finding, l) in &r.lines {
                if *finding {
                    out.push(format!("{} ({}x{}): {l}", r.id, dump::n(r.card[0]), dump::n(r.card[1])));
                }
            }
        }
        out
    }

    fn exit(&self, infra: bool) -> i32 {
        exit_of(!self.problems.is_empty() || self.rows.iter().any(|r| r.bad), infra, self.rows.iter().any(|r| r.finding))
    }
}

/// Runs `scenes` on a few threads (each scene builds its own world), results in order.
fn run_all(scenes: &[Scene], o: &Overrides) -> Vec<Result<Outcome, Failure>> {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let slots: Vec<Mutex<Option<Result<Outcome, Failure>>>> = scenes.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 8).min(scenes.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            // a layout recurses as deep as the widget nests: give the workers room
            let work = || {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(s) = scenes.get(i) else { break };
                    *slots[i].lock().unwrap() = Some(runner::run(s, o));
                }
            };
            std::thread::Builder::new().stack_size(8 << 20).spawn_scoped(scope, work).expect("a worker thread starts");
        }
    });
    slots.into_iter().map(|m| m.into_inner().unwrap().expect("every scene ran")).collect()
}

/// Runs the scenes `select` names and judges each by its flags, `[expect]` and the widget's own
/// warnings. Nothing is written and no GPU is used.
pub fn check_in(select: &Selection, o: &Overrides) -> Report {
    let found = find(select, o);
    if !found.problems.is_empty() {
        return Report { problems: found.problems, ..Default::default() };
    }
    let runs = run_all(&found.scenes, o);
    let mut report = Report { hermetic: true, ..Default::default() };
    for (s, run) in found.scenes.iter().zip(runs) {
        let mut row = CheckRow { id: s.id.clone(), card: [0.0; 2], verdict: "ok", detail: String::new(), finding: false, lines: vec![], bad: false, root: s.root.clone(), scene_hash: s.source_hash.clone(), dump_hash: None, keep: None };
        match run {
            Err(Failure::Bad(e)) => (row.verdict, row.detail, row.finding, row.bad, row.lines) = ("BAD", e.clone(), true, true, vec![(true, e)]),
            Err(Failure::Run(e)) => (row.verdict, row.detail, row.finding, row.lines) = ("FAILED", e.clone(), true, vec![(true, e)]),
            Ok(res) => {
                let j = judge(s, &res);
                let text = res.dump.to_text();
                row.card = res.dump.header.card;
                row.dump_hash = Some(fnv([text.as_bytes()]));
                report.hermetic &= res.dump.header.hermetic;
                report.fonts = res.dump.header.fonts.clone();
                (row.verdict, row.detail, row.finding) = (j.verdict, j.detail, j.finding);
                if !j.lines.is_empty() {
                    row.keep = Some((text, res.sidecar));
                }
                row.lines = j.lines;
            }
        }
        report.rows.push(row);
    }
    report
}

/// The scenes of `set` (a folder of ids, like `fits`) under `./scenes`, checked: what a test
/// calls and `wayfinder scene check <set>` runs.
pub fn check_set(set: &str) -> Report {
    check_in(&Selection { patterns: vec![set.to_string()], root: None, widget: None }, &Overrides::default())
}

/// The dump of `widget` (a built-in's id or a widget file) at a card size, at scale 1, in the
/// hermetic environment: the text and layout a test can read.
pub fn dump_widget(widget: &str, size: (f32, f32)) -> Result<SceneDump, String> {
    let o = Overrides { size: Some(Size::Card(size.0, size.1)), pins: vec![("scale".into(), Json::from(1))], ..Default::default() };
    let out = runner::run(&adhoc(widget, &o), &o).map_err(String::from)?;
    match out.error {
        Some(e) => Err(e),
        None => Ok(out.dump),
    }
}

fn check_cmd(select: &Selection, out: Option<PathBuf>, quiet: bool, o: &Overrides) -> i32 {
    let report = check_in(select, o);
    if !report.problems.is_empty() {
        report.problems.iter().for_each(|p| eprintln!("wayfinder: {p}"));
        eprintln!("wayfinder: nothing was run");
        return 2;
    }
    if !quiet {
        for r in &report.rows {
            for (finding, l) in &r.lines {
                println!("{} ({}x{}): {}{l}", r.id, dump::n(r.card[0]), dump::n(r.card[1]), if *finding { "" } else { "warning: " });
            }
        }
    }
    // the scenes with something to say keep their dump and record beside the summary
    let mut infra = false;
    for r in &report.rows {
        let Some((text, sidecar)) = &r.keep else { continue };
        let base = out.clone().unwrap_or_else(|| r.root.join(".look")).join(&r.id);
        for (ext, body) in [("dump.txt", text.clone()), ("env.json", serde_json::to_string_pretty(sidecar).unwrap_or_default() + "\n")] {
            if let Err(e) = write(&PathBuf::from(format!("{}.{ext}", base.display())), &body) {
                eprintln!("wayfinder: cannot write the output: {e}");
                infra = true;
            }
        }
    }
    let code = report.exit(infra);
    let rows = report.rows.iter().map(|r| Row { id: r.id.clone(), verdict: r.verdict, detail: r.detail.clone(), scene_hash: r.scene_hash.clone(), dump_hash: r.dump_hash.clone() }).collect();
    let dump_hashes = report.rows.iter().filter_map(|r| r.dump_hash.clone()).collect();
    let totals = Totals { rows, dump_hashes, hermetic: report.hermetic, fonts: report.fonts.clone() };
    // the file lists every scene; the console only the ones that need a look in a long set
    let (full, last) = totals.summaries(code, None);
    let (console, _) = totals.summaries(code, Some(20));
    let dir = out.unwrap_or_else(|| report.rows.first().map(|r| r.root.join(".look")).unwrap_or_default());
    for (name, text) in [("summary.txt", full), ("last.json", last)] {
        if let Err(e) = write(&dir.join(name), &text) {
            eprintln!("wayfinder: cannot write the output: {e}");
            return report.exit(true);
        }
    }
    print!("{}{console}", if quiet { "" } else { "\n" });
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
        let Ok(Invocation::Check { select, out, quiet, overrides }) = parse(&args(&["check", "fits", "--out", "o", "-q", "--env", "sys.cpu=1", "--root", "r", "--size", "300x200"])) else { panic!() };
        assert_eq!((select.patterns, select.root, out, quiet, overrides.pins.len(), overrides.size), (vec!["fits".to_string()], Some(PathBuf::from("r")), Some(PathBuf::from("o")), true, 1, Some(Size::Card(300.0, 200.0))));
        for bad in [&["check", "x", "--view", "full"][..], &["check", "x", "--format", "json"], &["check", "x", "--gpu", "software"], &["check", "x", "--sets"]] {
            assert!(parse(&args(bad)).is_err(), "{bad:?}");
        }
        assert!(parse(&args(&["dmp"])).unwrap_err().contains("did you mean `dump`"));
        assert!(parse(&args(&["dump", "x", "--view", "tree"])).unwrap_err().contains("outline, full, texts, hits, flags"));
    }

    fn outcome(flags: &[(&str, &str)], warnings: &[&str]) -> Outcome {
        let mut dump = crate::scene::dump::fixtures::tiny_dump(&["Hello"]);
        dump.flags = flags.iter().map(|(f, k)| dump::FlagRow { flag: (*f).into(), key: (*k).into(), detail: "d".into() }).collect();
        Outcome { dump, sidecar: Json::Null, unmet: vec![], error: None, notes: vec![], warnings: warnings.iter().map(|w| (*w).to_string()).collect() }
    }

    fn scene_with(flags: FlagLevel, allow: &[&str]) -> Scene {
        let mut s = adhoc("clock", &Overrides::default());
        s.expect = file::Expect { flags, allow: allow.iter().map(|a| (*a).to_string()).collect(), ..Default::default() };
        s
    }

    #[test]
    fn a_scenes_flag_level_decides_whether_a_flag_is_a_finding_a_warning_or_nothing() {
        let res = outcome(&[("TRUNCATED", "w/a"), ("OVERFLOW-Y", "w/b"), ("TRUNCATED", "w/c")], &["a binding did not resolve"]);
        let j = judge(&scene_with(FlagLevel::Error, &[]), &res);
        assert_eq!((j.verdict, j.finding, j.lines.len()), ("FLAGS", true, 4));
        assert_eq!(j.detail, "4 TRUNCATED w/a  d (+3 more)");
        let j = judge(&scene_with(FlagLevel::Warn, &[]), &res);
        assert_eq!((j.verdict, j.finding, j.lines.iter().all(|(f, _)| !f)), ("WARN", false, true), "a warning is shown and passes");
        let j = judge(&scene_with(FlagLevel::Ignore, &[]), &res);
        assert_eq!((j.verdict, j.finding, j.lines.len()), ("ok", false, 0));
        let j = judge(&scene_with(FlagLevel::Error, &["TRUNCATED", "OVERFLOW-Y"]), &res);
        assert_eq!((j.verdict, j.lines.len()), ("FLAGS", 1), "an allowed flag is neither a finding nor a warning; the widget's warning is still one");
        let mut res = outcome(&[("TRUNCATED", "w/a")], &[]);
        res.unmet = vec!["expect text: no text run holds `x`".into()];
        res.error = Some("the widget broke".into());
        let j = judge(&scene_with(FlagLevel::Error, &[]), &res);
        assert_eq!((j.verdict, j.detail.as_str(), j.lines.len()), ("ERROR", "the widget broke", 3), "the error card is the verdict; everything stays in the lines");
        res.error = None;
        assert_eq!(judge(&scene_with(FlagLevel::Error, &[]), &res).verdict, "EXPECT");
    }

    #[test]
    fn check_runs_a_set_without_a_device_and_names_each_failing_case_like_fits_did() {
        let dir = std::env::temp_dir().join(format!("wf-scene-check-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let scenes = dir.join("scenes");
        std::fs::create_dir_all(scenes.join("sets/widget")).unwrap();
        // a text 20 px wide in a one-line box can only be cut off, whatever the fonts are
        std::fs::write(scenes.join("sets/widget/narrow.toml"), "name = 'Narrow'\nsize = [120, 60]\nmin_size = [48, 48]\n[root]\npadding = 8\n  [[root.children]]\n  type = 'text'\n  text = 'Wednesday 23 September'\n  size = 14\n  width = 20\n").unwrap();
        let scene = |name: &str, level: &str| std::fs::write(scenes.join(format!("sets/{name}.scene.toml")), format!("format = 1\n[widget]\nfile = \"widget/narrow.toml\"\n[expect]\nflags = \"{level}\"\n")).unwrap();
        scene("a-error", "error");
        scene("b-warn", "warn");
        let select = |p: &str| Selection { patterns: vec![p.into()], root: Some(scenes.clone()), widget: None };
        let r = check_in(&select("sets"), &Overrides::default());
        assert!(!r.is_clean() && r.problems.is_empty() && r.rows.len() == 2, "{r:?}");
        assert_eq!(r.failures(), [format!("sets/a-error (120x60): TRUNCATED narrow-1/0  \"Wednesday 23 September\" nat {} > box 20", dump::n(nat_of(&r)))]);
        assert_eq!((r.rows[0].verdict, r.rows[1].verdict, r.rows[1].finding), ("FLAGS", "WARN", false));
        assert!(check_in(&select("sets/b-warn"), &Overrides::default()).is_clean(), "a warning alone is clean");
        let none = check_in(&select("sets/nothing"), &Overrides::default());
        assert!(!none.is_clean() && none.problems[0].contains("no scene matches"), "{none:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The natural width the first flagged text of a report was dumped with.
    fn nat_of(r: &Report) -> f64 {
        let text = &r.rows[0].keep.as_ref().expect("dumped").0;
        let at = text.find("nat=").expect("a text line") + 4;
        text[at..].split('x').next().unwrap().parse().unwrap()
    }

    #[test]
    fn dump_widget_reads_a_builtin_at_a_size_and_a_missing_one_is_an_error() {
        let d = dump_widget("digital_clock", (340.0, 220.0)).unwrap();
        assert!(d.texts().iter().any(|(_, t)| *t == "London"), "{:?}", d.texts());
        assert_eq!((d.header.card, d.header.scale), ([340.0, 220.0], 1.0));
        assert!(dump_widget("no-such-widget", (100.0, 100.0)).unwrap_err().contains("no widget"));
    }

    #[test]
    fn the_exit_code_ranks_bad_arguments_then_infrastructure_then_findings() {
        assert_eq!((exit_of(false, false, false), exit_of(false, false, true), exit_of(false, true, true), exit_of(true, true, true)), (0, 1, 3, 2));
    }
}
