//! Widget definition files (TOML) -> element AST -> `ui::Node` tree (decisions 8, 11, 12, 14).
//! Authored by hand, so unknown names are rejected with a "did you mean".

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use taffy::prelude::*;

use crate::anim::Ease;
use crate::color::{Color, MAGENTA};
use crate::elements;
use crate::expr::{Scope, Template};
use crate::modules::{Arrange, Arrangement, Each, Inst, ModuleDef, ModuleSet, Placed, PlacedSlot, SlotDef, TierDef, place};
use crate::theme::Theme;
use crate::ui::*;
use crate::value::Value;
use crate::widgets::{Built, Choice, ExpandInfo, Inputs, ModuleMeta, ParamDef, ParamType, Seed, TierMeta, WidgetMeta};


#[derive(Clone, Debug)]
pub enum Attr {
    Lit(Value),
    Tpl(Template),
    Token(String),
    List(Vec<Attr>),
    Table(BTreeMap<String, Attr>),
}

fn parse_attr(v: &toml::Value, path: &str) -> Result<Attr, String> {
    Ok(match v {
        toml::Value::String(s) => {
            if s.contains('{') || s.contains('}') {
                let t = Template::parse(s).map_err(|e| format!("{path}: {e} in `{s}`"))?;
                if t.is_literal() { Attr::Lit(Value::Str(s.replace("{{", "{").replace("}}", "}"))) } else { Attr::Tpl(t) }
            } else if let Some(n) = s.strip_prefix('$').filter(|n| !n.is_empty() && !n.contains(char::is_whitespace)) {
                Attr::Token(n.to_string())
            } else {
                Attr::Lit(Value::Str(s.clone()))
            }
        }
        toml::Value::Array(a) => Attr::List(a.iter().enumerate().map(|(i, x)| parse_attr(x, &format!("{path}[{i}]"))).collect::<Result<_, _>>()?),
        toml::Value::Table(t) => Attr::Table(t.iter().map(|(k, x)| Ok((k.clone(), parse_attr(x, &format!("{path}.{k}"))?))).collect::<Result<_, String>>()?),
        other => Attr::Lit(Value::from(other)),
    })
}

const COMMON_ATTRS: &[&str] = &[
    "id", "width", "height", "min_width", "min_height", "max_width", "max_height", "grow", "shrink", "basis", "direction", "wrap", "align",
    "justify", "align_self", "gap", "padding", "margin", "position", "inset", "left", "top", "right", "bottom", "aspect", "fill", "fill_alpha", "border",
    "border_color", "radius", "opacity", "shadow", "clip", "on_click", "on_drop", "hover", "transition", "enter", "scroll", "scroll_x", "overlay", "hit",
];
/// `repeat` is structural, not an element kind.
const REPEAT_ATTRS: &[&str] = &["for", "as", "index"];
/// `slot` is structural too: a box the user's arranged Modules fill.
/// `max`: how many Modules fit, so the rest are left out rather than overflowing the card.
const SLOT_ATTRS: &[&str] = &["slot", "max"];

fn type_names() -> Vec<&'static str> {
    elements::KINDS.iter().map(|k| k.name).chain(["repeat", "slot"]).collect()
}

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i];
        for j in 1..=b.len() {
            cur.push((prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1])));
        }
        prev = cur;
    }
    prev[b.len()]
}

pub(crate) fn suggest(name: &str, pool: &[&[&str]]) -> String {
    pool.iter()
        .flat_map(|p| p.iter())
        .map(|c| (edit_distance(name, c), *c))
        .filter(|(d, _)| *d <= 2)
        .min()
        .map(|(_, c)| format!(" (did you mean `{c}`?)"))
        .unwrap_or_default()
}

#[derive(Clone, Debug)]
pub struct Elem {
    pub ty: String,
    pub attrs: BTreeMap<String, Attr>,
    pub children: Vec<Elem>,
    pub when: Option<Template>,
}

fn parse_elem(t: &toml::Table, path: &str) -> Result<Elem, String> {
    let ty = t.get("type").and_then(|v| v.as_str()).unwrap_or("box").to_string();
    let specific: &[&str] = match elements::find(&ty) {
        Some(k) => k.own_attrs,
        None if ty == "repeat" => REPEAT_ATTRS,
        None if ty == "slot" => SLOT_ATTRS,
        None => {
            let types = type_names();
            return Err(format!("{path}: unknown type `{ty}`{} (expected one of {})", suggest(&ty, &[&types]), types.join(", ")));
        }
    };
    let (mut attrs, mut children, mut when) = (BTreeMap::new(), Vec::new(), None);
    for (k, v) in t {
        match k.as_str() {
            "type" => {}
            "when" => {
                let s = v.as_str().ok_or_else(|| format!("{path}.when: expected a string like \"{{state.open}}\""))?;
                when = Some(Template::parse(s).map_err(|e| format!("{path}.when: {e}"))?);
            }
            "children" => {
                let arr = v.as_array().ok_or_else(|| format!("{path}.children: expected an array of tables"))?;
                for (i, c) in arr.iter().enumerate() {
                    let ct = c.as_table().ok_or_else(|| format!("{path}.children[{i}]: expected a table"))?;
                    children.push(parse_elem(ct, &format!("{path}.children[{i}]"))?);
                }
            }
            k if COMMON_ATTRS.contains(&k) || specific.contains(&k) => {
                attrs.insert(k.to_string(), parse_attr(v, &format!("{path}.{k}"))?);
            }
            k => return Err(format!("{path}: unknown attribute `{k}` on `{ty}`{}", suggest(k, &[COMMON_ATTRS, specific]))),
        }
    }
    Ok(Elem { ty, attrs, children, when })
}


#[derive(Clone, Debug)]
pub struct Expand {
    pub when: Template,
    pub width: Option<Template>,
    pub height: Option<Template>,
}

#[derive(Clone, Debug)]
pub struct WidgetDef {
    pub meta: WidgetMeta,
    pub expand: Option<Expand>,
    pub root: Elem,
    /// Tiers, slots and Modules, when the file declares them.
    pub modules: Option<ModuleSet>,
    /// Where the file lives; `None` for a built-in.
    pub base: Option<Base>,
}

/// A definition file's folder, and the content root its `./` paths must stay inside.
#[derive(Clone, Debug, PartialEq)]
pub struct Base {
    pub dir: PathBuf,
    pub root: PathBuf,
}

impl Base {
    /// `rel` against the file's folder, resolved without touching the disk; `None` if it
    /// leaves the root (or is absolute).
    pub fn resolve(&self, rel: &str) -> Option<PathBuf> {
        let mut out = self.dir.clone();
        for c in Path::new(rel).components() {
            match c {
                Component::CurDir => {}
                Component::ParentDir => {
                    if !out.pop() {
                        return None;
                    }
                }
                Component::Normal(part) => out.push(part),
                Component::RootDir | Component::Prefix(_) => return None,
            }
        }
        out.starts_with(&self.root).then_some(out)
    }
}

const TOP: &[&str] = &["name", "description", "size", "min_size", "max_size", "needs", "params", "state", "expand", "tiers", "slots", "modules", "root"];

fn pair(v: Option<&toml::Value>, default: (f32, f32), what: &str) -> Result<(f32, f32), String> {
    let Some(v) = v else { return Ok(default) };
    let a = v.as_array().filter(|a| a.len() == 2).ok_or_else(|| format!("{what}: expected [width, height]"))?;
    let n = |x: &toml::Value| x.as_float().or_else(|| x.as_integer().map(|i| i as f64)).map(|f| f as f32);
    Ok((n(&a[0]).ok_or_else(|| format!("{what}: width is not a number"))?, n(&a[1]).ok_or_else(|| format!("{what}: height is not a number"))?))
}

/// A `[params]` table, in file order. Also parses the style schema (`assets/style.toml`).
/// `"fast"`, or `{ value = "fast", label = "Fast (30 fps)" }`.
fn choice(param: &str, c: &toml::Value) -> Result<Choice, String> {
    let bad = || format!("params.{param}.choices: each is a string or {{ value = \"...\", label = \"...\" }}");
    match c {
        toml::Value::String(s) => Ok(Choice { value: s.clone(), label: s.clone() }),
        toml::Value::Table(t) => {
            let value = t.get("value").and_then(|v| v.as_str()).ok_or_else(bad)?.to_string();
            let label = t.get("label").and_then(|v| v.as_str()).map_or_else(|| value.clone(), String::from);
            Ok(Choice { value, label })
        }
        _ => Err(bad()),
    }
}

