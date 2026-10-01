//! Scene files: `*.scene.toml` (one scene) and `*.scenes.toml` (a `[defaults]` table and
//! `[[scene]]` tables), parsed strictly (an unknown key is an error naming the nearest
//! known one) and expanded by `[sweep]` into the scenes they stand for.
//!
//! A scene names a target, the widget (`[widget]`), and the world it is shown in (`[look]`,
//! `[env]`), and may assert things about the result (`[expect]`). The format is tooling
//! (`format = 1`): it may change in minor releases.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value as Json;

use crate::render::env::fnv;
use crate::shortcut::Shortcut;
use crate::suggest::suggest;
use crate::widgets::{ParamType, WidgetMeta};

/// The file format this build reads and writes.
pub const FORMAT: u32 = 1;

const FILE_KEYS: &[&str] = &["format", "name", "tags", "widget", "settings", "look", "env", "expect", "sweep"];
const SET_KEYS: &[&str] = &["format", "defaults", "scene"];
const SCENE_KEYS: &[&str] = &["name", "tags", "widget", "settings", "look", "env", "expect", "sweep"];
const WIDGET_KEYS: &[&str] = &["id", "file", "plugin", "size", "tier", "params", "state", "hide", "items", "hover", "hover_at", "edit"];
const LOOK_KEYS: &[&str] = &["scale", "palette", "transparent"];
const EXPECT_KEYS: &[&str] = &["flags", "text", "no_text"];
const SWEEP_KEYS: &[&str] = &["sizes", "variants", "at", "hide"];
const VARIANT_KEYS: &[&str] = &["params", "state", "hide", "size", "tier", "items", "env"];
const ITEM_KEYS: &[&str] = &["name", "target", "icon"];

/// What a scene says about its flags (`[expect].flags`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FlagLevel {
    Error,
    #[default]
    Warn,
    Ignore,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Expect {
    pub flags: FlagLevel,
    /// Each must appear as (part of) a text run.
    pub text: Vec<String>,
    /// None may appear in any text run.
    pub no_text: Vec<String>,
}

/// How a scene sizes its card.
#[derive(Clone, Debug, PartialEq)]
pub enum Size {
    /// The widget's default.
    Default,
    Card(f32, f32),
    Tier(String),
}

/// The `[widget]` table, resolved: paths absolute, sizes parsed.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    /// A built-in (or plugin) widget id, or the absolute path of a widget file.
    pub widget: String,
    pub plugin: Option<PathBuf>,
    pub size: Size,
    pub params: Vec<(String, Json)>,
    pub state: Vec<(String, Json)>,
    pub hide: Vec<String>,
    pub items: Option<Vec<Shortcut>>,
    pub hover: Option<String>,
    pub hover_at: Option<(f32, f32)>,
}

/// One scene to run (a sweep has been expanded into these).
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    /// Scene id: the path under the scene root without extension, `/name` for a `[[scene]]`,
    /// `@size` and `#variant` from a sweep.
    pub id: String,
    /// Shown on a sheet; the id when the file names none.
    pub name: String,
    pub tags: Vec<String>,
    pub file: PathBuf,
    /// The folder that holds `.look/`.
    pub root: PathBuf,
    pub target: Target,
    /// The Pins this scene sets, `[look]` then `[env]`, as the dotted keys `Pins::set` takes.
    pub pins: Vec<(String, Json)>,
    pub expect: Expect,
    /// Hash of the file's text, for `last.json`.
    pub source_hash: String,
}

/// A scene as written, before its sweep is expanded.
#[derive(Clone, Debug)]
pub struct Base {
    pub scene: Scene,
    sweep: Sweep,
}

#[derive(Clone, Debug, Default)]
struct Sweep {
    sizes: Option<Sizes>,
    variants: Vec<(String, toml::Table)>,
    at: Vec<String>,
    each_module: bool,
}

