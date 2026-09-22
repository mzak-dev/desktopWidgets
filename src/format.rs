//! Widget definition files (TOML) -> element AST -> `ui::Node` tree
//! (decisions 8, 11, 12, 14). Authored by hand, so parse errors are precise and
//! unknown attributes come with a "did you mean" instead of being ignored.

use std::collections::{BTreeMap, BTreeSet};

use taffy::prelude::*;

use crate::anim::Ease;
use crate::color::{Color, MAGENTA};
use crate::expr::{Scope, Template};
use crate::text::{TextAlign, TextSpec};
use crate::theme::Theme;
use crate::ui::*;
use crate::value::Value;

// ---- attributes ------------------------------------------------------------

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

const COMMON: &[&str] = &[
    "id", "width", "height", "min_width", "min_height", "max_width", "max_height", "grow", "shrink", "basis", "direction", "wrap", "align",
    "justify", "align_self", "gap", "padding", "margin", "position", "inset", "left", "top", "right", "bottom", "aspect", "fill", "fill_alpha", "border",
    "border_color", "radius", "opacity", "shadow", "clip", "on_click", "hover", "transition", "enter", "scroll", "overlay", "hit",
];
const TEXT: &[&str] = &["text", "size", "color", "font", "weight", "text_align", "text_wrap", "line_height"];
const IMAGE: &[&str] = &["src", "tint"];
const HAND: &[&str] = &["angle", "length", "tail", "stroke", "color"];
const TICKS: &[&str] = &["count", "major_every", "tick_length", "major_length", "tick_width", "major_width", "color", "major_color", "tick_inset"];
const ARC: &[&str] = &["value", "start", "sweep", "stroke", "color", "track"];
const REPEAT: &[&str] = &["for", "as", "index"];
const TYPES: &[&str] = &["box", "text", "image", "hand", "ticks", "arc", "repeat"];

fn lev(a: &str, b: &str) -> usize {
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

fn suggest(name: &str, pool: &[&[&str]]) -> String {
    pool.iter()
        .flat_map(|p| p.iter())
        .map(|c| (lev(name, c), *c))
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
    if !TYPES.contains(&ty.as_str()) {
        return Err(format!("{path}: unknown type `{ty}`{} (expected one of {})", suggest(&ty, &[TYPES]), TYPES.join(", ")));
    }
    let specific: &[&str] = match ty.as_str() {
        "text" => TEXT,
        "image" => IMAGE,
        "hand" => HAND,
        "ticks" => TICKS,
        "arc" => ARC,
        "repeat" => REPEAT,
        _ => &[],
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
            k if COMMON.contains(&k) || specific.contains(&k) => {
                attrs.insert(k.to_string(), parse_attr(v, &format!("{path}.{k}"))?);
            }
            k => return Err(format!("{path}: unknown attribute `{k}` on `{ty}`{}", suggest(k, &[COMMON, specific]))),
        }
    }
    Ok(Elem { ty, attrs, children, when })
}

// ---- widget definition -----------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamType {
    Color,
    Font,
    Number,
    Enum,
    Bool,
    Str,
    Path,
    Duration,
    Shortcuts,
}