pub fn parse_params(pt: &toml::Table) -> Result<Vec<ParamDef>, String> {
    let mut params = Vec::new();
    for (name, v) in pt {
        let p = v.as_table().ok_or_else(|| format!("params.{name}: expected a table"))?;
        let ty = p.get("type").and_then(|v| v.as_str()).ok_or_else(|| format!("params.{name}: missing `type`"))?;
        let ty = ParamType::parse(ty).ok_or_else(|| format!("params.{name}: unknown param type `{ty}`"))?;
        let f = |k: &str| p.get(k).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)));
        let seed = match p.get("seed") {
            None => None,
            Some(v) => {
                let s = v.as_str().ok_or_else(|| format!("params.{name}.seed: expected a string"))?;
                let ids: Vec<&str> = Seed::ALL.iter().map(|x| x.id()).collect();
                let seed = Seed::parse(s).ok_or_else(|| format!("params.{name}: unknown seed `{s}`{}", suggest(s, &[&ids])))?;
                if seed.param_type() != ty {
                    return Err(format!("params.{name}: seed `{s}` needs type = \"{}\"", seed.param_type().id()));
                }
                Some(seed)
            }
        };
        params.push(ParamDef {
            name: name.clone(),
            ty,
            default: p.get("default").map(Value::from).unwrap_or(match ty {
                ParamType::Bool => Value::Bool(false),
                ParamType::Number | ParamType::Duration => Value::Num(0.0),
                ParamType::Shortcuts => Value::List(vec![]),
                _ => Value::Str(String::new()),
            }),
            label: p.get("label").and_then(|v| v.as_str()).unwrap_or(name).to_string(),
            help: p.get("help").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            min: f("min"),
            max: f("max"),
            step: f("step"),
            choices: match p.get("choices") {
                None => vec![],
                Some(v) => v.as_array().ok_or_else(|| format!("params.{name}.choices: expected a list"))?.iter().map(|c| choice(name, c)).collect::<Result<_, _>>()?,
            },
            seed,
            group: p.get("group").and_then(|v| v.as_str()).map(str::trim).filter(|g| !g.is_empty()).map(String::from),
            module: p.get("module").and_then(|v| v.as_str()).map(String::from),
        });
    }
    Ok(params)
}

fn table_of<'a>(v: &'a toml::Value, what: &str) -> Result<&'a toml::Table, String> {
    v.as_table().ok_or_else(|| format!("{what}: expected a table"))
}

fn only_keys(t: &toml::Table, allowed: &[&str], what: &str) -> Result<(), String> {
    match t.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(format!("{what}: unknown key `{k}`{}", suggest(k, &[allowed]))),
        None => Ok(()),
    }
}

fn tpl(t: &toml::Table, k: &str, what: &str) -> Result<Option<Template>, String> {
    t.get(k)
        .map(|v| {
            let s = v.as_str().ok_or_else(|| format!("{what}.{k}: expected a string"))?;
            Template::parse(s).map_err(|e| format!("{what}.{k}: {e}"))
        })
        .transpose()
}

fn names(t: &toml::Table, k: &str, what: &str) -> Result<Vec<String>, String> {
    match t.get(k) {
        None => Ok(vec![]),
        Some(v) => v.as_array().and_then(|a| a.iter().map(|x| x.as_str().map(String::from)).collect()).ok_or_else(|| format!("{what}.{k}: expected a list of names")),
    }
}

/// `[tiers.*]`, `[slots.*]` and `[modules.*]`: what a user may arrange, and how.
fn parse_modules(t: &toml::Table) -> Result<Option<ModuleSet>, String> {
    if !["tiers", "slots", "modules"].iter().any(|k| t.contains_key(*k)) {
        return Ok(None);
    }
    let sub = |k: &str| t.get(k).map(|v| table_of(v, k)).transpose();
    let mut set = ModuleSet::default();
    if let Some(tiers) = sub("tiers")? {
        for (name, v) in tiers {
            let what = format!("tiers.{name}");
            let tt = table_of(v, &what)?;
            only_keys(tt, &["label", "size", "when", "layout"], &what)?;
            let layout = match tt.get("layout") {
                None => BTreeMap::new(),
                Some(l) => table_of(l, &format!("{what}.layout"))?
                    .iter()
                    .map(|(slot, v)| Ok((slot.clone(), v.as_array().and_then(|a| a.iter().map(|x| x.as_str().map(String::from)).collect::<Option<Vec<_>>>()).ok_or_else(|| format!("{what}.layout.{slot}: expected a list of module names"))?)))
                    .collect::<Result<_, String>>()?,
            };
            set.tiers.push(TierDef {
                name: name.clone(),
                label: tt.get("label").and_then(|v| v.as_str()).map_or_else(|| name.clone(), String::from),
                when: tpl(tt, "when", &what)?,
                size: pair(tt.get("size"), (300.0, 200.0), &format!("{what}.size"))?,
                layout,
            });
        }
    }
    if let Some(slots) = sub("slots")? {
        for (name, v) in slots {
            let what = format!("slots.{name}");
            let st = table_of(v, &what)?;
            only_keys(st, &["label"], &what)?;
            set.slots.push(SlotDef { name: name.clone(), label: st.get("label").and_then(|v| v.as_str()).map_or_else(|| name.clone(), String::from) });
        }
    }
    if let Some(modules) = sub("modules")? {
        for (name, v) in modules {
            let what = format!("modules.{name}");
            let mut body = table_of(v, &what)?.clone();
            let label = match body.remove("label") {
                Some(toml::Value::String(s)) => Template::parse(&s).map_err(|e| format!("{what}.label: {e}"))?,
                Some(_) => return Err(format!("{what}.label: expected a string")),
                None => Template::parse(&name.replace('{', "{{").replace('}', "}}")).map_err(|e| format!("{what}: {e}"))?,
            };
            let mut head = toml::Table::new();
            for k in ["slots", "legacy", "when", "for", "as", "key"] {
                if let Some(x) = body.remove(k) {
                    head.insert(k.into(), x);
                }
            }
            let each = tpl(&head, "for", &what)?.map(|list| -> Result<Each, String> {
                Ok(Each { list, var: head.get("as").and_then(|v| v.as_str()).unwrap_or("item").to_string(), key: tpl(&head, "key", &what)? })
            });
            set.modules.push(ModuleDef {
                name: name.clone(),
                label,
                slots: names(&head, "slots", &what)?,
                legacy: match head.get("legacy") {
                    None => BTreeMap::new(),
                    Some(l) => table_of(l, &format!("{what}.legacy"))?.iter().map(|(e, p)| Ok((e.clone(), p.as_str().ok_or_else(|| format!("{what}.legacy.{e}: expected a param name"))?.to_string()))).collect::<Result<_, String>>()?,
                },
                when: tpl(&head, "when", &what)?,
                each: each.transpose()?,
                body: parse_elem(&body, &format!("modules.{name}"))?,
            });
        }
    }
    let slot_names: Vec<&str> = set.slots.iter().map(|s| s.name.as_str()).collect();
    let module_names: Vec<&str> = set.modules.iter().map(|m| m.name.as_str()).collect();
    if set.tiers.is_empty() {
        return Err("modules: declare at least one [tiers.<name>]".into());
    }
    for m in &set.modules {
        if let Some(bad) = m.slots.iter().find(|s| !slot_names.contains(&s.as_str())) {
            return Err(format!("modules.{}.slots: unknown slot `{bad}`{}", m.name, suggest(bad, &[&slot_names])));
        }
    }
    for tier in &set.tiers {
        for (slot, entries) in &tier.layout {
            if !slot_names.contains(&slot.as_str()) {
                return Err(format!("tiers.{}.layout: unknown slot `{slot}`{}", tier.name, suggest(slot, &[&slot_names])));
            }
            if let Some(bad) = entries.iter().find(|e| !module_names.contains(&e.split(':').next().unwrap_or(""))) {
                return Err(format!("tiers.{}.layout.{slot}: unknown module `{bad}`{}", tier.name, suggest(bad, &[&module_names])));
            }
        }
    }
    Ok(Some(set))
}