#[derive(Clone, Debug)]
enum Sizes {
    Tiers,
    Max,
    Grid(u32, u32),
    List(Vec<String>),
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RawScene {
    format: Option<u32>,
    name: Option<String>,
    tags: Vec<String>,
    widget: Option<RawWidget>,
    settings: Option<toml::Value>,
    look: RawLook,
    env: toml::Table,
    expect: RawExpect,
    sweep: RawSweep,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RawWidget {
    id: Option<String>,
    file: Option<String>,
    plugin: Option<String>,
    size: Option<String>,
    tier: Option<String>,
    params: toml::Table,
    state: toml::Table,
    hide: Option<RawHide>,
    items: Option<RawItems>,
    hover: Option<String>,
    hover_at: Option<[f32; 2]>,
    edit: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawHide {
    Each(String),
    List(Vec<String>),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawItems {
    Named(String),
    List(Vec<RawItem>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawItem {
    name: String,
    target: String,
    #[serde(default)]
    icon: String,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RawLook {
    scale: Option<f64>,
    palette: Option<String>,
    transparent: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RawExpect {
    flags: Option<String>,
    text: Vec<String>,
    no_text: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RawSweep {
    sizes: Option<RawSizes>,
    variants: toml::Table,
    at: Vec<String>,
    hide: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawSizes {
    Name(String),
    List(Vec<String>),
}

/// The seven shortcuts of `items = "demo"`: fixed names and targets, so a dump does not
/// depend on the machine (the tiles' colours come from the file names).
pub fn demo_items() -> Vec<Shortcut> {
    ["Notepad", "Calculator", "Explorer", "Terminal", "Paint", "Registry", "Settings"].iter().map(|n| Shortcut { name: (*n).into(), target: format!("C:\\Windows\\{}.exe", n.to_lowercase()), icon: String::new() }).collect()
}

/// `WxH` in logical px.
pub fn parse_size(s: &str) -> Result<(f32, f32), String> {
    let bad = || format!("`{s}` is not a size: write it as WxH, like 300x200");
    let (w, h) = s.split_once(['x', 'X']).ok_or_else(bad)?;
    let (w, h): (f32, f32) = (w.trim().parse().map_err(|_| bad())?, h.trim().parse().map_err(|_| bad())?);
    if w > 0.0 && h > 0.0 && w <= 8000.0 && h <= 8000.0 { Ok((w, h)) } else { Err(format!("`{s}` is not a size: each side is from 1 to 8000")) }
}

/// A TOML value as JSON (datetimes as their text).
pub fn json(v: &toml::Value) -> Json {
    match v {
        toml::Value::String(s) => Json::String(s.clone()),
        toml::Value::Integer(i) => Json::from(*i),
        toml::Value::Float(f) => serde_json::Number::from_f64(*f).map_or(Json::Null, Json::Number),
        toml::Value::Boolean(b) => Json::Bool(*b),
        toml::Value::Datetime(d) => Json::String(d.to_string()),
        toml::Value::Array(a) => Json::Array(a.iter().map(json).collect()),
        toml::Value::Table(t) => Json::Object(t.iter().map(|(k, v)| (k.clone(), json(v))).collect()),
    }
}

/// `[env.sys] cpu = 42` as `("sys.cpu", 42)`: tables flatten to dotted keys, lists (and a
/// list of tables, like `[[env.fetch.responses]]`) stay values.
fn flatten(prefix: &str, t: &toml::Table, out: &mut Vec<(String, Json)>) {
    for (k, v) in t {
        let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
        match v {
            toml::Value::Table(inner) => flatten(&key, inner, out),
            other => out.push((key, json(other))),
        }
    }
}

fn check_keys(t: &toml::Table, known: &[&str], at: &str, file: &Path) -> Result<(), String> {
    match t.keys().find(|k| !known.contains(&k.as_str())) {
        None => Ok(()),
        Some(k) => {
            let path = if at.is_empty() { k.clone() } else { format!("{at}.{k}") };
            Err(format!("{}: unknown key `{path}`{} (the keys here are {})", file.display(), suggest(k, &[known]), known.join(", ")))
        }
    }
}

/// Unknown keys anywhere in a scene table, with the nearest known name.
fn check_scene(t: &toml::Table, known: &[&str], file: &Path) -> Result<(), String> {
    check_keys(t, known, "", file)?;
    let sub = |name: &str, keys: &[&str]| -> Result<(), String> {
        match t.get(name) {
            Some(toml::Value::Table(inner)) => check_keys(inner, keys, name, file),
            _ => Ok(()),
        }
    };
    sub("widget", WIDGET_KEYS)?;
    sub("look", LOOK_KEYS)?;
    sub("expect", EXPECT_KEYS)?;
    sub("sweep", SWEEP_KEYS)?;
    if let Some(toml::Value::Table(w)) = t.get("widget") {
        if let Some(toml::Value::Array(items)) = w.get("items") {
            for (i, it) in items.iter().enumerate() {
                if let toml::Value::Table(it) = it {
                    check_keys(it, ITEM_KEYS, &format!("widget.items[{i}]"), file)?;
                }
            }
        }
    }
    if let Some(toml::Value::Table(s)) = t.get("sweep") {
        if let Some(toml::Value::Table(vs)) = s.get("variants") {
            for (name, v) in vs {
                if let toml::Value::Table(v) = v {
                    check_keys(v, VARIANT_KEYS, &format!("sweep.variants.{name}"), file)?;
                }
            }
        }
    }
    Ok(())
}

/// `b` over `a`: tables merge key by key, anything else is replaced.
fn merge(a: &mut toml::Table, b: &toml::Table) {
    for (k, v) in b {
        match (a.get_mut(k), v) {
            (Some(toml::Value::Table(x)), toml::Value::Table(y)) => merge(x, y),
            _ => {
                a.insert(k.clone(), v.clone());
            }
        }
    }
}

/// The nearest ancestor folder of `file` named `scenes`; else the folder it is in.
pub fn scene_root(file: &Path) -> PathBuf {
    let abs = std::path::absolute(file).unwrap_or_else(|_| file.to_path_buf());
    abs.ancestors().skip(1).find(|a| a.file_name().is_some_and(|n| n == "scenes")).map(Path::to_path_buf).unwrap_or_else(|| abs.parent().map(Path::to_path_buf).unwrap_or_default())
}

/// Whether `path` is a scene file by its name.
pub fn is_scene_file(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(".scene.toml") || n.ends_with(".scenes.toml"))
}

fn id_piece_ok(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

/// The scenes written in `file`, before their sweeps are expanded.
pub fn read(file: &Path) -> Result<Vec<Base>, String> {
    let shown = file.display();
    let text = std::fs::read_to_string(file).map_err(|e| format!("{shown}: {e}"))?;
    let doc: toml::Table = toml::from_str(&text).map_err(|e| format!("{shown}: {}", e.to_string().trim_end()))?;
    let root = scene_root(file);
    let abs = std::path::absolute(file).unwrap_or_else(|_| file.to_path_buf());
    let rel = abs.strip_prefix(&root).unwrap_or(&abs).to_string_lossy().replace('\\', "/");
    let hash = fnv([text.as_bytes()]);
    let multi = rel.ends_with(".scenes.toml");
    let stem = rel.strip_suffix(".scenes.toml").or_else(|| rel.strip_suffix(".scene.toml")).ok_or_else(|| format!("{shown}: a scene file is named *.scene.toml or *.scenes.toml"))?;
    match doc.get("format") {
        Some(toml::Value::Integer(n)) if *n == i64::from(FORMAT) => {}
        Some(other) => return Err(format!("{shown}: `format` is {other}, this build reads format = {FORMAT}")),
        None => return Err(format!("{shown}: add `format = {FORMAT}` at the top")),
    }
    let mut out = Vec::new();
    if multi {
        check_keys(&doc, SET_KEYS, "", file)?;
        let defaults = match doc.get("defaults") {
            None => toml::Table::new(),
            Some(toml::Value::Table(d)) => {
                check_scene(d, SCENE_KEYS, file).map_err(|e| e.replacen("unknown key `", "unknown key `defaults.", 1))?;
                d.clone()
            }
            Some(_) => return Err(format!("{shown}: `defaults` is a table")),
        };
        let Some(toml::Value::Array(list)) = doc.get("scene") else { return Err(format!("{shown}: a *.scenes.toml holds [[scene]] tables")) };
        for (i, s) in list.iter().enumerate() {
            let toml::Value::Table(s) = s else { return Err(format!("{shown}: scene {i} is not a table")) };
            check_scene(s, SCENE_KEYS, file)?;
            let name = match s.get("name") {
                Some(toml::Value::String(n)) if id_piece_ok(n) => n.clone(),
                Some(other) => return Err(format!("{shown}: scene {i}: `name` is part of the scene id, letters, digits and - _ . only, not {other}")),
                None => return Err(format!("{shown}: scene {i} needs a `name` (it ends the scene id)")),
            };
            let mut merged = defaults.clone();
            merge(&mut merged, s);
            out.push(build(&merged, file, &root, format!("{stem}/{name}"), name, hash.clone())?);
        }
    } else {
        check_scene(&doc, FILE_KEYS, file)?;
        let name = match doc.get("name") {
            Some(toml::Value::String(n)) => n.clone(),
            _ => stem.to_string(),
        };
        out.push(build(&doc, file, &root, stem.to_string(), name, hash)?);
    }
    Ok(out)
}

fn build(t: &toml::Table, file: &Path, root: &Path, id: String, name: String, source_hash: String) -> Result<Base, String> {
    let shown = format!("{} ({id})", file.display());
    let raw: RawScene = toml::Value::Table(t.clone()).try_into().map_err(|e: toml::de::Error| format!("{shown}: {}", e.to_string().trim_end()))?;
    let _ = raw.format;
    if raw.settings.is_some() {
        return Err(format!("{shown}: `[settings]` scenes are not supported by this build yet; only `[widget]` scenes run"));
    }
    let w = raw.widget.ok_or_else(|| format!("{shown}: a scene needs a [widget] table"))?;
    if w.edit {
        return Err(format!("{shown}: `edit = true` (the Edit Mode overlay) is not supported by this build yet"));
    }
    let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
    let abs = |base: &Path, p: &str| {
        let p = Path::new(p);
        std::path::absolute(if p.is_absolute() { p.to_path_buf() } else { base.join(p) }).unwrap_or_else(|_| p.to_path_buf())
    };
    let widget = match (&w.id, &w.file) {
        (Some(id), None) => id.clone(),
        (None, Some(f)) => abs(&dir, f).to_string_lossy().into_owned(),
        (None, None) => return Err(format!("{shown}: [widget] needs `id` (a built-in or plugin widget) or `file` (a widget file)")),
        (Some(_), Some(_)) => return Err(format!("{shown}: [widget] takes `id` or `file`, not both")),
    };
    if !w.file.as_deref().map_or(true, |f| f.ends_with(".toml")) {
        return Err(format!("{shown}: [widget] `file` is a .toml widget file"));
    }
    let plugin = match &w.plugin {
        Some(p) => Some(abs(root, p)),
        // a scenes folder inside a plugin folder implies that plugin
        None => root.parent().filter(|p| p.join("plugin.toml").is_file()).map(Path::to_path_buf),
    };
    let size = match (&w.size, &w.tier) {
        (Some(s), None) => {
            let (x, y) = parse_size(s).map_err(|e| format!("{shown}: [widget] size: {e}"))?;
            Size::Card(x, y)
        }
        (None, Some(t)) => Size::Tier(t.clone()),
        (None, None) => Size::Default,
        (Some(_), Some(_)) => return Err(format!("{shown}: [widget] takes `size` or `tier`, not both")),
    };
    let pairs = |t: &toml::Table| t.iter().map(|(k, v)| (k.clone(), json(v))).collect::<Vec<_>>();
    let items = match w.items {
        None => None,
        Some(RawItems::Named(n)) if n == "demo" => Some(demo_items()),
        Some(RawItems::Named(n)) => return Err(format!("{shown}: [widget] items is \"demo\" or a list of {{ name, target }}, not `{n}`")),
        Some(RawItems::List(l)) => Some(l.into_iter().map(|i| Shortcut { name: i.name, target: i.target, icon: i.icon }).collect()),
    };
    let (hide, hide_each) = match w.hide {
        None => (vec![], false),
        Some(RawHide::List(l)) => (l, false),
        Some(RawHide::Each(e)) if e == "each" => (vec![], true),
        Some(RawHide::Each(e)) => return Err(format!("{shown}: [widget] hide is a list of Module ids or \"each\", not `{e}`")),
    };
    let mut pins = Vec::new();
    if let Some(s) = raw.look.scale {
        pins.push(("scale".to_string(), Json::from(s)));
    }
    if let Some(p) = raw.look.palette {
        pins.push(("palette".to_string(), Json::String(p)));
    }
    if let Some(b) = raw.look.transparent {
        pins.push(("transparent".to_string(), Json::Bool(b)));
    }
    flatten("", &raw.env, &mut pins);
    for (k, v) in &mut pins {
        // a cover picture is named from the scene file
        if k == "media.art" {
            if let Json::String(s) = v {
                if !s.is_empty() {
                    *s = abs(&dir, s).to_string_lossy().into_owned();
                }
            }
        }
    }
    let flags = match raw.expect.flags.as_deref() {
        None | Some("warn") => FlagLevel::Warn,
        Some("error") => FlagLevel::Error,
        Some("ignore") => FlagLevel::Ignore,
        Some(other) => return Err(format!("{shown}: [expect] flags is error, warn or ignore, not `{other}`{}", suggest(other, &[&["error", "warn", "ignore"]]))),
    };
    let sizes = match raw.sweep.sizes {
        None => None,
        Some(RawSizes::Name(n)) => Some(match n.as_str() {
            "tiers" => Sizes::Tiers,
            "max" => Sizes::Max,
            g if g.starts_with("grid:") => {
                let (x, y) = g["grid:".len()..].split_once(['x', 'X']).and_then(|(x, y)| Some((x.parse::<u32>().ok()?, y.parse::<u32>().ok()?))).filter(|(x, y)| *x >= 2 && *y >= 2 && x * y <= 64).ok_or_else(|| format!("{shown}: [sweep] sizes `{g}`: write grid:NxM with N and M from 2 and at most 64 cells"))?;
                Sizes::Grid(x, y)
            }
            other => return Err(format!("{shown}: [sweep] sizes is tiers, max, grid:NxM or a list of WxH, not `{other}`{}", suggest(other, &[&["tiers", "max"]]))),
        }),
        Some(RawSizes::List(l)) => {
            l.iter().try_for_each(|s| parse_size(s).map(|_| ()).map_err(|e| format!("{shown}: [sweep] sizes: {e}")))?;
            Some(Sizes::List(l))
        }
    };
    let mut variants = Vec::new();
    for (name, v) in &raw.sweep.variants {
        if !id_piece_ok(name) {
            return Err(format!("{shown}: [sweep] variant name `{name}`: letters, digits and - _ . only"));
        }
        match v {
            toml::Value::Table(t) => variants.push((name.clone(), t.clone())),
            _ => return Err(format!("{shown}: [sweep] variant `{name}` is a table")),
        }
    }
    let each_module = hide_each
        || match raw.sweep.hide.as_deref() {
            None => false,
            Some("each") => true,
            Some(other) => return Err(format!("{shown}: [sweep] hide is \"each\" (one variant per Module), not `{other}`")),
        };
    let target = Target { widget, plugin, size, params: pairs(&w.params), state: pairs(&w.state), hide, items, hover: w.hover, hover_at: w.hover_at.map(|[x, y]| (x, y)) };
    let scene = Scene { id, name, tags: raw.tags, file: file.to_path_buf(), root: root.to_path_buf(), target, pins, expect: Expect { flags, text: raw.expect.text, no_text: raw.expect.no_text }, source_hash };
    Ok(Base { scene, sweep: Sweep { sizes, variants, at: raw.sweep.at, each_module } })
}

/// Sizes of the `i/(n-1), j/(m-1)` grid from min to max, as `fits.rs` walks them.
fn grid(meta: &WidgetMeta, n: u32, m: u32) -> Vec<(String, (f32, f32))> {
    let (min, max) = (meta.min_card_size, meta.max_card_size.unwrap_or(meta.default_card_size));
    let mut out = vec![("default".to_string(), meta.default_card_size)];
    for i in 0..n {
        for j in 0..m {
            let (tx, ty) = (i as f32 / (n - 1) as f32, j as f32 / (m - 1) as f32);
            out.push((format!("{i},{j}"), ((min.0 + (max.0 - min.0) * tx).round(), (min.1 + (max.1 - min.1) * ty).round())));
        }
    }
    out
}

/// A `Seed`-free copy of the overrides a variant applies to a scene.
fn apply_variant(s: &mut Scene, v: &toml::Table, meta: Option<&WidgetMeta>, shown: &str) -> Result<(), String> {
    let set = |list: &mut Vec<(String, Json)>, k: &str, val: Json| match list.iter_mut().find(|(n, _)| n == k) {
        Some(e) => e.1 = val,
        None => list.push((k.to_string(), val)),
    };
    for (key, val) in v {
        match (key.as_str(), val) {
            ("params", toml::Value::String(s_)) => {
                let on = match s_.as_str() {
                    "bools:true" => true,
                    "bools:false" => false,
                    other => return Err(format!("{shown}: variant params is a table or \"bools:true\" / \"bools:false\", not `{other}`")),
                };
                let meta = meta.ok_or_else(|| format!("{shown}: variant params \"bools:{on}\" needs the widget to load"))?;
                for p in meta.params.iter().filter(|p| p.ty == ParamType::Bool) {
                    set(&mut s.target.params, &p.name, Json::Bool(on));
                }
            }
            ("params", toml::Value::Table(t)) => t.iter().for_each(|(k, x)| set(&mut s.target.params, k, json(x))),
            ("state", toml::Value::Table(t)) => t.iter().for_each(|(k, x)| set(&mut s.target.state, k, json(x))),
            ("env", toml::Value::Table(t)) => {
                let mut flat = Vec::new();
                flatten("", t, &mut flat);
                flat.into_iter().for_each(|(k, x)| set(&mut s.pins, &k, x));
            }
            ("hide", toml::Value::Array(a)) => s.target.hide = a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
            ("size", toml::Value::String(z)) => {
                let (w, h) = parse_size(z).map_err(|e| format!("{shown}: variant size: {e}"))?;
                s.target.size = Size::Card(w, h);
            }
            ("tier", toml::Value::String(t)) => s.target.size = Size::Tier(t.clone()),
            ("items", toml::Value::String(n)) if n == "demo" => s.target.items = Some(demo_items()),
            (k, _) => return Err(format!("{shown}: variant `{k}` has the wrong kind of value")),
        }
    }
    Ok(())
}

/// Whether `expand` has to load the widget to name the scenes.
pub fn needs_widget(b: &Base) -> bool {
    matches!(b.sweep.sizes, Some(Sizes::Tiers | Sizes::Max | Sizes::Grid(..))) || b.sweep.each_module || b.sweep.variants.iter().any(|(_, v)| matches!(v.get("params"), Some(toml::Value::String(_))))
}

/// The scenes a file stands for: itself, or one per size, variant, hidden Module and frame
/// time its `[sweep]` lists. `meta` loads the widget's metadata when a sweep needs it.
pub fn expand(b: &Base, meta: &mut dyn FnMut(&Scene) -> Result<WidgetMeta, String>) -> Result<Vec<Scene>, String> {
    let shown = format!("{} ({})", b.scene.file.display(), b.scene.id);
    let m = if needs_widget(b) { Some(meta(&b.scene)?) } else { None };
    // sizes
    let mut sized: Vec<(String, Size)> = vec![(String::new(), b.scene.target.size.clone())];
    match (&b.sweep.sizes, &m) {
        (None, _) => {}
        (Some(Sizes::Tiers), Some(m)) => {
            if m.tiers.is_empty() {
                return Err(format!("{shown}: [sweep] sizes = \"tiers\", but widget `{}` declares no tiers", m.id));
            }
            sized = m.tiers.iter().map(|t| (format!("@{}", t.name), Size::Tier(t.name.clone()))).collect();
        }
        (Some(Sizes::Max), Some(m)) => {
            let max = m.max_card_size.ok_or_else(|| format!("{shown}: [sweep] sizes = \"max\", but widget `{}` has no max size", m.id))?;
            sized = vec![("@max".into(), Size::Card(max.0, max.1))];
        }
        (Some(Sizes::Grid(n, k)), Some(m)) => sized = grid(m, *n, *k).into_iter().map(|(name, s)| (format!("@{name}"), Size::Card(s.0, s.1))).collect(),
        (Some(Sizes::List(l)), _) => {
            sized = l
                .iter()
                .map(|s| {
                    let (w, h) = parse_size(s)?;
                    Ok((format!("@{s}"), Size::Card(w, h)))
                })
                .collect::<Result<_, String>>()?
        }
        (Some(_), None) => unreachable!("a size sweep loads the widget"),
    }
    // named variants, then one Module put away at a time
    let mut varied: Vec<(String, Option<&toml::Table>, Option<String>)> = if b.sweep.variants.is_empty() { vec![(String::new(), None, None)] } else { b.sweep.variants.iter().map(|(n, t)| (format!("#{n}"), Some(t), None)).collect() };
    if b.sweep.each_module {
        let m = m.as_ref().expect("loaded");
        if m.modules.is_empty() {
            return Err(format!("{shown}: hide = \"each\", but widget `{}` has no Modules", m.id));
        }
        varied = varied.into_iter().flat_map(|(n, t, _)| m.modules.iter().map(move |module| (format!("{n}#no-{}", module.name), t, Some(module.name.clone())))).collect();
    }
    // extra frames of an animation: `at = ["0ms", "250ms", "settle"]`
    let frames: Vec<(String, Option<Json>)> = if b.sweep.at.is_empty() {
        vec![(String::new(), None)]
    } else {
        b.sweep.at.iter().map(|a| if a == "settle" { (if b.sweep.at.len() > 1 { "@settle".to_string() } else { String::new() }, None) } else { (format!("@{a}"), Some(Json::String(a.clone()))) }).collect()
    };
    let mut out = Vec::new();
    for (size_suffix, size) in &sized {
        for (var_suffix, table, hidden) in &varied {
            for (frame_suffix, at) in &frames {
                let mut s = b.scene.clone();
                s.target.size = size.clone();
                if let Some(t) = table {
                    apply_variant(&mut s, t, m.as_ref(), &shown)?;
                }
                if let Some(h) = hidden {
                    s.target.hide = vec![h.clone()];
                }
                if let Some(a) = at {
                    match s.pins.iter_mut().find(|(k, _)| k == "settle") {
                        Some(e) => e.1 = a.clone(),
                        None => s.pins.push(("settle".into(), a.clone())),
                    }
                }
                s.id = format!("{}{size_suffix}{var_suffix}{frame_suffix}", b.scene.id);
                out.push(s);
            }
        }
    }
    let mut seen = BTreeMap::new();
    for s in &out {
        if let Some(()) = seen.insert(s.id.clone(), ()) {
            return Err(format!("{shown}: the sweep makes the scene id `{}` twice", s.id));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        p
    }

    fn temp(name: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("wf-scene-file-{name}-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("scenes")
    }

    fn one(text: &str) -> Result<Scene, String> {
        let dir = temp("one");
        let f = write(&dir, "widgets/clock/small.scene.toml", text);
        let r = read(&f);
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
        r.map(|mut v| v.remove(0).scene)
    }

    #[test]
    fn a_scene_file_reads_every_table() {
        let s = one("format = 1\nname = \"Clock, small\"\ntags = [\"golden\"]\n[widget]\nid = \"clock\"\nsize = \"180x180\"\nparams = { ticks = false }\nitems = \"demo\"\nhover_at = [40, 22]\n[look]\nscale = 1.25\npalette = \"Midnight\"\n[env]\nnow = 2026-03-08T15:42:00\n[env.sys]\ncpu = 42\n[[env.fetch.responses]]\nurl = \"https://x.example/\"\nbody = \"{}\"\n[expect]\nflags = \"error\"\ntext = [\"12\"]\nno_text = [\"NaN\"]\n").unwrap();
        assert_eq!((s.id.as_str(), s.name.as_str(), s.tags.clone()), ("widgets/clock/small", "Clock, small", vec!["golden".to_string()]));
        assert_eq!((s.target.widget.as_str(), s.target.size.clone(), s.target.params.clone(), s.target.items.as_ref().map(Vec::len), s.target.hover_at), ("clock", Size::Card(180.0, 180.0), vec![("ticks".to_string(), Json::Bool(false))], Some(7), Some((40.0, 22.0))));
        let keys: Vec<&str> = s.pins.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["scale", "palette", "now", "sys.cpu", "fetch.responses"], "look first, then env flattened to dotted keys, a list of tables kept whole");
        assert_eq!(s.pins[2].1, Json::String("2026-03-08T15:42:00".into()), "a TOML datetime is its text");
        assert_eq!((s.expect.flags, s.expect.text, s.expect.no_text), (FlagLevel::Error, vec!["12".to_string()], vec!["NaN".to_string()]));
    }

    #[test]
    fn an_unknown_key_is_an_error_that_names_the_nearest_known_one() {
        let e = one("format = 1\n[widget]\nid = \"clock\"\nsizee = \"180x180\"\n").unwrap_err();
        assert!(e.contains("unknown key `widget.sizee` (did you mean `size`?)") && e.contains("small.scene.toml"), "{e}");
        let e = one("format = 1\n[widgt]\nid = \"clock\"\n").unwrap_err();
        assert!(e.contains("unknown key `widgt` (did you mean `widget`?)"), "{e}");
        let e = one("format = 1\n[widget]\nid = \"clock\"\n[look]\nscael = 2\n").unwrap_err();
        assert!(e.contains("`look.scael` (did you mean `scale`?)"), "{e}");
        let e = one("format = 1\n[widget]\nid = \"clock\"\n[sweep]\nsize = \"tiers\"\n").unwrap_err();
        assert!(e.contains("`sweep.size` (did you mean `sizes`?)"), "{e}");
    }

    #[test]
    fn the_format_key_is_required_and_must_be_the_one_this_build_reads() {
        assert!(one("[widget]\nid = \"clock\"\n").unwrap_err().contains("add `format = 1`"));
        assert!(one("format = 2\n[widget]\nid = \"clock\"\n").unwrap_err().contains("this build reads format = 1"));
    }

    #[test]
    fn bad_targets_are_errors_not_guesses() {
        assert!(one("format = 1\n").unwrap_err().contains("needs a [widget] table"));
        assert!(one("format = 1\n[widget]\nsize = \"10x10\"\n").unwrap_err().contains("needs `id`"));
        assert!(one("format = 1\n[widget]\nid = \"a\"\nfile = \"b.toml\"\n").unwrap_err().contains("not both"));
        assert!(one("format = 1\n[widget]\nid = \"a\"\nsize = \"big\"\n").unwrap_err().contains("WxH"));
        assert!(one("format = 1\n[widget]\nid = \"a\"\nsize = \"1x1\"\ntier = \"t\"\n").unwrap_err().contains("`size` or `tier`"));
        assert!(one("format = 1\n[expect]\n[widget]\nid = \"a\"\n[expect]\nflags = \"warm\"\n").is_err());
        assert!(one("format = 1\n[widget]\nid = \"a\"\n[expect]\nflags = \"warm\"\n").unwrap_err().contains("did you mean `warn`"));
        assert!(one("format = 1\n[settings]\nfixture = \"demo\"\n").unwrap_err().contains("`[settings]` scenes are not supported"));
        assert!(one("format = 1\n[widget]\nid = \"a\"\nedit = true\n").unwrap_err().contains("`edit = true`"));
    }

    #[test]
    fn a_scenes_file_merges_its_defaults_into_each_scene_and_ends_each_id_with_the_name() {
        let dir = temp("multi");
        let f = write(&dir, "settings/pages.scenes.toml", "format = 1\n[defaults.widget]\nid = \"clock\"\nsize = \"100x100\"\n[defaults.look]\nscale = 2\n[[scene]]\nname = \"a\"\n[[scene]]\nname = \"b\"\n[scene.widget]\nsize = \"200x100\"\n");
        let v = read(&f).unwrap();
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
        let ids: Vec<&str> = v.iter().map(|b| b.scene.id.as_str()).collect();
        assert_eq!(ids, ["settings/pages/a", "settings/pages/b"]);
        assert_eq!((v[0].scene.target.size.clone(), v[1].scene.target.size.clone()), (Size::Card(100.0, 100.0), Size::Card(200.0, 100.0)));
        assert_eq!(v[1].scene.pins, [("scale".to_string(), Json::from(2.0))], "the default look reaches every scene");
        let dir = temp("multi-bad");
        let f = write(&dir, "p.scenes.toml", "format = 1\n[[scene]]\nname = \"a b\"\n[scene.widget]\nid = \"clock\"\n");
        assert!(read(&f).unwrap_err().contains("part of the scene id"));
        let f = write(&dir, "q.scenes.toml", "format = 1\n[[scene]]\nname = \"a\"\n[scene.widget]\nid = \"clock\"\nsiz = \"1x1\"\n");
        assert!(read(&f).unwrap_err().contains("did you mean `size`"));
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    fn widgets() -> crate::widgets::Registry {
        crate::widgets::Registry::builtin()
    }

    fn meta_of(id: &str) -> WidgetMeta {
        widgets().get(id).and_then(|d| d.as_ref().ok()).map(|w| w.meta().clone()).expect(id)
    }

    fn expand_text(text: &str) -> Result<Vec<Scene>, String> {
        let dir = temp("expand");
        let f = write(&dir, "x.scene.toml", text);
        let b = read(&f).unwrap().remove(0);
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
        expand(&b, &mut |s| Ok(meta_of(&s.target.widget)))
    }

    #[test]
    fn a_tier_sweep_makes_one_scene_per_tier_with_the_tier_in_the_id() {
        let v = expand_text("format = 1\n[widget]\nid = \"system_monitor\"\n[sweep]\nsizes = \"tiers\"\n").unwrap();
        let got: Vec<(&str, &Size)> = v.iter().map(|s| (s.id.as_str(), &s.target.size)).collect();
        assert_eq!(got, [("x@compact", &Size::Tier("compact".into())), ("x@normal", &Size::Tier("normal".into())), ("x@large", &Size::Tier("large".into()))]);
        assert!(expand_text("format = 1\n[widget]\nid = \"clock\"\n[sweep]\nsizes = \"tiers\"\n").unwrap_err().contains("declares no tiers"));
    }

    #[test]
    fn the_grid_sweep_walks_min_to_max_like_fits_did_and_variants_name_their_overrides() {
        let v = expand_text("format = 1\n[widget]\nid = \"clock\"\n[sweep]\nsizes = \"grid:4x4\"\n[sweep.variants]\ndefaults = {}\nall_on = { params = \"bools:true\" }\n").unwrap();
        assert_eq!(v.len(), 17 * 2);
        assert_eq!(v[0].id, "x@default#defaults");
        assert_eq!(v[1].id, "x@default#all_on");
        let last = v.last().unwrap();
        assert_eq!((last.id.as_str(), &last.target.size), ("x@3,3#all_on", &Size::Card(760.0, 560.0)), "the far corner of the grid is the max size");
        assert_eq!(last.target.params.len(), 3, "every Bool param of the clock is on");
        assert!(last.target.params.iter().all(|(_, v)| v == &Json::Bool(true)));
        assert!(v[0].target.params.is_empty());
    }

    #[test]
    fn hiding_each_module_and_frame_times_fan_out_too() {
        let v = expand_text("format = 1\n[widget]\nid = \"system_monitor\"\nhide = \"each\"\n[sweep]\nat = [\"0ms\", \"settle\"]\n").unwrap();
        let ids: Vec<&str> = v.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"x#no-gauge@0ms") && ids.contains(&"x#no-footer@settle"), "{ids:?}");
        assert_eq!(v[0].target.hide, ["gauge"]);
        assert!(v[0].pins.iter().any(|(k, val)| k == "settle" && val == &Json::String("0ms".into())));
        let one = expand_text("format = 1\n[widget]\nid = \"clock\"\n[sweep]\nat = [\"settle\"]\n").unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].id, "x", "a lone `settle` is the default frame");
    }

    #[test]
    fn a_sweep_of_explicit_sizes_needs_no_widget() {
        let dir = temp("explicit");
        let f = write(&dir, "x.scene.toml", "format = 1\n[widget]\nid = \"clock\"\n[sweep]\nsizes = [\"180x180\", \"330x400\"]\n");
        let b = read(&f).unwrap().remove(0);
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
        assert!(!needs_widget(&b));
        let v = expand(&b, &mut |_| panic!("not needed")).unwrap();
        assert_eq!(v.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["x@180x180", "x@330x400"]);
    }

    #[test]
    fn a_scene_root_is_the_nearest_scenes_folder() {
        assert_eq!(scene_root(Path::new("/a/scenes/widgets/x.scene.toml")), PathBuf::from("/a/scenes").canonicalize_lossy());
        assert_eq!(scene_root(Path::new("/a/b/x.scene.toml")), PathBuf::from("/a/b").canonicalize_lossy());
    }

    trait Lossy {
        fn canonicalize_lossy(self) -> PathBuf;
    }

    impl Lossy for PathBuf {
        fn canonicalize_lossy(self) -> PathBuf {
            std::path::absolute(&self).unwrap_or(self)
        }
    }
}