impl ParamType {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "color" => Self::Color,
            "font" => Self::Font,
            "number" => Self::Number,
            "enum" => Self::Enum,
            "bool" => Self::Bool,
            "string" => Self::Str,
            "path" => Self::Path,
            "duration" => Self::Duration,
            "shortcuts" => Self::Shortcuts,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct ParamDef {
    pub name: String,
    pub ty: ParamType,
    pub default: Value,
    pub label: String,
    pub help: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    pub choices: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Expand {
    pub when: Template,
    pub width: Option<Template>,
    pub height: Option<Template>,
}

#[derive(Clone, Debug)]
pub struct WidgetDef {
    pub id: String,
    pub name: String,
    pub description: String,
    pub size: (f32, f32),
    pub min_size: (f32, f32),
    pub params: Vec<ParamDef>,
    pub state: BTreeMap<String, Value>,
    pub expand: Option<Expand>,
    pub root: Elem,
}

const TOP: &[&str] = &["name", "description", "size", "min_size", "params", "state", "expand", "root"];

fn pair(v: Option<&toml::Value>, default: (f32, f32), what: &str) -> Result<(f32, f32), String> {
    let Some(v) = v else { return Ok(default) };
    let a = v.as_array().filter(|a| a.len() == 2).ok_or_else(|| format!("{what}: expected [width, height]"))?;
    let n = |x: &toml::Value| x.as_float().or_else(|| x.as_integer().map(|i| i as f64)).map(|f| f as f32);
    Ok((n(&a[0]).ok_or_else(|| format!("{what}: width is not a number"))?, n(&a[1]).ok_or_else(|| format!("{what}: height is not a number"))?))
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
        let mut params = Vec::new();
        if let Some(pt) = t.get("params").and_then(|v| v.as_table()) {
            for (name, v) in pt {
                let p = v.as_table().ok_or_else(|| format!("params.{name}: expected a table"))?;
                let ty = p.get("type").and_then(|v| v.as_str()).ok_or_else(|| format!("params.{name}: missing `type`"))?;
                let ty = ParamType::parse(ty).ok_or_else(|| format!("params.{name}: unknown param type `{ty}`"))?;
                let f = |k: &str| p.get(k).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)));
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
                });
            }
        }
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
        Ok(WidgetDef {
            id: id.to_string(),
            name: if text("name").is_empty() { id.to_string() } else { text("name") },
            description: text("description"),
            size: pair(t.get("size"), (200.0, 120.0), "size")?,
            min_size: pair(t.get("min_size"), (48.0, 48.0), "min_size")?,
            params,
            state,
            expand,
            root: parse_elem(root_t, "root")?,
        })
    }

    /// Defaults overlaid with an Instance's saved values.
    pub fn effective_params(&self, over: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
        self.params.iter().map(|p| (p.name.clone(), over.get(&p.name).cloned().unwrap_or_else(|| p.default.clone()))).collect()
    }
}

// ---- building --------------------------------------------------------------

/// Everything a build reads besides the definition.
pub struct Inputs<'a> {
    pub params: &'a BTreeMap<String, Value>,
    pub state: &'a BTreeMap<String, Value>,
    /// Card size, logical px (the window minus its gutter: see `card`).
    pub size: (f32, f32),
    /// Unique per Instance: prefixes every node key so text, hover and animation state never collide.
    pub key: &'a str,
    pub clock: Value,
    pub sys: Value,
    pub shortcuts: Value,
}

#[derive(Debug)]
pub struct Built {
    pub root: Node,
    /// Dotted paths the build read: the scheduler's dependency set.
    pub deps: BTreeSet<String>,
    /// Image ids the tree uses, so the app can make sure they are uploaded.
    pub images: BTreeSet<String>,
    pub warnings: Vec<String>,
    pub expand: Option<ExpandInfo>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExpandInfo {
    pub active: bool,
    pub width: Option<f32>,
    pub height: Option<f32>,
}

struct B<'a> {
    theme: &'a Theme,
    scope: Scope,
    warns: Vec<String>,
    images: BTreeSet<String>,
    image_size: &'a dyn Fn(&str) -> Option<(f32, f32)>,
}