impl WidgetDef {
    pub fn parse(id: &str, src: &str) -> Result<WidgetDef, String> {
        let t: toml::Table = src.parse().map_err(|e| format!("{e}"))?;
        for k in t.keys() {
            if !TOP.contains(&k.as_str()) {
                return Err(format!("unknown top-level key `{k}`{}", suggest(k, &[TOP])));
            }
        }
        let text = |k: &str| t.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let params = match t.get("params").and_then(|v| v.as_table()) {
            Some(pt) => parse_params(pt)?,
            None => Vec::new(),
        };
        let state = t.get("state").and_then(|v| v.as_table()).map(|s| s.iter().map(|(k, v)| (k.clone(), Value::from(v))).collect()).unwrap_or_default();
        let expand = match t.get("expand").and_then(|v| v.as_table()) {
            None => None,
            Some(e) => {
                let tpl = |k: &str| -> Result<Option<Template>, String> {
                    e.get(k).and_then(|v| v.as_str()).map(|s| Template::parse(s).map_err(|er| format!("expand.{k}: {er}"))).transpose()
                };
                Some(Expand { when: tpl("when")?.ok_or("expand: missing `when`")?, width: tpl("width")?, height: tpl("height")? })
            }
        };
        let modules = parse_modules(&t)?;
        if let Some(bad) = params.iter().filter_map(|p| p.module.as_deref()).flat_map(|m| m.split(',')).map(str::trim).find(|m| !modules.as_ref().is_some_and(|ms| ms.modules.iter().any(|d| d.name == m.split(':').next().unwrap_or("")))) {
            return Err(format!("params: `module = \"{bad}\"` names a module this widget does not declare"));
        }
        let root_t = t.get("root").and_then(|v| v.as_table()).ok_or("missing [root] table")?;
        let min_card_size = pair(t.get("min_size"), (48.0, 48.0), "min_size")?;
        let max_card_size = t.get("max_size").map(|v| pair(Some(v), (0.0, 0.0), "max_size")).transpose()?;
        if max_card_size.is_some_and(|m| m.0 < min_card_size.0 || m.1 < min_card_size.1) {
            return Err("max_size is smaller than min_size".into());
        }
        let meta = WidgetMeta {
            id: id.to_string(),
            name: if text("name").is_empty() { id.to_string() } else { text("name") },
            description: text("description"),
            default_card_size: pair(t.get("size"), (200.0, 120.0), "size")?,
            min_card_size,
            max_card_size,
            params,
            initial_state: state,
            needs: match t.get("needs") {
                None => vec![],
                Some(v) => v.as_array().and_then(|a| a.iter().map(|x| x.as_str().map(String::from)).collect()).ok_or("needs must be a list of data source names")?,
            },
            tiers: modules.iter().flat_map(|m| &m.tiers).map(|t| TierMeta { name: t.name.clone(), label: t.label.clone(), size: t.size, layout: t.layout.clone() }).collect(),
            slots: modules.iter().flat_map(|m| &m.slots).map(|s| (s.name.clone(), s.label.clone())).collect(),
            modules: modules
                .iter()
                .flat_map(|m| &m.modules)
                .map(|d| ModuleMeta {
                    name: d.name.clone(),
                    label: if d.each.is_some() || !d.label.is_literal() { d.name.clone() } else { d.label.eval(&Scope::default()).map(|v| v.to_string()).unwrap_or_else(|_| d.name.clone()) },
                    slots: d.slots.clone(),
                    legacy: d.legacy.clone(),
                })
                .collect(),
        };
        Ok(WidgetDef { meta, expand, root: parse_elem(root_t, "root")?, modules, base: None })
    }
}


struct TreeBuilder<'a> {
    theme: &'a Theme,
    base: Option<&'a Base>,
    scope: Scope<'a>,
    warns: Vec<String>,
    images: BTreeSet<String>,
    image_size: &'a dyn Fn(&str) -> Option<(f32, f32)>,
    modules: Option<&'a ModuleSet>,
    /// Every Module (item of a `for` Module) that exists now.
    insts: Vec<Inst>,
    /// slot -> indexes into `insts`, for the current tier.
    placed: BTreeMap<String, Vec<usize>>,
    tier: String,
    /// The slots built so far, in tree order.
    built: Vec<PlacedSlot>,
    /// Settings preview: Modules are the only hit targets.
    preview: bool,
}

pub fn build(def: &WidgetDef, inp: &Inputs, theme: &Theme, image_size: &dyn Fn(&str) -> Option<(f32, f32)>) -> Result<Built, String> {
    let card = inp.card_size;
    let mut scope = Scope::with_provider(inp.read_source);
    let obj = |m: &BTreeMap<String, Value>| Value::Obj(m.clone());
    scope.set("param", obj(&def.meta.effective_params(inp.params)));
    let mut st = def.meta.initial_state.clone();
    st.extend(inp.state.clone());
    scope.set("state", obj(&st));
    scope.set("self", Value::obj([("w", (card.0 as f64).into()), ("h", (card.1 as f64).into())]));
    let mut b = TreeBuilder {
        theme,
        base: def.base.as_ref(),
        scope,
        warns: vec![],
        images: BTreeSet::new(),
        image_size,
        modules: def.modules.as_ref(),
        insts: vec![],
        placed: BTreeMap::new(),
        tier: String::new(),
        built: vec![],
        preview: inp.arrange.is_some_and(|a| a.preview),
    };
    if let Some(ms) = &def.modules {
        b.arrange(ms, inp.arrange)?;
    }
    let mut nodes = b.build_elem(&def.root, inp.key_prefix)?;
    let mut root = nodes.pop().ok_or("root produced no node")?;
    root.style.size = Size { width: length(card.0), height: length(card.1) };
    let expand = match &def.expand {
        None => None,
        Some(e) => {
            let active = e.when.eval(&b.scope).map_err(|x| format!("expand.when: {x}"))?.truthy();
            let dim = |t: &Option<Template>, b: &TreeBuilder| -> Result<Option<f32>, String> {
                t.as_ref().map(|t| t.eval(&b.scope).map_err(|x| format!("expand: {x}")).map(|v| v.as_f64().unwrap_or(0.0) as f32)).transpose()
            };
            Some(ExpandInfo { active, width: dim(&e.width, &b)?, height: dim(&e.height, &b)? })
        }
    };
    let arrangement = b.arrangement();
    Ok(Built { root, deps: b.scope.deps(), image_ids: b.images, warnings: b.warns, expand, arrangement })
}

/// Decision 14: a broken widget shows this in place, never a silent skip.
pub fn error_card(msg: &str, size: (f32, f32), theme: &Theme) -> Node {
    let danger = theme.color("danger");
    let head = Node::text("e0", "Widget error", 13.0, danger).with_text(|t| t.weight = 700);
    let body = Node::text("e1", msg.to_string(), 11.5, theme.color("text")).with_text(|t| t.wrap = true);
    let mut card = Node::new("err").wh(size.0, size.1).col().gap(4.0).pad(10.0).fill(Color([0.08, 0.02, 0.03, 0.92])).radius(10.0).border(2.0, danger).child(head).child(body);
    card.clip = true;
    card
}

/// Resolves tokens and bindings; bad values become warnings, not errors.
pub struct Attrs<'r, 'a> {
    b: &'r mut TreeBuilder<'a>,
    e: &'r Elem,
    path: &'r str,
}

