//! Widget definition files (TOML) -> element AST -> `ui::Node` tree (decisions 8, 11, 12, 14).
//! Authored by hand, so unknown names are rejected with a "did you mean".

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use taffy::prelude::*;

use crate::anim::Ease;
use crate::color::{Color, MAGENTA};
use crate::elements;
use crate::expr::{Scope, Template};
use crate::theme::Theme;
use crate::ui::*;
use crate::value::Value;
use crate::widgets::{Built, ExpandInfo, Inputs, ParamDef, ParamType, Seed, WidgetMeta};


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
    "border_color", "radius", "opacity", "shadow", "clip", "on_click", "hover", "transition", "enter", "scroll", "overlay", "hit",
];
/// `repeat` is structural, not an element kind.
const REPEAT_ATTRS: &[&str] = &["for", "as", "index"];

fn type_names() -> Vec<&'static str> {
    elements::KINDS.iter().map(|k| k.name).chain(["repeat"]).collect()
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

const TOP: &[&str] = &["name", "description", "size", "min_size", "max_size", "params", "state", "expand", "root"];

fn pair(v: Option<&toml::Value>, default: (f32, f32), what: &str) -> Result<(f32, f32), String> {
    let Some(v) = v else { return Ok(default) };
    let a = v.as_array().filter(|a| a.len() == 2).ok_or_else(|| format!("{what}: expected [width, height]"))?;
    let n = |x: &toml::Value| x.as_float().or_else(|| x.as_integer().map(|i| i as f64)).map(|f| f as f32);
    Ok((n(&a[0]).ok_or_else(|| format!("{what}: width is not a number"))?, n(&a[1]).ok_or_else(|| format!("{what}: height is not a number"))?))
}

/// A `[params]` table, in file order. Also parses the style schema (`assets/style.toml`).
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
            choices: p.get("choices").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|c| c.as_str().map(String::from)).collect()).unwrap_or_default(),
            seed,
        });
    }
    Ok(params)
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
        };
        Ok(WidgetDef { meta, expand, root: parse_elem(root_t, "root")?, base: None })
    }
}


struct TreeBuilder<'a> {
    theme: &'a Theme,
    base: Option<&'a Base>,
    scope: Scope<'a>,
    warns: Vec<String>,
    images: BTreeSet<String>,
    image_size: &'a dyn Fn(&str) -> Option<(f32, f32)>,
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
    let mut b = TreeBuilder { theme, base: def.base.as_ref(), scope, warns: vec![], images: BTreeSet::new(), image_size };
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
    Ok(Built { root, deps: b.scope.deps(), image_ids: b.images, warnings: b.warns, expand })
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

impl TreeBuilder<'_> {
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
        n.overlay = self.flag(e, "overlay", path)?.unwrap_or(false);
        n.hit_testable = self.flag(e, "hit", path)?.unwrap_or(false);
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
        };
        build(&def, &inp, &theme(), &|_| None)
    }

    fn image_ids(def: &WidgetDef) -> (BTreeSet<String>, Vec<String>) {
        let (p, st) = (BTreeMap::new(), BTreeMap::new());
        let inp = Inputs { params: &p, state: &st, card_size: (100.0, 60.0), key_prefix: "t", read_source: &|_| None };
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
            let inp = Inputs { params: &p, state: &st, card_size: (100.0, 60.0), key_prefix: "t", read_source: &|_| None };
            build(&def, &inp, &theme(), &|_| None).map(|b| match b.root.kind {
                crate::ui::Kind::Image(im) => im.fit,
                _ => panic!("not an image"),
            })
        };
        assert_eq!((fit(""), fit("fit = 'cover'"), fit("fit = 'contain'")), (Ok(crate::ui::Fit::Contain), Ok(crate::ui::Fit::Cover), Ok(crate::ui::Fit::Contain)));
        assert!(fit("fit = 'fill'").unwrap_err().contains("contain or cover"));
    }

    #[test]
    fn a_dot_slash_src_in_a_builtin_warns() {
        let (ids, warns) = image_ids(&WidgetDef::parse("t", LOGO).unwrap());
        assert!(warns.iter().any(|w| w.contains("needs a widget file")), "{warns:?}");
        assert!(!ids.iter().any(|i| i.contains("logo")));
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
        assert!(e.contains("box, text, image, hand, ticks, arc, graph, repeat"), "{e}");
        assert!(WidgetDef::parse("t", "[root]\ntype='arc'\nsweep=90\nvalue=50").is_ok());
        let e = WidgetDef::parse("t", "[root]\ntype='arc'\nangle=90").unwrap_err();
        assert!(e.contains("unknown attribute `angle` on `arc`"), "an attribute of another kind is rejected: {e}");
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
        assert!(e.contains("undefined name `nope`"), "{e}");
    }
}