/// Build the card tree of one Instance. The window around it (gutter, blur
/// look, outline switch) is added by `card::Card::dress`.
pub fn build(def: &WidgetDef, inp: &Inputs, theme: &Theme, image_size: &dyn Fn(&str) -> Option<(f32, f32)>) -> Result<Built, String> {
    let card = inp.size;
    let mut scope = Scope::new();
    let obj = |m: &BTreeMap<String, Value>| Value::Obj(m.clone());
    scope.set("param", obj(&def.effective_params(inp.params)));
    let mut st = def.state.clone();
    st.extend(inp.state.clone());
    scope.set("state", obj(&st));
    scope.set("self", Value::obj([("w", (card.0 as f64).into()), ("h", (card.1 as f64).into())]));
    scope.set("clock", inp.clock.clone());
    scope.set("sys", inp.sys.clone());
    scope.set("shortcuts", inp.shortcuts.clone());
    let mut b = B { theme, scope, warns: vec![], images: BTreeSet::new(), image_size };
    let mut nodes = b.build_elem(&def.root, inp.key)?;
    let mut root = nodes.pop().ok_or("root produced no node")?;
    root.style.size = Size { width: length(card.0), height: length(card.1) };
    let expand = match &def.expand {
        None => None,
        Some(e) => {
            let active = e.when.eval(&b.scope).map_err(|x| format!("expand.when: {x}"))?.truthy();
            let dim = |t: &Option<Template>, b: &B| -> Result<Option<f32>, String> {
                t.as_ref().map(|t| t.eval(&b.scope).map_err(|x| format!("expand: {x}")).map(|v| v.as_f64().unwrap_or(0.0) as f32)).transpose()
            };
            // in card units, like the widget file; the window adds its gutter
            Some(ExpandInfo { active, width: dim(&e.width, &b)?, height: dim(&e.height, &b)? })
        }
    };
    Ok(Built { root, deps: b.scope.deps(), images: b.images, warnings: b.warns, expand })
}

/// A visible in-place error (decision 14): same footprint, red outline, the
/// first message. Never a silent skip.
pub fn error_card(msg: &str, size: (f32, f32), theme: &Theme) -> Node {
    let danger = theme.color("danger");
    let head = Node::text("e0", "Widget error", 13.0, danger).with_text(|t| t.weight = 700);
    let body = Node::text("e1", msg.to_string(), 11.5, theme.color("text")).with_text(|t| t.wrap = true);
    let mut card = Node::new("err").wh(size.0, size.1).col().gap(4.0).pad(10.0).fill(Color([0.08, 0.02, 0.03, 0.92])).radius(10.0).border(2.0, danger).child(head).child(body);
    card.clip = true;
    card
}