impl<'a> Attrs<'_, 'a> {
    pub fn theme(&self) -> &'a Theme {
        self.b.theme
    }

    pub fn value(&mut self, k: &str) -> Result<Option<Value>, String> {
        self.b.get(self.e, k, self.path)
    }

    pub fn num(&mut self, k: &str) -> Result<Option<f32>, String> {
        self.b.num(self.e, k, self.path)
    }

    pub fn flag(&mut self, k: &str) -> Result<Option<bool>, String> {
        self.b.flag(self.e, k, self.path)
    }

    pub fn text(&mut self, k: &str) -> Result<Option<String>, String> {
        self.b.text(self.e, k, self.path)
    }

    pub fn color(&mut self, k: &str) -> Result<Option<Color>, String> {
        self.b.color(self.e, k, self.path)
    }

    /// An image `src`: `./x.png` is a file next to the definition, inside its content root.
    pub fn image_id(&mut self, src: &str) -> String {
        // a full path, as a folder listing gives it (`src = "{item.path}"`)
        if Path::new(src).is_absolute() {
            return format!("file:{src}");
        }
        if !src.starts_with("./") {
            return src.to_string();
        }
        match self.b.base.map(|b| b.resolve(src)) {
            Some(Some(p)) => format!("file:{}", p.display()),
            Some(None) => {
                self.b.warn(format!("{}.src: `{src}` leaves the widget's folder", self.path));
                String::new()
            }
            None => {
                self.b.warn(format!("{}.src: `{src}` needs a widget file on disk", self.path));
                String::new()
            }
        }
    }

    /// 32x32 until the image is uploaded.
    pub fn request_image(&mut self, id: &str) -> (f32, f32) {
        self.b.images.insert(id.to_string());
        (self.b.image_size)(id).unwrap_or((32.0, 32.0))
    }
}

impl<'a> TreeBuilder<'a> {
    /// Picks the tier, lists the Modules that exist, and places them per the layout.
    fn arrange(&mut self, ms: &'a ModuleSet, a: Option<Arrange>) -> Result<(), String> {
        let forced = a.and_then(|a| a.tier).and_then(|n| ms.tiers.iter().find(|t| t.name == n));
        let tier = match forced {
            Some(t) => t,
            None => {
                let mut hit = None;
                for t in &ms.tiers {
                    if let Some(w) = &t.when {
                        if w.eval(&self.scope).map_err(|e| format!("tiers.{}.when: {e}", t.name))?.truthy() {
                            hit = Some(t);
                            break;
                        }
                    }
                }
                hit.or_else(|| ms.tiers.iter().find(|t| t.when.is_none())).unwrap_or(&ms.tiers[ms.tiers.len() - 1])
            }
        };
        self.tier = tier.name.clone();
        self.scope.set("tier", Value::Str(tier.name.clone()));
        for (mi, m) in ms.modules.iter().enumerate() {
            let what = format!("modules.{}", m.name);
            let Some(e) = &m.each else {
                if let Some(inst) = self.instance(m, mi, None, 0, &what)? {
                    self.insts.push(inst);
                }
                continue;
            };
            let list = match e.list.eval(&self.scope).map_err(|x| format!("{what}.for: {x}"))? {
                Value::List(l) => l,
                Value::Nil => vec![],
                other => return Err(format!("{what}.for: expected a list, got `{other}`")),
            };
            for (i, item) in list.into_iter().enumerate() {
                self.scope.set(&e.var, item.clone());
                self.scope.set("index", Value::Num(i as f64));
                let inst = self.instance(m, mi, Some(item), i, &what);
                self.scope.pop();
                self.scope.pop();
                if let Some(inst) = inst? {
                    self.insts.push(inst);
                }
            }
        }
        let user = a.and_then(|a| a.layout.get(&tier.name));
        self.placed = place(ms, &self.insts, user.unwrap_or(&tier.layout));
        Ok(())
    }

    /// The Module (or one item of it), unless its `when` says it does not exist.
    fn instance(&self, m: &ModuleDef, module: usize, item: Option<Value>, i: usize, what: &str) -> Result<Option<Inst>, String> {
        if let Some(w) = &m.when {
            if !w.eval(&self.scope).map_err(|e| format!("{what}.when: {e}"))?.truthy() {
                return Ok(None);
            }
        }
        let label = m.label.eval(&self.scope).map_err(|e| format!("{what}.label: {e}"))?.to_string();
        let id = match m.each.as_ref().map(|e| &e.key) {
            None => m.name.clone(),
            Some(None) => format!("{}:{i}", m.name),
            Some(Some(k)) => format!("{}:{}", m.name, k.eval(&self.scope).map_err(|e| format!("{what}.key: {e}"))?),
        };
        Ok(Some(Inst { id, module, label, item }))
    }

    fn arrangement(&self) -> Option<Arrangement> {
        let ms = self.modules?;
        let placed: BTreeSet<&str> = self.built.iter().flat_map(|s| s.modules.iter().chain(&s.cut).map(|m| m.id.as_str())).collect();
        let hidden = self
            .insts
            .iter()
            .filter(|i| !placed.contains(i.id.as_str()))
            .filter(|i| self.built.iter().any(|s| ms.modules[i.module].fits(&s.name)))
            .map(|i| Placed { id: i.id.clone(), module: ms.modules[i.module].name.clone(), label: i.label.clone(), key: String::new() })
            .collect();
        Some(Arrangement { tier: self.tier.clone(), slots: self.built.clone(), hidden })
    }

    fn slot(&mut self, e: &Elem, key: &str) -> Result<Vec<Node>, String> {
        let path = key.to_string();
        let ms = self.modules.ok_or_else(|| format!("{path}: `slot` needs the file to declare [tiers], [slots] and [modules]"))?;
        let name = self.text(e, "slot", &path)?.ok_or_else(|| format!("{path}: a slot element needs `slot = \"name\"`"))?;
        let Some(def) = ms.slots.iter().find(|s| s.name == name) else {
            let known: Vec<&str> = ms.slots.iter().map(|s| s.name.as_str()).collect();
            return Err(format!("{path}: unknown slot `{name}`{}", suggest(&name, &[&known])));
        };
        if self.built.iter().any(|s| s.name == name) {
            return Err(format!("{path}: slot `{name}` is used twice"));
        }
        let mut n = Node::new(key);
        self.layout(e, &mut n, &path)?;
        self.look(e, &mut n, &path)?;
        self.interact(e, &mut n, &path)?;
        let room = self.num(e, "max", &path)?.map_or(usize::MAX, |m| m.max(0.0) as usize);
        let (mut modules, mut cut) = (Vec::new(), Vec::new());
        for (pos, i) in self.placed.get(&name).cloned().unwrap_or_default().into_iter().enumerate() {
            let inst = self.insts[i].clone();
            let m = &ms.modules[inst.module];
            let mkey = format!("{key}/m:{}", inst.id);
            if pos >= room {
                cut.push(Placed { id: inst.id.clone(), module: m.name.clone(), label: inst.label.clone(), key: String::new() });
                continue;
            }
            let mut pushed = 1;
            if let Some(each) = &m.each {
                self.scope.set(&each.var, inst.item.clone().unwrap_or(Value::Nil));
                pushed += 1;
            }
            self.scope.set("index", Value::Num(pos as f64)); // the place in the slot, for `enter` staggers
            self.scope.set("slot", Value::Str(name.clone()));
            let built = self.build_elem(&m.body, &mkey);
            for _ in 0..pushed + 1 {
                self.scope.pop();
            }
            let mut nodes = built?;
            if self.preview {
                for nd in &mut nodes {
                    nd.action = Some(format!("mod:{}", inst.id));
                    nd.hit_testable = true;
                }
            }
            n.children.extend(nodes);
            modules.push(Placed { id: inst.id.clone(), module: m.name.clone(), label: inst.label.clone(), key: mkey });
        }
        self.built.push(PlacedSlot { name, label: def.label.clone(), key: key.to_string(), modules, cut });
        Ok(vec![n])
    }

    fn warn(&mut self, m: String) {
        if !self.warns.contains(&m) {
            self.warns.push(m);
        }
    }

    fn resolve_token(&mut self, v: Value, ctx: &str) -> Value {
        let mut v = v;
        for _ in 0..4 {
            let Value::Str(s) = &v else { break };
            let Some(name) = s.strip_prefix('$').filter(|n| !n.is_empty() && !n.contains(char::is_whitespace)) else { break };
            match self.theme.get(name) {
                Some(t) => v = t.clone(),
                None => {
                    self.warn(format!("{ctx}: undefined token ${name}"));
                    return Value::Str("#ff00ff".into());
                }
            }
        }
        v
    }

    fn eval_attr(&mut self, a: &Attr, ctx: &str) -> Result<Value, String> {
        Ok(match a {
            Attr::Lit(v) => self.resolve_token(v.clone(), ctx),
            Attr::Token(n) => self.resolve_token(Value::Str(format!("${n}")), ctx),
            Attr::Tpl(t) => {
                let v = t.eval(&self.scope).map_err(|e| format!("{ctx}: {e}"))?;
                self.resolve_token(v, ctx)
            }
            Attr::List(l) => Value::List(l.iter().map(|x| self.eval_attr(x, ctx)).collect::<Result<_, _>>()?),
            Attr::Table(_) => return Err(format!("{ctx}: expected a value, found a table")),
        })
    }

    fn get(&mut self, e: &Elem, k: &str, path: &str) -> Result<Option<Value>, String> {
        e.attrs.get(k).map(|a| self.eval_attr(a, &format!("{path}.{k}"))).transpose()
    }

    fn num(&mut self, e: &Elem, k: &str, path: &str) -> Result<Option<f32>, String> {
        Ok(self.get(e, k, path)?.and_then(|v| v.as_f64()).map(|x| x as f32))
    }

    fn flag(&mut self, e: &Elem, k: &str, path: &str) -> Result<Option<bool>, String> {
        Ok(self.get(e, k, path)?.map(|v| v.truthy()))
    }

    fn text(&mut self, e: &Elem, k: &str, path: &str) -> Result<Option<String>, String> {
        Ok(self.get(e, k, path)?.map(|v| v.to_string()))
    }

    fn color_of(&mut self, v: &Value, ctx: &str) -> Color {
        match v {
            Value::Str(s) => Color::parse(s).unwrap_or_else(|| {
                self.warn(format!("{ctx}: `{s}` is not a colour"));
                MAGENTA
            }),
            _ => {
                self.warn(format!("{ctx}: expected a colour"));
                MAGENTA
            }
        }
    }

    fn color(&mut self, e: &Elem, k: &str, path: &str) -> Result<Option<Color>, String> {
        let ctx = format!("{path}.{k}");
        Ok(self.get(e, k, path)?.map(|v| self.color_of(&v, &ctx)))
    }

    fn dimension(v: &Value) -> Option<Dimension> {
        match v {
            Value::Num(n) => Some(length(*n as f32)),
            Value::Str(s) if s == "auto" => Some(auto()),
            Value::Str(s) => s.strip_suffix('%').and_then(|p| p.trim().parse::<f32>().ok()).map(|p| percent(p / 100.0)).or_else(|| s.parse::<f32>().ok().map(length)),
            _ => None,
        }
    }

    fn length_percent(v: &Value) -> LengthPercentage {
        match Self::dimension(v) {
            Some(_) if matches!(v, Value::Str(s) if s.ends_with('%')) => {
                let p = if let Value::Str(s) = v { s.trim_end_matches('%').parse::<f32>().unwrap_or(0.0) } else { 0.0 };
                percent(p / 100.0)
            }
            _ => length(v.as_f64().unwrap_or(0.0) as f32),
        }
    }

    fn length_percent_auto(v: &Value) -> LengthPercentageAuto {
        match v {
            Value::Str(s) if s == "auto" => auto(),
            Value::Str(s) if s.ends_with('%') => percent(s.trim_end_matches('%').parse::<f32>().unwrap_or(0.0) / 100.0),
            _ => length(v.as_f64().unwrap_or(0.0) as f32),
        }
    }

    /// `n`, `[v, h]` or `[t, r, b, l]`.
    fn sides(v: &Value) -> [Value; 4] {
        match v {
            Value::List(l) if l.len() == 2 => [l[0].clone(), l[1].clone(), l[0].clone(), l[1].clone()],
            Value::List(l) if l.len() == 4 => [l[0].clone(), l[1].clone(), l[2].clone(), l[3].clone()],
            other => [other.clone(), other.clone(), other.clone(), other.clone()],
        }
    }

    fn align(s: &str) -> Option<AlignItems> {
        Some(match s {
            "start" => AlignItems::FLEX_START,
            "center" => AlignItems::CENTER,
            "end" => AlignItems::FLEX_END,
            "stretch" => AlignItems::STRETCH,
            "baseline" => AlignItems::BASELINE,
            _ => return None,
        })
    }

    fn layout(&mut self, e: &Elem, n: &mut Node, path: &str) -> Result<(), String> {
        let st = &mut n.style;
        st.flex_direction = match self.text(e, "direction", path)?.as_deref() {
            Some("row") => FlexDirection::Row,
            Some("row-reverse") => FlexDirection::RowReverse,
            Some("column-reverse") => FlexDirection::ColumnReverse,
            _ => FlexDirection::Column,
        };
        if self.flag(e, "wrap", path)? == Some(true) {
            st.flex_wrap = FlexWrap::Wrap;
        }
        if let Some(a) = self.text(e, "align", path)? {
            st.align_items = Self::align(&a).or_else(|| {
                self.warn(format!("{path}.align: `{a}` (use start, center, end, stretch, baseline)"));
                None
            });
        }
        if let Some(a) = self.text(e, "align_self", path)? {
            st.align_self = Self::align(&a);
        }
        if let Some(j) = self.text(e, "justify", path)? {
            st.justify_content = Some(match j.as_str() {
                "center" => JustifyContent::CENTER,
                "end" => JustifyContent::FLEX_END,
                "between" => JustifyContent::SPACE_BETWEEN,
                "around" => JustifyContent::SPACE_AROUND,
                "evenly" => JustifyContent::SPACE_EVENLY,
                _ => JustifyContent::FLEX_START,
            });
        }
        if let Some(g) = self.get(e, "gap", path)? {
            let (r, c) = match &g {
                Value::List(l) if l.len() == 2 => (Self::length_percent(&l[0]), Self::length_percent(&l[1])),
                v => (Self::length_percent(v), Self::length_percent(v)),
            };
            st.gap = Size { width: c, height: r };
        }
        if let Some(p) = self.get(e, "padding", path)? {
            let [t, r, b, l] = Self::sides(&p);
            st.padding = Rect { left: Self::length_percent(&l), right: Self::length_percent(&r), top: Self::length_percent(&t), bottom: Self::length_percent(&b) };
        }
        if let Some(m) = self.get(e, "margin", path)? {
            let [t, r, b, l] = Self::sides(&m);
            st.margin = Rect { left: Self::length_percent_auto(&l), right: Self::length_percent_auto(&r), top: Self::length_percent_auto(&t), bottom: Self::length_percent_auto(&b) };
        }
        if let Some(v) = self.get(e, "width", path)? {
            st.size.width = Self::dimension(&v).unwrap_or(auto());
        }
        if let Some(v) = self.get(e, "height", path)? {
            st.size.height = Self::dimension(&v).unwrap_or(auto());
        }
        for (k, apply) in [("min_width", 0), ("min_height", 1), ("max_width", 2), ("max_height", 3)] {
            if let Some(v) = self.get(e, k, path)? {
                let d = Self::length_percent_auto(&v);
                match apply {
                    0 => st.min_size.width = d,
                    1 => st.min_size.height = d,
                    2 => st.max_size.width = d,
                    _ => st.max_size.height = d,
                }
            }
        }
        if let Some(v) = self.num(e, "grow", path)? {
            st.flex_grow = v;
        }
        if let Some(v) = self.num(e, "shrink", path)? {
            st.flex_shrink = v;
        }
        if let Some(v) = self.get(e, "basis", path)? {
            st.flex_basis = Self::dimension(&v).unwrap_or(auto());
        }
        if let Some(v) = self.num(e, "aspect", path)? {
            st.aspect_ratio = Some(v);
        }
        let abs = self.text(e, "position", path)?.as_deref() == Some("absolute");
        let has_inset = ["inset", "left", "top", "right", "bottom"].iter().any(|k| e.attrs.contains_key(*k));
        let fills_parent = elements::find(&e.ty).is_some_and(|k| k.fills_parent_when_unsized);
        let implicit_fill = fills_parent && !e.attrs.contains_key("width") && !e.attrs.contains_key("height");
        if abs || has_inset || implicit_fill {
            st.position = Position::Absolute;
            let mut ins = [auto(), auto(), auto(), auto()]; // l t r b
            if let Some(v) = self.get(e, "inset", path)? {
                let s = Self::sides(&v); // t r b l order for lists; scalars fill all
                ins = [Self::length_percent_auto(&s[3]), Self::length_percent_auto(&s[0]), Self::length_percent_auto(&s[1]), Self::length_percent_auto(&s[2])];
            } else if implicit_fill && !has_inset {
                ins = [length(0.0), length(0.0), length(0.0), length(0.0)];
            }
            for (i, k) in ["left", "top", "right", "bottom"].iter().enumerate() {
                if let Some(v) = self.get(e, k, path)? {
                    ins[i] = Self::length_percent_auto(&v);
                }
            }
            st.inset = Rect { left: ins[0], right: ins[2], top: ins[1], bottom: ins[3] };
        }
        Ok(())
    }