impl B<'_> {
    fn warn(&mut self, m: String) {
        if !self.warns.contains(&m) {
            self.warns.push(m);
        }
    }

    fn deref(&mut self, v: Value, ctx: &str) -> Value {
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

    fn val(&mut self, a: &Attr, ctx: &str) -> Result<Value, String> {
        Ok(match a {
            Attr::Lit(v) => self.deref(v.clone(), ctx),
            Attr::Token(n) => self.deref(Value::Str(format!("${n}")), ctx),
            Attr::Tpl(t) => {
                let v = t.eval(&self.scope).map_err(|e| format!("{ctx}: {e}"))?;
                self.deref(v, ctx)
            }
            Attr::List(l) => Value::List(l.iter().map(|x| self.val(x, ctx)).collect::<Result<_, _>>()?),
            Attr::Table(_) => return Err(format!("{ctx}: expected a value, found a table")),
        })
    }

    fn get(&mut self, e: &Elem, k: &str, path: &str) -> Result<Option<Value>, String> {
        e.attrs.get(k).map(|a| self.val(a, &format!("{path}.{k}"))).transpose()
    }

    fn f(&mut self, e: &Elem, k: &str, path: &str) -> Result<Option<f32>, String> {
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

    fn col(&mut self, e: &Elem, k: &str, path: &str) -> Result<Option<Color>, String> {
        let ctx = format!("{path}.{k}");
        Ok(self.get(e, k, path)?.map(|v| self.color_of(&v, &ctx)))
    }

    fn dim(v: &Value) -> Option<Dimension> {
        match v {
            Value::Num(n) => Some(length(*n as f32)),
            Value::Str(s) if s == "auto" => Some(auto()),
            Value::Str(s) => s.strip_suffix('%').and_then(|p| p.trim().parse::<f32>().ok()).map(|p| percent(p / 100.0)).or_else(|| s.parse::<f32>().ok().map(length)),
            _ => None,
        }
    }

    fn lp(v: &Value) -> LengthPercentage {
        match Self::dim(v) {
            Some(_) if matches!(v, Value::Str(s) if s.ends_with('%')) => {
                let p = if let Value::Str(s) = v { s.trim_end_matches('%').parse::<f32>().unwrap_or(0.0) } else { 0.0 };
                percent(p / 100.0)
            }
            _ => length(v.as_f64().unwrap_or(0.0) as f32),
        }
    }

    fn lpa(v: &Value) -> LengthPercentageAuto {
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
                Value::List(l) if l.len() == 2 => (Self::lp(&l[0]), Self::lp(&l[1])),
                v => (Self::lp(v), Self::lp(v)),
            };
            st.gap = Size { width: c, height: r };
        }
        if let Some(p) = self.get(e, "padding", path)? {
            let [t, r, b, l] = Self::sides(&p);
            st.padding = Rect { left: Self::lp(&l), right: Self::lp(&r), top: Self::lp(&t), bottom: Self::lp(&b) };
        }
        if let Some(m) = self.get(e, "margin", path)? {
            let [t, r, b, l] = Self::sides(&m);
            st.margin = Rect { left: Self::lpa(&l), right: Self::lpa(&r), top: Self::lpa(&t), bottom: Self::lpa(&b) };
        }
        if let Some(v) = self.get(e, "width", path)? {
            st.size.width = Self::dim(&v).unwrap_or(auto());
        }
        if let Some(v) = self.get(e, "height", path)? {
            st.size.height = Self::dim(&v).unwrap_or(auto());
        }
        for (k, apply) in [("min_width", 0), ("min_height", 1), ("max_width", 2), ("max_height", 3)] {
            if let Some(v) = self.get(e, k, path)? {
                let d = Self::lpa(&v);
                match apply {
                    0 => st.min_size.width = d,
                    1 => st.min_size.height = d,
                    2 => st.max_size.width = d,
                    _ => st.max_size.height = d,
                }
            }
        }
        if let Some(v) = self.f(e, "grow", path)? {
            st.flex_grow = v;
        }
        if let Some(v) = self.f(e, "shrink", path)? {
            st.flex_shrink = v;
        }
        if let Some(v) = self.get(e, "basis", path)? {
            st.flex_basis = Self::dim(&v).unwrap_or(auto());
        }
        if let Some(v) = self.f(e, "aspect", path)? {
            st.aspect_ratio = Some(v);
        }
        let abs = self.text(e, "position", path)?.as_deref() == Some("absolute");
        let has_inset = ["inset", "left", "top", "right", "bottom"].iter().any(|k| e.attrs.contains_key(*k));
        let implicit_fill = matches!(e.ty.as_str(), "hand" | "ticks" | "arc") && !e.attrs.contains_key("width") && !e.attrs.contains_key("height");
        if abs || has_inset || implicit_fill {
            st.position = Position::Absolute;
            let mut ins = [auto(), auto(), auto(), auto()]; // l t r b
            if let Some(v) = self.get(e, "inset", path)? {
                let s = Self::sides(&v); // t r b l order for lists; scalars fill all
                ins = [Self::lpa(&s[3]), Self::lpa(&s[0]), Self::lpa(&s[1]), Self::lpa(&s[2])];
            } else if implicit_fill && !has_inset {
                ins = [length(0.0), length(0.0), length(0.0), length(0.0)];
            }
            for (i, k) in ["left", "top", "right", "bottom"].iter().enumerate() {
                if let Some(v) = self.get(e, k, path)? {
                    ins[i] = Self::lpa(&v);
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
                n.look.fill2 = Some(self.color_of(&l[1], &format!("{path}.fill")));
            }
            Some(v) => n.look.fill = self.color_of(&v, &format!("{path}.fill")),
            None => {}
        }
        // absolute alpha for the fill only, so a see-through card keeps opaque children
        // (negative = keep the theme's own alpha)
        if let Some(a) = self.f(e, "fill_alpha", path)?.filter(|a| *a >= 0.0) {
            n.look.fill = n.look.fill.with_alpha(a.clamp(0.0, 1.0));
            n.look.fill2 = n.look.fill2.map(|c| c.with_alpha(a.clamp(0.0, 1.0)));
        }
        if let Some(w) = self.f(e, "border", path)? {
            n.look.border = w;
            n.look.border_color = self.col(e, "border_color", path)?.unwrap_or_else(|| self.theme.color("border"));
        }
        if let Some(r) = self.f(e, "radius", path)? {
            n.look.radius = r;
        }
        if let Some(o) = self.f(e, "opacity", path)? {
            n.look.opacity = o;
        }
        if let Some(a) = e.attrs.get("shadow") {
            let shadow_col = self.theme.color("shadow");
            n.look.shadow = match a {
                Attr::Table(t) => {
                    let mut num = |k: &str, d: f32| -> Result<f32, String> {
                        Ok(t.get(k).map(|a| self.val(a, &format!("{path}.shadow.{k}"))).transpose()?.and_then(|v| v.as_f64()).map_or(d, |x| x as f32))
                    };
                    let (blur, dy) = (num("blur", 12.0)?, num("dy", 4.0)?);
                    let c = match t.get("color") {
                        Some(a) => {
                            let v = self.val(a, &format!("{path}.shadow.color"))?;
                            self.color_of(&v, path)
                        }
                        None => shadow_col,
                    };
                    Some(Shadow { blur, dy, color: c })
                }
                other => {
                    let blur = self.val(other, &format!("{path}.shadow"))?.as_f64().unwrap_or(0.0) as f32;
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
                let v = self.val(a, &ctx)?;
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
                let ms = t.get("ms").map(|a| self.val(a, path)).transpose()?.and_then(|v| v.as_f64()).unwrap_or(150.0) as u32;
                let ease = t.get("ease").map(|a| self.val(a, path)).transpose()?.map(|v| Ease::parse(&v.to_string())).unwrap_or_default();
                n.transition = Transition { ms, ease };
            }
            Some(a) => n.transition = Transition { ms: self.val(a, path)?.as_f64().unwrap_or(0.0) as u32, ease: Ease::Out },
            None => {}
        }
        if let Some(Attr::Table(t)) = e.attrs.get("enter") {
            let mut num = |k: &str, d: f64| -> Result<f64, String> { Ok(t.get(k).map(|a| self.val(a, path)).transpose()?.and_then(|v| v.as_f64()).unwrap_or(d)) };
            let (ms, dy, delay, stagger) = (num("ms", 200.0)?, num("dy", 8.0)?, num("delay", 0.0)?, num("stagger", 0.0)?);
            let idx = self.scope_num("index");
            n.enter = Some(Enter { ms: ms as u32, dy: dy as f32, delay: (delay + stagger * idx) as u32 });
        }
        if let Some(s) = self.f(e, "scroll", path)? {
            n.scroll = Some(s.max(0.0));
        }
        n.overlay = self.flag(e, "overlay", path)?.unwrap_or(false);
        n.hit = self.flag(e, "hit", path)?.unwrap_or(false);
        Ok(())
    }

    fn scope_num(&self, name: &str) -> f64 {
        self.scope.peek(name).and_then(|v| v.as_f64()).unwrap_or(0.0)
    }

    fn weight(v: &Value) -> u16 {
        match v {
            Value::Num(n) => *n as u16,
            Value::Str(s) => match s.as_str() {
                "thin" => 100,
                "light" => 300,
                "medium" => 500,
                "semibold" => 600,
                "bold" => 700,
                "black" => 900,
                _ => 400,
            },
            _ => 400,
        }
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

        match e.ty.as_str() {
            "text" => {
                let theme = self.theme;
                let mut spec = TextSpec { size: theme.num("font-size-md"), color: theme.color("text"), family: theme.str("font-body"), ..Default::default() };
                spec.text = self.text(e, "text", &path)?.unwrap_or_default();
                if let Some(s) = self.f(e, "size", &path)? {
                    spec.size = s.max(1.0);
                }
                if let Some(c) = self.col(e, "color", &path)? {
                    spec.color = c;
                }
                if let Some(f) = self.text(e, "font", &path)? {
                    spec.family = f;
                }
                if let Some(w) = self.get(e, "weight", &path)? {
                    spec.weight = Self::weight(&w);
                }
                if let Some(a) = self.text(e, "text_align", &path)? {
                    spec.align = TextAlign::parse(&a);
                }
                spec.wrap = self.flag(e, "text_wrap", &path)?.unwrap_or(false);
                if let Some(l) = self.f(e, "line_height", &path)? {
                    spec.line_height = l;
                }
                n.kind = Kind::Text(spec);
            }
            "image" => {
                let id = self.text(e, "src", &path)?.unwrap_or_default();
                self.images.insert(id.clone());
                let (w, h) = (self.image_size)(&id).unwrap_or((32.0, 32.0));
                n.kind = Kind::Image(ImageSpec { id, w, h, tint: self.col(e, "tint", &path)? });
            }
            "hand" => {
                let text = self.theme.color("hand");
                n.kind = Kind::Hand(HandSpec {
                    angle: self.f(e, "angle", &path)?.unwrap_or(0.0),
                    length: self.f(e, "length", &path)?.unwrap_or(0.8),
                    tail: self.f(e, "tail", &path)?.unwrap_or(0.1),
                    width: self.f(e, "stroke", &path)?.unwrap_or(2.0),
                    color: self.col(e, "color", &path)?.unwrap_or(text),
                });
            }
            "arc" => {
                n.kind = Kind::Arc(ArcSpec {
                    start: self.f(e, "start", &path)?.unwrap_or(225.0),
                    sweep: self.f(e, "sweep", &path)?.unwrap_or(270.0).clamp(1.0, 360.0),
                    value: self.f(e, "value", &path)?.unwrap_or(0.0).clamp(0.0, 100.0),
                    width: self.f(e, "stroke", &path)?.unwrap_or(6.0).max(1.0),
                    color: self.col(e, "color", &path)?.unwrap_or_else(|| self.theme.color("accent")),
                    track: self.col(e, "track", &path)?.unwrap_or_else(|| self.theme.color("track")),
                });
            }
            "ticks" => {
                let tick = self.theme.color("tick");
                let hand = self.theme.color("hand");
                n.kind = Kind::Ticks(TicksSpec {
                    count: self.f(e, "count", &path)?.unwrap_or(60.0).max(1.0) as u32,
                    major_every: self.f(e, "major_every", &path)?.unwrap_or(5.0) as u32,
                    len: self.f(e, "tick_length", &path)?.unwrap_or(5.0),
                    major_len: self.f(e, "major_length", &path)?.unwrap_or(10.0),
                    width: self.f(e, "tick_width", &path)?.unwrap_or(1.2),
                    major_width: self.f(e, "major_width", &path)?.unwrap_or(2.4),
                    color: self.col(e, "color", &path)?.unwrap_or(tick),
                    major_color: self.col(e, "major_color", &path)?.unwrap_or(hand),
                    inset: self.f(e, "tick_inset", &path)?.unwrap_or(8.0),
                });
            }
            _ => {}
        }
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
        Theme::compose(&Library::load(Path::new("nope")), &Selection::default(), &BTreeMap::new())
    }

    fn build_src(src: &str, params: &[(&str, Value)]) -> Result<Built, String> {
        let def = WidgetDef::parse("t", src)?;
        let p: BTreeMap<String, Value> = params.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        let st = BTreeMap::new();
        let inp = Inputs {
            params: &p,
            state: &st,
            size: (100.0, 60.0),
            key: "t",
            clock: Value::obj([("minute", 7.into()), ("second", 3.into())]),
            sys: Value::default(),
            shortcuts: crate::data::shortcuts_value(&[], "Default"),
        };
        build(&def, &inp, &theme(), &|_| None)
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
        // the build is the card alone, sized to the card; `card::Card::dress` adds the window
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
        assert_eq!((l.fill.to_hex().as_str(), l.fill.0[3], l.fill2.unwrap().0[3]), ("#6ea8ff80", 0.5, 0.5));
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