    fn look(&mut self, e: &Elem, n: &mut Node, path: &str) -> Result<(), String> {
        match self.get(e, "fill", path)? {
            Some(Value::List(l)) if l.len() == 2 => {
                n.look.fill = self.color_of(&l[0], &format!("{path}.fill"));
                n.look.gradient_bottom = Some(self.color_of(&l[1], &format!("{path}.fill")));
            }
            Some(v) => n.look.fill = self.color_of(&v, &format!("{path}.fill")),
            None => {}
        }
        // fill only, so a see-through card keeps opaque children; negative keeps the theme's alpha
        if let Some(a) = self.num(e, "fill_alpha", path)?.filter(|a| *a >= 0.0) {
            n.look.fill = n.look.fill.with_alpha(a.clamp(0.0, 1.0));
            n.look.gradient_bottom = n.look.gradient_bottom.map(|c| c.with_alpha(a.clamp(0.0, 1.0)));
        }
        if let Some(w) = self.num(e, "border", path)? {
            n.look.border = w;
            n.look.border_color = self.color(e, "border_color", path)?.unwrap_or_else(|| self.theme.color("border"));
        }
        if let Some(r) = self.num(e, "radius", path)? {
            n.look.radius = r;
        }
        if let Some(o) = self.num(e, "opacity", path)? {
            n.look.opacity = o;
        }
        if let Some(a) = e.attrs.get("shadow") {
            let shadow_col = self.theme.color("shadow");
            n.look.shadow = match a {
                Attr::Table(t) => {
                    let mut num = |k: &str, d: f32| -> Result<f32, String> {
                        Ok(t.get(k).map(|a| self.eval_attr(a, &format!("{path}.shadow.{k}"))).transpose()?.and_then(|v| v.as_f64()).map_or(d, |x| x as f32))
                    };
                    let (blur, dy) = (num("blur", 12.0)?, num("dy", 4.0)?);
                    let c = match t.get("color") {
                        Some(a) => {
                            let v = self.eval_attr(a, &format!("{path}.shadow.color"))?;
                            self.color_of(&v, path)
                        }
                        None => shadow_col,
                    };
                    Some(Shadow { blur, dy, color: c })
                }
                other => {
                    let blur = self.eval_attr(other, &format!("{path}.shadow"))?.as_f64().unwrap_or(0.0) as f32;
                    (blur > 0.0).then_some(Shadow { blur, dy: blur / 3.0, color: shadow_col })
                }
            };
        }
        if let Some(c) = self.flag(e, "clip", path)? {
            n.clip = c;
        }
        Ok(())
    }

    fn interact(&mut self, e: &Elem, n: &mut Node, path: &str) -> Result<(), String> {
        n.action = self.text(e, "on_click", path)?.filter(|s| !s.is_empty());
        n.on_drop = self.text(e, "on_drop", path)?.filter(|s| !s.is_empty());
        if let Some(Attr::Table(t)) = e.attrs.get("hover") {
            for (k, a) in t {
                let ctx = format!("{path}.hover.{k}");
                let v = self.eval_attr(a, &ctx)?;
                match k.as_str() {
                    "fill" => n.hover.fill = Some(self.color_of(&v, &ctx)),
                    "border_color" => n.hover.border_color = Some(self.color_of(&v, &ctx)),
                    "color" => n.hover.text_color = Some(self.color_of(&v, &ctx)),
                    "opacity" => n.hover.opacity = v.as_f64().map(|x| x as f32),
                    _ => return Err(format!("{ctx}: unknown hover key `{k}` (fill, border_color, color, opacity)")),
                }
            }
        }
        match e.attrs.get("transition") {
            Some(Attr::Table(t)) => {
                let ms = t.get("ms").map(|a| self.eval_attr(a, path)).transpose()?.and_then(|v| v.as_f64()).unwrap_or(150.0) as u32;
                let ease = t.get("ease").map(|a| self.eval_attr(a, path)).transpose()?.map(|v| Ease::parse(&v.to_string())).unwrap_or_default();
                n.transition = Transition { ms, ease };
            }
            Some(a) => n.transition = Transition { ms: self.eval_attr(a, path)?.as_f64().unwrap_or(0.0) as u32, ease: Ease::Out },
            None => {}
        }
        if let Some(Attr::Table(t)) = e.attrs.get("enter") {
            let mut num = |k: &str, d: f64| -> Result<f64, String> { Ok(t.get(k).map(|a| self.eval_attr(a, path)).transpose()?.and_then(|v| v.as_f64()).unwrap_or(d)) };
            let (ms, dy, delay, stagger) = (num("ms", 200.0)?, num("dy", 8.0)?, num("delay", 0.0)?, num("stagger", 0.0)?);
            let idx = self.scope_number("index");
            n.enter = Some(Enter { ms: ms as u32, dy: dy as f32, delay: (delay + stagger * idx) as u32 });
        }
        if let Some(s) = self.num(e, "scroll", path)? {
            n.scroll_offset = Some(s.max(0.0));
        }
        if let Some(s) = self.num(e, "scroll_x", path)? {
            n.scroll_offset_x = Some(s.max(0.0));
        }
        n.overlay = self.flag(e, "overlay", path)?.unwrap_or(false);
        n.hit_testable = self.flag(e, "hit", path)?.unwrap_or(false);
        if self.preview {
            // Settings shows the widget, it must not act on it
            n.action = None;
            n.on_drop = None;
            n.hit_testable = false;
        }
        Ok(())
    }

    fn scope_number(&self, name: &str) -> f64 {
        self.scope.peek_untracked(name).and_then(|v| v.as_f64()).unwrap_or(0.0)
    }

    fn build_elem(&mut self, e: &Elem, key: &str) -> Result<Vec<Node>, String> {
        let path = key.to_string();
        if let Some(w) = &e.when {
            if !w.eval(&self.scope).map_err(|x| format!("{path}.when: {x}"))?.truthy() {
                return Ok(vec![]);
            }
        }
        if e.ty == "repeat" {
            return self.repeat(e, key);
        }
        if e.ty == "slot" {
            return self.slot(e, key);
        }
        let mut n = Node::new(key);
        self.layout(e, &mut n, &path)?;
        self.look(e, &mut n, &path)?;
        self.interact(e, &mut n, &path)?;

        let kind = elements::find(&e.ty).ok_or_else(|| format!("{path}: unknown type `{}`", e.ty))?;
        n.kind = (kind.build)(&mut Attrs { b: self, e, path: &path })?;
        for (i, c) in e.children.iter().enumerate() {
            let kids = self.build_elem(c, &format!("{key}/{i}"))?;
            n.children.extend(kids);
        }
        Ok(vec![n])
    }

    fn repeat(&mut self, e: &Elem, key: &str) -> Result<Vec<Node>, String> {
        let path = key.to_string();
        let list = match self.get(e, "for", &path)? {
            Some(Value::List(l)) => l,
            Some(Value::Nil) | None => vec![],
            Some(other) => return Err(format!("{path}.for: expected a list, got `{other}`")),
        };
        let name = self.text(e, "as", &path)?.unwrap_or_else(|| "item".into());
        let idx_name = self.text(e, "index", &path)?.unwrap_or_else(|| "index".into());
        let mut out = Vec::new();
        for (i, item) in list.into_iter().enumerate() {
            self.scope.set(&name, item);
            self.scope.set(&idx_name, Value::Num(i as f64));
            for (ci, c) in e.children.iter().enumerate() {
                match self.build_elem(c, &format!("{key}/{i}.{ci}")) {
                    Ok(v) => out.extend(v),
                    Err(err) => {
                        self.scope.pop();
                        self.scope.pop();
                        return Err(err);
                    }
                }
            }
            self.scope.pop();
            self.scope.pop();
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Library, Selection};
    use std::path::Path;

    fn theme() -> Theme {
        Theme::compose(&Library::load(Path::new("nope")), &Selection::default(), &[])
    }

    fn build_src(src: &str, params: &[(&str, Value)]) -> Result<Built, String> {
        let def = WidgetDef::parse("t", src)?;
        let p: BTreeMap<String, Value> = params.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        let st = BTreeMap::new();
        let inp = Inputs {
            params: &p,
            state: &st,
            card_size: (100.0, 60.0),
            key_prefix: "t",
            read_source: &|name| match name {
                "clock" => Some(Value::obj([("minute", 7.into()), ("second", 3.into())])),
                "shortcuts" => Some(crate::data::shortcuts_value(&[], "Default")),
                _ => None,
            },
            arrange: None,
        };
        build(&def, &inp, &theme(), &|_| None)
    }

    fn image_ids(def: &WidgetDef) -> (BTreeSet<String>, Vec<String>) {
        let (p, st) = (BTreeMap::new(), BTreeMap::new());
        let inp = Inputs { params: &p, state: &st, card_size: (100.0, 60.0), key_prefix: "t", read_source: &|_| None, arrange: None };
        let b = build(def, &inp, &theme(), &|_| None).unwrap();
        (b.image_ids, b.warnings)
    }

    const LOGO: &str = "[root]\ntype = 'image'\nsrc = './logo.png'";

    #[test]
    fn a_dot_slash_src_resolves_next_to_the_widget_file() {
        let root = std::env::temp_dir().join("plug");
        let mut def = WidgetDef::parse("t", LOGO).unwrap();
        def.base = Some(Base { dir: root.join("widgets"), root: root.clone() });
        let (ids, warns) = image_ids(&def);
        assert!(warns.is_empty(), "{warns:?}");
        assert_eq!(ids.into_iter().collect::<Vec<_>>(), [format!("file:{}", root.join("widgets").join("logo.png").display())]);
        let shared = Base { dir: root.join("widgets"), root: root.clone() }.resolve("./../images/a.png");
        assert_eq!(shared, Some(root.join("images").join("a.png")), "a sibling folder of the root is fine");
    }

    #[test]
    fn a_dot_slash_src_cannot_escape_its_root() {
        let root = std::env::temp_dir().join("plug");
        let base = Base { dir: root.join("widgets"), root: root.clone() };
        assert_eq!(base.resolve("./../../secret.png"), None);
        let mut def = WidgetDef::parse("t", "[root]\ntype = 'image'\nsrc = './../../x.png'").unwrap();
        def.base = Some(base);
        let (ids, warns) = image_ids(&def);
        assert!(warns.iter().any(|w| w.contains("leaves")), "{warns:?}");
        assert!(!ids.iter().any(|i| i.contains("x.png")));
    }

    #[test]
    fn an_image_fits_by_contain_or_cover() {
        let fit = |f: &str| {
            let def = WidgetDef::parse("t", &format!("[root]\ntype = 'image'\nsrc = 'icon:x'\n{f}")).unwrap();
            let (p, st) = (BTreeMap::new(), BTreeMap::new());
            let inp = Inputs { params: &p, state: &st, card_size: (100.0, 60.0), key_prefix: "t", read_source: &|_| None, arrange: None };
            build(&def, &inp, &theme(), &|_| None).map(|b| match b.root.kind {
                crate::ui::Kind::Image(im) => im.fit,
                _ => panic!("not an image"),
            })
        };
        assert_eq!((fit(""), fit("fit = 'cover'"), fit("fit = 'contain'")), (Ok(crate::ui::Fit::Contain), Ok(crate::ui::Fit::Cover), Ok(crate::ui::Fit::Contain)));
        assert!(fit("fit = 'fill'").unwrap_err().contains("contain or cover"));
    }

    #[test]
    fn a_full_path_with_max_asks_for_a_small_copy() {
        let full = |extra: &str| image_ids(&WidgetDef::parse("t", &format!("[root]\ntype = 'image'\nsrc = 'C:\\Photos\\a.jpg'\n{extra}")).unwrap()).0.into_iter().collect::<Vec<_>>();
        assert_eq!(full(""), ["file:C:\\Photos\\a.jpg"]);
        assert_eq!(full("max = 256"), ["thumb:256:C:\\Photos\\a.jpg"]);
        assert_eq!(full("max = 1"), ["thumb:8:C:\\Photos\\a.jpg"], "at least 8 px");
        let icon = image_ids(&WidgetDef::parse("t", "[root]\ntype = 'image'\nsrc = 'icon:x'\nmax = 64").unwrap()).0;
        assert_eq!(icon.into_iter().collect::<Vec<_>>(), ["icon:x"], "app icons are small already");
    }

    #[test]
    fn a_dot_slash_src_in_a_builtin_warns() {
        let (ids, warns) = image_ids(&WidgetDef::parse("t", LOGO).unwrap());
        assert!(warns.iter().any(|w| w.contains("needs a widget file")), "{warns:?}");
        assert!(!ids.iter().any(|i| i.contains("logo")));
    }

    #[test]
    fn params_take_labelled_choices_and_groups() {
        let src = "[params.speed]\ntype = 'enum'\ngroup = 'Motion'\nchoices = ['slow', { value = 'fast', label = 'Fast (30 fps)' }]\n[params.color]\ntype = 'color'\n[params.decay]\ntype = 'number'\ngroup = 'Motion'\n[params.bars]\ntype = 'number'\ngroup = 'Shape'\n[root]\ntype = 'box'";
        let d = WidgetDef::parse("t", src).unwrap();
        let speed = &d.meta.params[0];
        assert_eq!(speed.choices, [Choice { value: "slow".into(), label: "slow".into() }, Choice { value: "fast".into(), label: "Fast (30 fps)".into() }]);
        let groups: Vec<(Option<&str>, Vec<&str>)> = ParamDef::grouped(&d.meta.params).into_iter().map(|(g, ps)| (g, ps.iter().map(|p| p.name.as_str()).collect())).collect();
        assert_eq!(groups, [(None, vec!["color"]), (Some("Motion"), vec!["speed", "decay"]), (Some("Shape"), vec!["bars"])]);
        let pics = WidgetDef::parse("t", "[params.pics]\ntype = 'folder'\n[root]\ntype = 'box'").unwrap();
        assert_eq!(pics.meta.params[0].ty, ParamType::Path, "a folder picker");
        let e = WidgetDef::parse("t", "[params.s]\ntype = 'enum'\nchoices = [{ label = 'x' }]\n[root]\ntype = 'box'").err().unwrap();
        assert!(e.contains("params.s.choices"), "{e}");
    }

    #[test]
    fn max_size_is_read_and_must_not_undercut_the_min() {
        let d = WidgetDef::parse("t", "min_size = [100, 100]\nmax_size = [300, 200]\n[root]\ntype = 'box'").unwrap();
        assert_eq!((d.meta.max_card_size, WidgetDef::parse("t", "[root]\ntype = 'box'").unwrap().meta.max_card_size), (Some((300.0, 200.0)), None));
        let e = WidgetDef::parse("t", "min_size = [100, 100]\nmax_size = [80, 200]\n[root]\ntype = 'box'").err().unwrap();
        assert!(e.contains("max_size"), "{e}");
    }

    #[test]
    fn typo_in_an_attribute_is_named_with_a_suggestion() {
        let e = WidgetDef::parse("t", "[root]\ntype='text'\ncolour='#fff'").unwrap_err();
        assert!(e.contains("unknown attribute `colour`") && e.contains("did you mean `color`"), "{e}");
        let e = WidgetDef::parse("t", "[root]\ntype='txet'").unwrap_err();
        assert!(e.contains("did you mean `text`"), "{e}");
        let e = WidgetDef::parse("t", "nmae='x'\n[root]").unwrap_err();
        assert!(e.contains("did you mean `name`"), "{e}");
        assert!(WidgetDef::parse("t", "[root]\ntext='{1 +}'\ntype='text'").is_err()); // bad expression caught at parse time
    }

    #[test]
    fn seeds_are_checked_when_the_file_is_read() {
        let p = |extra: &str| WidgetDef::parse("t", &format!("[params.items]\ntype='shortcuts'\n{extra}\n[root]"));
        assert_eq!(p("seed='starter-apps'").unwrap().meta.params[0].seed, Some(Seed::StarterApps));
        let e = p("seed='starter-app'").unwrap_err();
        assert!(e.contains("did you mean `starter-apps`"), "{e}");
        let e = WidgetDef::parse("t", "[params.t]\ntype='string'\nseed='starter-apps'\n[root]").unwrap_err();
        assert!(e.contains("needs type = \"shortcuts\""), "{e}");
    }

    #[test]
    fn element_kinds_come_from_the_table_and_keep_their_own_attributes() {
        for k in elements::KINDS {
            assert!(k.own_attrs.iter().all(|a| !COMMON_ATTRS.contains(a)), "`{}` redeclares a common attribute", k.name);
        }
        let e = WidgetDef::parse("t", "[root]\ntype='nope'").unwrap_err();
        assert!(e.contains("box, text, image, hand, ticks, arc, graph, repeat, slot"), "{e}");
        assert!(WidgetDef::parse("t", "[root]\ntype='arc'\nsweep=90\nvalue=50").is_ok());
        let e = WidgetDef::parse("t", "[root]\ntype='arc'\nangle=90").unwrap_err();
        assert!(e.contains("unknown attribute `angle` on `arc`"), "an attribute of another kind is rejected: {e}");
    }

    const MODS: &str = "
[tiers.small]
when = '{self.w < 80}'
[tiers.wide]
layout = { row = ['a', 'g', 'b'] }
[slots.row]
[slots.col]
[modules.a]
label = 'A'
[modules.b]
slots = ['row']
[modules.g]
for = '{clock.list}'
as = 'x'
key = '{x.id}'
label = 'GPU {x.id}'
[root]
type = 'box'
[[root.children]]
type = 'slot'
slot = 'row'
";

    fn mods(size: f32, layout: &crate::workspace::Layout, tier: Option<&str>) -> Built {
        let def = WidgetDef::parse("t", MODS).unwrap();
        let (p, st) = (BTreeMap::new(), BTreeMap::new());
        let read = |n: &str| (n == "clock").then(|| Value::obj([("list", Value::List(vec![Value::obj([("id", "one".into())]), Value::obj([("id", "two".into())])]))]));
        let inp = Inputs { params: &p, state: &st, card_size: (size, 60.0), key_prefix: "t", read_source: &read, arrange: Some(Arrange { layout, tier, preview: false }) };
        build(&def, &inp, &theme(), &|_| None).unwrap()
    }

    #[test]
    fn modules_fill_a_slot_from_the_default_then_from_the_saved_layout() {
        let none = crate::workspace::Layout::new();
        let ids = |b: &Built| b.arrangement.as_ref().unwrap().slots[0].modules.iter().map(|m| m.id.clone()).collect::<Vec<_>>();
        let b = mods(200.0, &none, None);
        let a = b.arrangement.as_ref().unwrap();
        assert_eq!((a.tier.as_str(), ids(&b)), ("wide", vec!["a".into(), "g:one".into(), "g:two".into(), "b".into()]), "a module name places every item of it");
        assert_eq!(b.root.children[0].children.len(), 4);
        assert_eq!(a.slots[0].modules[1].label, "GPU one");
        assert_eq!(mods(50.0, &none, None).arrangement.unwrap().tier, "small", "the first tier whose `when` holds");
        assert_eq!(mods(200.0, &none, Some("small")).arrangement.unwrap().tier, "small", "a tier can be asked for");

        let saved = crate::workspace::Layout::from([("wide".to_string(), BTreeMap::from([("row".to_string(), vec!["g:two".to_string(), "a".to_string()])]))]);
        let b = mods(200.0, &saved, None);
        assert_eq!(ids(&b), ["g:two", "a"]);
        let hidden: Vec<String> = b.arrangement.unwrap().hidden.into_iter().map(|m| m.id).collect();
        assert_eq!(hidden, ["b", "g:one"], "what a saved layout leaves out is hidden, in declaration order");
    }

    #[test]
    fn a_widget_that_names_no_module_or_slot_it_declares_is_rejected() {
        let bad = |extra: &str| WidgetDef::parse("t", &format!("[tiers.a]\n{extra}\n[slots.row]\n[modules.a]\n[root]\ntype='box'")).err().unwrap();
        assert!(bad("layout = { rwo = ['a'] }").contains("did you mean `row`"));
        assert!(bad("layout = { row = ['b'] }").contains("unknown module `b`"));
        let e = WidgetDef::parse("t", "[params.x]\ntype='bool'\nmodule='nope'\n[tiers.a]\n[root]\ntype='box'").err().unwrap();
        assert!(e.contains("does not declare"), "{e}");
    }

    #[test]
    fn bindings_tokens_and_dependencies() {
        let b = build_src(
            "[params.who]\ntype='string'\ndefault='World'\n[root]\ntype='text'\ntext='Hi {param.who} {clock.minute|02}'\ncolor='$accent'\nsize='$font-size-lg'",
            &[],
        )
        .unwrap();
        let Kind::Text(t) = &b.root.kind else { panic!("not text") };
        assert_eq!(t.text, "Hi World 07");
        assert_eq!(t.size, 18.0);
        assert_eq!(t.color.to_hex(), "#6ea8ff");
        assert!(b.deps.contains("clock.minute") && b.deps.contains("param.who"), "{:?}", b.deps);
        assert!(!b.deps.contains("clock.second"));
        // the build is the card alone, sized to the card; `Card::window_node` adds the window
        assert_eq!((b.root.style.size.width, b.root.style.size.height), (length(100.0), length(60.0)));
    }

    #[test]
    fn fill_alpha_replaces_only_the_fill_alpha_and_a_param_can_pick_the_tint() {
        let src = "[params.tint]
type='bool'
default=true
[root]
type='box'
fill=[\"{param.tint ? '$accent' : '#000000'}\", '$accent']
fill_alpha=0.5";
        let b = build_src(src, &[]).unwrap();
        let l = &b.root.look;
        assert_eq!((l.fill.to_hex().as_str(), l.fill.0[3], l.gradient_bottom.unwrap().0[3]), ("#6ea8ff80", 0.5, 0.5));
        let b = build_src(src, &[("tint", false.into())]).unwrap();
        assert_eq!(b.root.look.fill.to_hex(), "#00000080");
    }

    #[test]
    fn undefined_token_is_magenta_and_reported_not_defaulted() {
        let b = build_src("[root]\ntype='box'\nfill='$nope'", &[]).unwrap();
        assert_eq!(b.root.look.fill, MAGENTA);
        assert!(b.warnings.iter().any(|w| w.contains("undefined token $nope")), "{:?}", b.warnings);
    }

    #[test]
    fn a_param_can_carry_a_token_reference() {
        let b = build_src("[params.c]\ntype='color'\ndefault='$accent'\n[root]\ntype='box'\nfill='{param.c}'", &[]).unwrap();
        assert_eq!(b.root.look.fill.to_hex(), "#6ea8ff");
        let b = build_src("[params.c]\ntype='color'\ndefault='$accent'\n[root]\ntype='box'\nfill='{param.c}'", &[("c", "#00ff00".into())]).unwrap();
        assert_eq!(b.root.look.fill.to_hex(), "#00ff00");
    }

    #[test]
    fn repeat_expands_conditionals_and_reports_runtime_errors() {
        let src = "[root]\ntype='box'\n[[root.children]]\ntype='repeat'\nfor='{[1]}'\n";
        // a literal list isn't in the grammar, so this is a *runtime* error, surfaced not swallowed
        assert!(build_src(src, &[]).is_err());

        let src = "[root]\ntype='box'\n[[root.children]]\ntype='text'\ntext='shown'\nwhen='{clock.minute > 5}'\n[[root.children]]\ntype='text'\ntext='hidden'\nwhen='{clock.minute > 50}'";
        let b = build_src(src, &[]).unwrap();
        assert_eq!(b.root.children.len(), 1);

        let e = build_src("[root]\ntype='text'\ntext='{nope.x}'", &[]).unwrap_err();
        assert!(e.contains("`nope`"), "{e}");
    }
}
