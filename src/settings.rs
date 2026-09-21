//! The settings window, built on the same renderer as the widgets (decision 6)
//! from the Rust builder API (decision 9). `UiState` holds all the logic with
//! no window or GPU, so it is unit-testable; `SettingsWin` is the thin shell.
//!
//! Every interactive node carries an action string (`nav:widgets`,
//! `tog:clock-1|ticks`, ...). `UiState::act` turns an action into `Cmd`s that
//! the app applies; the window never mutates app state directly.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::{CursorIcon, Window, WindowAttributes};

use crate::anim::{Anim, Ease};
use crate::color::Color;
use crate::data::Shortcut;
use crate::dialog;
use crate::format::{ParamDef, ParamType, WidgetDef};
use crate::gfx::{Gpu, Power, RenderError, Target};
use crate::icons::IconService;
use crate::text::TextEngine;
use crate::theme::{Library, Selection, Theme};
use crate::ui::{self, Env, Frame, Kind, Node};
use crate::value::Value;
use crate::widgets::Registry;
use crate::workspace::Workspace;

/// Everything the settings window may read.
pub struct Ctx<'a> {
    pub ws: &'a Workspace,
    pub reg: &'a Registry,
    pub lib: &'a Library,
    pub theme: &'a Theme,
    pub log: &'a [String],
    pub gpu_info: &'a str,
    pub fonts: &'a [String],
    pub edit: bool,
    pub parked: &'a [String],
}

#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    Add(String),
    Remove(String),
    Param(String, String, Value),
    Items(String, Vec<Shortcut>),
    Z(String, String),
    ClickThrough(String, bool),
    ResetPos(String),
    Theme(Selection),
    Override(String, Option<String>),
    Gpu(String),
    Autostart(bool),
    Grid(f32),
    Edit(bool),
    Reload,
    OpenFolder,
    Quit,
    Close,
    Minimize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Widgets,
    Appearance,
    General,
    Log,
}

impl Page {
    const ALL: [Page; 4] = [Page::Widgets, Page::Appearance, Page::General, Page::Log];

    fn id(self) -> &'static str {
        match self {
            Page::Widgets => "widgets",
            Page::Appearance => "appearance",
            Page::General => "general",
            Page::Log => "log",
        }
    }
    fn title(self) -> &'static str {
        match self {
            Page::Widgets => "Widgets",
            Page::Appearance => "Appearance",
            Page::General => "General",
            Page::Log => "Log",
        }
    }
    fn subtitle(self) -> &'static str {
        match self {
            Page::Widgets => "Add, arrange and configure the widgets on your desktop",
            Page::Appearance => "Colours, fonts and icons, applied to every widget at once",
            Page::General => "Graphics, startup and behaviour",
            Page::Log => "What Wayfinder has been doing, and anything that went wrong",
        }
    }
    fn glyph(self) -> &'static str {
        match self {
            Page::Widgets => "widgets",
            Page::Appearance => "palette",
            Page::General => "gear",
            Page::Log => "info",
        }
    }
    fn parse(s: &str) -> Page {
        Page::ALL.into_iter().find(|p| p.id() == s).unwrap_or(Page::Widgets)
    }
}

struct Focus {
    key: String,
    text: String,
    caret: usize,
}

enum Open {
    Dropdown(String),
    Color(String),
}

// ---- sizes -----------------------------------------------------------------------

const GUTTER: f32 = 24.0;
const WIN: (f32, f32) = (960.0, 680.0);
const NAV_W: f32 = 196.0;
const LABEL_W: f32 = 132.0;
const CONTROL_W: f32 = 250.0;
const SV_N: usize = 14;
const HUE_N: usize = 28;

// ---- pure helpers -------------------------------------------------------------------

/// Map a 0..1 fraction to a value snapped to `step` within `[min, max]`.
pub fn slider_value(frac: f32, min: f64, max: f64, step: f64) -> f64 {
    let raw = min + (max - min) * frac.clamp(0.0, 1.0) as f64;
    let v = if step > 0.0 { (raw / step).round() * step } else { raw };
    v.clamp(min, max)
}

fn fmt_num(v: f64) -> String {
    if v.fract().abs() < 1e-9 { format!("{}", v as i64) } else { format!("{v:.2}") }
}

fn insert_at(s: &mut String, caret: usize, t: &str) -> usize {
    let c = caret.min(s.len());
    s.insert_str(c, t);
    c + t.len()
}

fn prev_boundary(s: &str, i: usize) -> usize {
    s[..i.min(s.len())].char_indices().next_back().map_or(0, |(b, _)| b)
}

fn next_boundary(s: &str, i: usize) -> usize {
    s[i.min(s.len())..].chars().next().map_or(s.len(), |c| i + c.len_utf8())
}

// ---- controls kit -----------------------------------------------------------------------

struct Kit<'a> {
    t: &'a Theme,
}

impl Kit<'_> {
    fn c(&self, n: &str) -> Color {
        self.t.color(n)
    }

    fn txt(&self, key: String, s: &str, size: f32, col: Color) -> Node {
        let fam = self.t.str("font-body");
        Node::text(key, s, size, col).with_text(|t| t.family = fam)
    }

    fn bold(&self, key: String, s: &str, size: f32, col: Color) -> Node {
        self.txt(key, s, size, col).with_text(|t| t.weight = 600)
    }

    fn glyph(&self, key: String, name: &str, size: f32, col: Color) -> Node {
        let (fam, ch) = (self.t.str("font-glyph"), self.t.str(&format!("glyph-{name}")));
        Node::text(key, ch, size, col).with_text(|t| t.family = fam)
    }

    fn button(&self, key: &str, label: &str, action: String, primary: bool) -> Node {
        let (fill, txt) = if primary { (self.c("accent"), self.c("accent-text")) } else { (Color([1.0, 1.0, 1.0, 0.08]), self.c("text")) };
        let hover = if primary { fill.mul_alpha(0.86) } else { Color([1.0, 1.0, 1.0, 0.16]) };
        Node::new(key).h(32.0).pad_xy(14.0, 0.0).center().radius(8.0).fill(fill).hover_fill(hover).ease(120).on(action).child(self.bold(format!("{key}/t"), label, 13.0, txt))
    }

    fn icon_button(&self, key: &str, glyph: &str, action: String, danger: bool) -> Node {
        let col = if danger { self.c("danger") } else { self.c("text-dim") };
        Node::new(key).wh(30.0, 30.0).center().radius(15.0).hover_fill(Color([1.0, 1.0, 1.0, 0.12])).ease(120).on(action).child(self.glyph(format!("{key}/g"), glyph, 13.0, col))
    }

    fn toggle(&self, key: &str, on: bool, action: String) -> Node {
        let track = if on { self.c("accent") } else { self.c("track") };
        let knob = Node::new(format!("{key}/k"))
            .wh(18.0, 18.0)
            .radius(9.0)
            .fill(if on { self.c("accent-text") } else { Color([1.0, 1.0, 1.0, 0.92]) })
            .abs(Some(3.0), Some(3.0), None, None)
            .offset(if on { 20.0 } else { 0.0 }, 0.0)
            .transition(220, Ease::Back)
            .shadow(3.0, 1.0, Color([0.0, 0.0, 0.0, 0.3]));
        Node::new(key).wh(46.0, 24.0).radius(12.0).fill(track).ease(180).on(action).child(knob)
    }

    fn slider(&self, key: &str, frac: f32, w: f32, action: String) -> Node {
        let f = frac.clamp(0.0, 1.0);
        let fill = Node::new(format!("{key}/f")).abs(Some(0.0), Some(0.0), None, Some(0.0)).w(w * f).radius(3.0).fill(self.c("accent"));
        let thumb = Node::new(format!("{key}/th"))
            .wh(16.0, 16.0)
            .radius(8.0)
            .fill(Color([1.0, 1.0, 1.0, 1.0]))
            .abs(Some(w * f - 8.0), Some(-5.0), None, None)
            .shadow(4.0, 1.0, Color([0.0, 0.0, 0.0, 0.4]));
        let rail = Node::new(format!("{key}/r")).w(w).h(6.0).radius(3.0).fill(self.c("track")).child(fill).child(thumb);
        // the taller wrapper is the hit target and the rect the drag maps onto
        Node::new(key).w(w).h(24.0).align(taffy::AlignItems::CENTER).on(action).child(rail)
    }

    fn input(&self, key: &str, text: &str, placeholder: &str, focus: Option<(usize, bool)>, w: f32, mono: bool) -> Node {
        let focused = focus.is_some();
        let fam = if mono { self.t.str("font-mono") } else { self.t.str("font-body") };
        let shown = if text.is_empty() && !focused { placeholder } else { text };
        let col = if text.is_empty() { self.c("text-dim").mul_alpha(0.7) } else { self.c("text") };
        let caret = focus.and_then(|(c, on)| on.then_some(c));
        let mut t = Node::text(format!("{key}/t"), shown, 13.0, col).with_text(|s| {
            s.family = fam;
            s.caret = caret;
        });
        if let Kind::Text(s) = &mut t.kind {
            s.wrap = false;
        }
        Node::new(key)
            .w(w)
            .h(32.0)
            .pad_xy(10.0, 0.0)
            .align(taffy::AlignItems::CENTER)
            .radius(8.0)
            .fill(Color([0.0, 0.0, 0.0, 0.22]))
            .border(if focused { 2.0 } else { 1.0 }, if focused { self.c("accent") } else { self.c("border") })
            .ease(120)
            .clip()
            .on(format!("in:{key}"))
            .child(t)
    }

    fn dropdown(&self, key: &str, label: &str, w: f32, open: bool) -> Node {
        Node::new(key)
            .w(w)
            .h(32.0)
            .row()
            .align(taffy::AlignItems::CENTER)
            .pad_xy(10.0, 0.0)
            .gap(6.0)
            .radius(8.0)
            .fill(Color([0.0, 0.0, 0.0, 0.22]))
            .hover_fill(Color([1.0, 1.0, 1.0, 0.08]))
            .border(if open { 2.0 } else { 1.0 }, if open { self.c("accent") } else { self.c("border") })
            .ease(120)
            .on(format!("dd:{key}"))
            .child(self.txt(format!("{key}/t"), label, 13.0, self.c("text")).grow_text())
            .child(self.glyph(format!("{key}/g"), "chevron-down", 11.0, self.c("text-dim")))
    }

    fn swatch(&self, key: &str, col: Color, size: f32, action: Option<String>, selected: bool) -> Node {
        let mut n = Node::new(key).wh(size, size).radius(size / 2.0).fill(col).border(if selected { 2.0 } else { 1.0 }, if selected { self.c("text") } else { Color([1.0, 1.0, 1.0, 0.3]) }).ease(120);
        if let Some(a) = action {
            n = n.on(a).hover_fill(col.mul_alpha(0.8));
        }
        n
    }

    /// A labelled settings row: label + help on the left, the control on the right.
    fn row(&self, key: &str, label: &str, help: &str, control: Node) -> Node {
        let mut left = Node::new(format!("{key}/l")).col().w(LABEL_W).gap(2.0).no_shrink().child(self.txt(format!("{key}/lt"), label, 13.0, self.c("text")).wrap_text());
        if !help.is_empty() {
            left = left.child(self.txt(format!("{key}/lh"), help, 11.5, self.c("text-dim")).wrap_text());
        }
        Node::new(key).row().gap(14.0).align(taffy::AlignItems::FLEX_START).pad_xy(0.0, 6.0).child(left).child(Node::new(format!("{key}/ctl")).col().gap(8.0).child(control))
    }

    fn section(&self, key: &str, title: &str) -> Node {
        Node::new(key)
            .col()
            .gap(6.0)
            .pad_xy(0.0, 6.0)
            .child(self.bold(format!("{key}/t"), &title.to_uppercase(), 11.0, self.c("text-dim")).with_text(|t| t.weight = 700))
            .child(Node::new(format!("{key}/d")).h(1.0).fill(self.c("border").mul_alpha(0.6)))
    }
}

trait TextExt {
    fn grow_text(self) -> Self;
    fn wrap_text(self) -> Self;
}
impl TextExt for Node {
    fn grow_text(self) -> Self {
        self.grow(1.0)
    }
    fn wrap_text(self) -> Self {
        self.with_text(|t| t.wrap = true)
    }
}

// ---- UI state -------------------------------------------------------------------------------

pub struct UiState {
    pub page: Page,
    pub selected: Option<String>,
    pub scroll: HashMap<String, f32>,
    focus: Option<Focus>,
    open: Option<Open>,
    confirm_del: Option<String>,
    hsv: (f32, f32, f32),
    pub caret_on: bool,
    pub caret_at: Instant,
    pub mods: ModifiersState,
    /// Last-layout rects of popup anchors, so a popup can position itself.
    pub anchors: HashMap<String, (f32, f32, f32, f32)>,
}

impl Default for UiState {
    fn default() -> Self {
        Self { page: Page::Widgets, selected: None, scroll: HashMap::new(), focus: None, open: None, confirm_del: None, hsv: (0.6, 0.6, 1.0), caret_on: true, caret_at: Instant::now(), mods: ModifiersState::empty(), anchors: HashMap::new() }
    }
}

impl UiState {
    pub fn wants_caret(&self) -> bool {
        self.focus.is_some()
    }

    pub fn has_popup(&self) -> bool {
        self.open.is_some()
    }

    pub fn anchor_key(&self) -> String {
        match &self.open {
            Some(Open::Dropdown(k)) => k.clone(),
            Some(Open::Color(t)) => format!("cp:{t}"),
            None => String::new(),
        }
    }

    fn selected_cfg<'a>(&self, ctx: &'a Ctx) -> Option<&'a crate::workspace::InstanceCfg> {
        let id = self.selected.as_deref().or_else(|| ctx.ws.instances.first().map(|c| c.id.as_str()))?;
        ctx.ws.instances.iter().find(|c| c.id == id)
    }

    fn def_of<'a>(&self, ctx: &'a Ctx, widget: &str) -> Option<&'a Arc<WidgetDef>> {
        ctx.reg.get(widget).and_then(|d| d.as_ref().ok())
    }

    fn param_value(cfg: &crate::workspace::InstanceCfg, p: &ParamDef) -> Value {
        cfg.params.get(&p.name).map(Value::from).unwrap_or_else(|| p.default.clone())
    }

    fn resolve_color(ctx: &Ctx, v: &Value) -> Color {
        let s = v.to_string();
        match s.strip_prefix('$') {
            Some(tok) => ctx.theme.color(tok),
            None => Color::parse(&s).unwrap_or(crate::color::MAGENTA),
        }
    }

    // -- data for popups and value-bearing controls, keyed by control key --

    fn dropdown_items(&self, ctx: &Ctx, key: &str) -> Vec<(String, String)> {
        let two = |v: &[(&str, &str)]| v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        if let Some(_id) = key.strip_prefix("z:") {
            return two(&[("desktop", "On the desktop (behind windows)"), ("bottom", "Bottom of the window stack"), ("normal", "Normal window"), ("topmost", "Always on top")]);
        }
        if key == "gpu" {
            return two(&[("low", "Integrated GPU (recommended)"), ("high", "Dedicated GPU"), ("software", "Software (CPU, slow)")]);
        }
        match key {
            "th:palette" => return ctx.lib.palettes.iter().map(|a| (a.name.clone(), a.name.clone())).collect(),
            "th:fonts" => return ctx.lib.fonts.iter().map(|a| (a.name.clone(), a.name.clone())).collect(),
            "th:glyphs" => return ctx.lib.glyphs.iter().map(|a| (a.name.clone(), a.name.clone())).collect(),
            "th:pack" => return ctx.lib.icon_packs.iter().map(|n| (n.clone(), n.clone())).collect(),
            _ => {}
        }
        if let Some(rest) = key.strip_prefix("p:") {
            if let Some((id, name)) = rest.split_once(':') {
                let def = ctx.ws.instances.iter().find(|c| c.id == id).and_then(|c| self.def_of(ctx, &c.widget));
                if let Some(p) = def.and_then(|d| d.params.iter().find(|p| p.name == name)) {
                    return match p.ty {
                        ParamType::Font => std::iter::once((String::new(), "Theme font".to_string())).chain(ctx.fonts.iter().map(|f| (f.clone(), f.clone()))).collect(),
                        _ => p.choices.iter().map(|c| (c.clone(), c.clone())).collect(),
                    };
                }
            }
        }
        Vec::new()
    }

    fn dropdown_current(&self, ctx: &Ctx, key: &str) -> String {
        if let Some(id) = key.strip_prefix("z:") {
            return ctx.ws.instances.iter().find(|c| c.id == id).map(|c| c.z.clone()).unwrap_or_default();
        }
        match key {
            "gpu" => return ctx.ws.gpu.clone(),
            "th:palette" => return ctx.ws.theme.palette.clone(),
            "th:fonts" => return ctx.ws.theme.fonts.clone(),
            "th:glyphs" => return ctx.ws.theme.glyphs.clone(),
            "th:pack" => return ctx.ws.theme.icon_pack.clone(),
            _ => {}
        }
        if let Some(rest) = key.strip_prefix("p:") {
            if let Some((id, name)) = rest.split_once(':') {
                if let Some(cfg) = ctx.ws.instances.iter().find(|c| c.id == id) {
                    return cfg.params.get(name).map(|v| Value::from(v).to_string()).unwrap_or_default();
                }
            }
        }
        String::new()
    }

    fn dropdown_label(&self, ctx: &Ctx, key: &str) -> String {
        let cur = self.dropdown_current(ctx, key);
        self.dropdown_items(ctx, key).into_iter().find(|(v, _)| *v == cur).map(|(_, l)| l).unwrap_or(if cur.is_empty() { "Theme font".into() } else { cur })
    }

    fn color_of_target(&self, ctx: &Ctx, target: &str) -> Color {
        if let Some(tok) = target.strip_prefix("ov:") {
            return ctx.ws.overrides.get(tok).and_then(|s| Color::parse(s)).unwrap_or_else(|| ctx.theme.color(tok));
        }
        if let Some((id, name)) = target.strip_prefix("p:").and_then(|r| r.split_once(':')) {
            if let Some(cfg) = ctx.ws.instances.iter().find(|c| c.id == id) {
                if let Some(p) = self.def_of(ctx, &cfg.widget).and_then(|d| d.params.iter().find(|p| p.name == name)) {
                    return Self::resolve_color(ctx, &Self::param_value(cfg, p));
                }
            }
        }
        crate::color::MAGENTA
    }

    fn color_cmd(target: &str, hex: String) -> Option<Cmd> {
        if let Some(tok) = target.strip_prefix("ov:") {
            return Some(Cmd::Override(tok.to_string(), Some(hex)));
        }
        let (id, name) = target.strip_prefix("p:")?.split_once(':')?;
        Some(Cmd::Param(id.to_string(), name.to_string(), Value::Str(hex)))
    }

    /// (min, max, step, current) for a slider key.
    fn slider_spec(&self, ctx: &Ctx, key: &str) -> Option<(f64, f64, f64, f64)> {
        if key == "grid" {
            return Some((0.0, 32.0, 4.0, ctx.ws.grid as f64));
        }
        if let Some(tok) = key.strip_prefix("ov:") {
            let cur = ctx.ws.overrides.get(tok).and_then(|s| s.parse().ok()).unwrap_or(ctx.theme.num(tok) as f64);
            return Some((0.0, 40.0, 1.0, cur));
        }
        let (id, name) = key.strip_prefix("p:")?.split_once(':')?;
        let cfg = ctx.ws.instances.iter().find(|c| c.id == id)?;
        let p = self.def_of(ctx, &cfg.widget)?.params.iter().find(|p| p.name == name)?.clone();
        let cur = Self::param_value(cfg, &p).as_f64().unwrap_or(0.0);
        Some((p.min.unwrap_or(0.0), p.max.unwrap_or(100.0), p.step.unwrap_or(1.0), cur))
    }

    fn slider_cmd(key: &str, v: f64) -> Option<Cmd> {
        if key == "grid" {
            return Some(Cmd::Grid(v as f32));
        }
        if let Some(tok) = key.strip_prefix("ov:") {
            return Some(Cmd::Override(tok.to_string(), Some(fmt_num(v))));
        }
        let (id, name) = key.strip_prefix("p:")?.split_once(':')?;
        Some(Cmd::Param(id.to_string(), name.to_string(), Value::Num(v)))
    }

    fn input_text(&self, ctx: &Ctx, key: &str) -> String {
        if let Some(f) = &self.focus {
            if f.key == key {
                return f.text.clone();
            }
        }
        if let Some(t) = key.strip_prefix("hx:") {
            return self.color_of_target(ctx, t).to_hex();
        }
        if let Some(rest) = key.strip_prefix("n:") {
            if let Some((id, name)) = rest.split_once(':') {
                return ctx.ws.instances.iter().find(|c| c.id == id).and_then(|c| c.params.get(name)).map(|v| Value::from(v).to_string()).unwrap_or_default();
            }
        }
        for (prefix, name_field) in [("sn:", true), ("st:", false)] {
            if let Some(rest) = key.strip_prefix(prefix) {
                if let Some((id, idx)) = rest.split_once(':') {
                    let items = ctx.ws.instances.iter().find(|c| c.id == id).map(|c| c.items()).unwrap_or_default();
                    return items.get(idx.parse::<usize>().unwrap_or(usize::MAX)).map(|s| if name_field { s.name.clone() } else { s.target.clone() }).unwrap_or_default();
                }
            }
        }
        String::new()
    }

    /// Turn edited text into the command that stores it.
    fn commit_text(&self, ctx: &Ctx, key: &str, text: &str) -> Vec<Cmd> {
        if let Some(t) = key.strip_prefix("hx:") {
            let s = if text.starts_with('#') { text.to_string() } else { format!("#{text}") };
            return match Color::parse(&s) {
                Some(c) => Self::color_cmd(t, c.to_hex()).into_iter().collect(),
                None => vec![],
            };
        }
        if let Some((id, name)) = key.strip_prefix("n:").and_then(|r| r.split_once(':')) {
            return vec![Cmd::Param(id.into(), name.into(), Value::Str(text.into()))];
        }
        for (prefix, name_field) in [("sn:", true), ("st:", false)] {
            if let Some((id, idx)) = key.strip_prefix(prefix).and_then(|r| r.split_once(':')) {
                let mut items = ctx.ws.instances.iter().find(|c| c.id == id).map(|c| c.items()).unwrap_or_default();
                if let Some(it) = items.get_mut(idx.parse::<usize>().unwrap_or(usize::MAX)) {
                    if name_field { it.name = text.into() } else { it.target = text.into() }
                    return vec![Cmd::Items(id.into(), items)];
                }
            }
        }
        vec![]
    }

    // -- the tree --

    fn scroll_of(&self, key: &str) -> f32 {
        self.scroll.get(key).copied().unwrap_or(0.0)
    }

    /// The whole window as a node tree, plus the image ids it needs uploaded.
    pub fn build(&self, ctx: &Ctx, size: (f32, f32)) -> (Node, Vec<String>) {
        let k = Kit { t: ctx.theme };
        let mut images = Vec::new();
        let card_size = (size.0 - 2.0 * GUTTER, size.1 - 2.0 * GUTTER);
        let idx = Page::ALL.iter().position(|p| *p == self.page).unwrap_or(0);

        // sidebar
        let mut nav = Node::new("s/nav").col().w(NAV_W).no_shrink().pad(14.0).gap(4.0).fill(Color([0.0, 0.0, 0.0, 0.16]));
        nav = nav.child(
            Node::new("s/brand")
                .row()
                .align(taffy::AlignItems::CENTER)
                .gap(10.0)
                .pad_xy(8.0, 10.0)
                .child(Node::new("s/logo").wh(28.0, 28.0).radius(9.0).fill(k.c("accent")).center().child(k.glyph("s/logo/g".into(), "launch", 14.0, k.c("accent-text"))))
                .child(k.bold("s/brand/t".into(), "Wayfinder", 16.0, k.c("text"))),
        );
        let mut list = Node::new("s/nav/list").col().gap(4.0).pad_xy(0.0, 8.0);
        for p in Page::ALL {
            let sel = p == self.page;
            list = list.child(
                Node::new(format!("s/nav/{}", p.id()))
                    .row()
                    .h(40.0)
                    .align(taffy::AlignItems::CENTER)
                    .gap(12.0)
                    .pad_xy(14.0, 0.0)
                    .radius(10.0)
                    .fill(if sel { k.c("accent").with_alpha(0.16) } else { Color([1.0, 1.0, 1.0, 0.0]) })
                    .hover_fill(if sel { k.c("accent").with_alpha(0.22) } else { Color([1.0, 1.0, 1.0, 0.07]) })
                    .ease(170)
                    .on(format!("nav:{}", p.id()))
                    .child(k.glyph(format!("s/nav/{}/g", p.id()), p.glyph(), 15.0, if sel { k.c("accent") } else { k.c("text-dim") }))
                    .child(k.txt(format!("s/nav/{}/t", p.id()), p.title(), 14.0, if sel { k.c("text") } else { k.c("text-dim") }).with_text(|t| t.weight = if sel { 600 } else { 400 })),
            );
        }
        // a pill that slides to the selected page
        list = list.child(Node::new("s/nav/ind").wh(3.0, 20.0).radius(2.0).fill(k.c("accent")).abs(Some(0.0), Some(8.0 + 10.0), None, None).offset(0.0, idx as f32 * 44.0).transition(300, Ease::Back));
        nav = nav.child(list).child(Node::new("s/nav/sp").grow(1.0));
        nav = nav
            .child(self.edit_button(&k, ctx))
            .child(Node::new("s/nav/gap").h(6.0))
            .child(k.button("s/quit", "Quit Wayfinder", "quit".into(), false).with_danger(&k));

        // title bar (drag region) + page
        let titlebar = Node::new("s/title")
            .row()
            .h(64.0)
            .no_shrink()
            .align(taffy::AlignItems::CENTER)
            .pad_xy(24.0, 0.0)
            .gap(6.0)
            .on("drag")
            .child(Node::new("s/title/l").col().grow(1.0).gap(1.0).child(k.bold("s/title/t".into(), self.page.title(), 19.0, k.c("text"))).child(k.txt("s/title/s".into(), self.page.subtitle(), 12.0, k.c("text-dim"))))
            .child(k.icon_button("s/min", "minimize", "min".into(), false))
            .child(k.icon_button("s/close", "close", "close".into(), false));

        let body = match self.page {
            Page::Widgets => self.page_widgets(&k, ctx, &mut images),
            Page::Appearance => self.page_appearance(&k, ctx),
            Page::General => self.page_general(&k, ctx),
            Page::Log => self.page_log(&k, ctx),
        };
        // the whole page fades and slides in when the page changes (new key => enter replays)
        let page_root = Node::new(format!("s/page/{}", self.page.id())).col().grow(1.0).enter(240, 10.0, 0).child(body);
        let content = Node::new("s/content").col().grow(1.0).child(titlebar).child(page_root);

        let card = Node::new("s/card")
            .row()
            .wh(card_size.0, card_size.1)
            .fill(k.c("surface"))
            .radius(22.0)
            .border(1.0, k.c("border"))
            .shadow(26.0, 10.0, k.c("shadow"))
            .clip()
            .child(nav)
            .child(content);
        let mut root = Node::new("s").wh(size.0, size.1).pad(GUTTER).child(card);

        if let Some(o) = &self.open {
            root = root.child(self.popup(&k, ctx, o, size));
        }
        (root, images)
    }

    fn edit_button(&self, k: &Kit, ctx: &Ctx) -> Node {
        let on = ctx.edit;
        Node::new("s/edit")
            .row()
            .h(40.0)
            .align(taffy::AlignItems::CENTER)
            .gap(12.0)
            .pad_xy(14.0, 0.0)
            .radius(10.0)
            .fill(if on { k.c("accent") } else { Color([1.0, 1.0, 1.0, 0.07]) })
            .hover_fill(if on { k.c("accent").mul_alpha(0.86) } else { Color([1.0, 1.0, 1.0, 0.13]) })
            .ease(160)
            .on("edit:toggle")
            .child(k.glyph("s/edit/g".into(), "move", 15.0, if on { k.c("accent-text") } else { k.c("text") }))
            .child(k.bold("s/edit/t".into(), if on { "Finish editing" } else { "Edit layout" }, 13.0, if on { k.c("accent-text") } else { k.c("text") }))
    }

    fn scrolling(&self, key: &str, content: Node) -> Node {
        Node::new(key).col().grow(1.0).scroll(self.scroll_of(key)).hit().child(content)
    }

    // -- pages --

    fn page_widgets(&self, k: &Kit, ctx: &Ctx, images: &mut Vec<String>) -> Node {
        let sel = self.selected_cfg(ctx);
        // left: instance list + add
        let mut list = Node::new("w/list").col().gap(4.0);
        if ctx.ws.instances.is_empty() {
            list = list.child(k.txt("w/empty".into(), "No widgets yet. Add one below.", 12.5, k.c("text-dim")).wrap_text());
        }
        for (i, c) in ctx.ws.instances.iter().enumerate() {
            let on = sel.is_some_and(|s| s.id == c.id);
            let name = ctx.reg.get(&c.widget).and_then(|d| d.as_ref().ok()).map_or(c.widget.clone(), |d| d.name.clone());
            let parked = ctx.parked.contains(&c.id);
            list = list.child(
                Node::new(format!("w/i/{}", c.id))
                    .col()
                    .gap(1.0)
                    .pad_xy(12.0, 9.0)
                    .radius(10.0)
                    .fill(if on { k.c("accent").with_alpha(0.16) } else { Color([1.0, 1.0, 1.0, 0.0]) })
                    .hover_fill(if on { k.c("accent").with_alpha(0.22) } else { Color([1.0, 1.0, 1.0, 0.07]) })
                    .border(if on { 1.0 } else { 0.0 }, k.c("accent").with_alpha(0.5))
                    .ease(150)
                    .on(format!("sel:{}", c.id))
                    .enter(220, 6.0, (i as u32) * 24)
                    .child(k.bold(format!("w/i/{}/n", c.id), &name, 13.0, k.c("text")))
                    .child(k.txt(format!("w/i/{}/s", c.id), &format!("{}{}", c.id, if parked { "  ·  parked (monitor missing)" } else { "" }), 11.5, if parked { k.c("danger") } else { k.c("text-dim") })),
            );
        }
        let mut add = Node::new("w/add").col().gap(6.0).child(k.section("w/add/h", "Add a widget"));
        let mut chips = Node::new("w/add/chips").row().wrap().gap(6.0);
        for id in ctx.reg.ids() {
            let label = ctx.reg.get(&id).and_then(|d| d.as_ref().ok()).map_or(id.clone(), |d| d.name.clone());
            chips = chips.child(
                Node::new(format!("w/add/{id}"))
                    .row()
                    .align(taffy::AlignItems::CENTER)
                    .gap(6.0)
                    .h(30.0)
                    .pad_xy(11.0, 0.0)
                    .radius(15.0)
                    .fill(Color([1.0, 1.0, 1.0, 0.07]))
                    .hover_fill(k.c("accent").with_alpha(0.25))
                    .ease(130)
                    .on(format!("add:{id}"))
                    .child(k.glyph(format!("w/add/{id}/g"), "add", 11.0, k.c("accent")))
                    .child(k.txt(format!("w/add/{id}/t"), &label, 12.0, k.c("text"))),
            );
        }
        add = add.child(chips);
        let left = Node::new("w/left").col().w(214.0).no_shrink().gap(10.0).pad_xy(0.0, 0.0).child(self.scrolling("w/scroll-l", Node::new("w/l/c").col().gap(10.0).child(list).child(add)));

        let right = match sel {
            None => Node::new("w/right").col().grow(1.0).center().child(k.txt("w/none".into(), "Select a widget to configure it.", 13.0, k.c("text-dim"))),
            Some(cfg) => Node::new("w/right").col().grow(1.0).child(self.scrolling("w/scroll-r", self.instance_panel(k, ctx, cfg, images))),
        };
        Node::new("w").row().grow(1.0).gap(22.0).pad_xy(24.0, 4.0).child(left).child(right)
    }

    fn instance_panel(&self, k: &Kit, ctx: &Ctx, cfg: &crate::workspace::InstanceCfg, images: &mut Vec<String>) -> Node {
        let id = &cfg.id;
        let def = self.def_of(ctx, &cfg.widget);
        let title = def.map_or(cfg.widget.clone(), |d| d.name.clone());
        let confirm = self.confirm_del.as_deref() == Some(id.as_str());
        let mut p = Node::new(format!("ip/{id}")).col().gap(4.0).pad_xy(0.0, 0.0);
        p = p.child(
            Node::new(format!("ip/{id}/h"))
                .row()
                .align(taffy::AlignItems::CENTER)
                .gap(10.0)
                .child(Node::new(format!("ip/{id}/hl")).col().grow(1.0).child(k.bold(format!("ip/{id}/ht"), &title, 17.0, k.c("text"))).child(k.txt(format!("ip/{id}/hs"), def.map_or("", |d| d.description.as_str()), 12.0, k.c("text-dim")).wrap_text()))
                .child(if confirm {
                    k.button(&format!("ip/{id}/del"), "Really remove?", format!("del:{id}"), true).with_danger_fill(k)
                } else {
                    k.button(&format!("ip/{id}/del"), "Remove", format!("del:{id}"), false)
                }),
        );
        p = p.child(k.section(&format!("ip/{id}/s1"), "Placement"));
        let z = Node::new(format!("ip/{id}/zc")).child(k.dropdown(&format!("z:{id}"), &self.dropdown_label(ctx, &format!("z:{id}")), CONTROL_W, matches!(&self.open, Some(Open::Dropdown(o)) if *o == format!("z:{id}"))));
        p = p.child(k.row(&format!("ip/{id}/z"), "Layer", "Where it sits relative to other windows", z));
        p = p.child(k.row(&format!("ip/{id}/ct"), "Click-through", "Clicks pass to whatever is underneath", k.toggle(&format!("ct:{id}"), cfg.click_through, format!("ct:{id}"))));
        let placement = Node::new(format!("ip/{id}/pl")).row().gap(8.0).child(k.button(&format!("ip/{id}/edit"), "Edit layout", "edit:toggle".into(), false)).child(k.button(&format!("ip/{id}/reset"), "Reset position", format!("reset:{id}"), false));
        p = p.child(k.row(&format!("ip/{id}/pos"), "Position", &format!("{:.0}, {:.0}  ·  {:.0} x {:.0}", cfg.x, cfg.y, cfg.w, cfg.h), placement));

        if let Some(d) = def {
            if !d.params.is_empty() {
                p = p.child(k.section(&format!("ip/{id}/s2"), "Options"));
            }
            for pd in &d.params {
                p = p.child(self.param_row(k, ctx, cfg, pd, images));
            }
        } else {
            p = p.child(k.txt(format!("ip/{id}/err"), "This widget's definition failed to load; see the Log page.", 12.5, k.c("danger")).wrap_text());
        }
        p.child(Node::new(format!("ip/{id}/pad")).h(24.0))
    }

    fn param_row(&self, k: &Kit, ctx: &Ctx, cfg: &crate::workspace::InstanceCfg, pd: &ParamDef, images: &mut Vec<String>) -> Node {
        let id = &cfg.id;
        let name = &pd.name;
        let key = format!("p:{id}:{name}");
        let rk = format!("pr/{id}/{name}");
        let val = Self::param_value(cfg, pd);
        let control = match pd.ty {
            ParamType::Bool => k.toggle(&format!("tg:{id}:{name}"), val.truthy(), format!("tog:{id}|{name}")),
            ParamType::Number | ParamType::Duration => {
                let (min, max, _step, cur) = self.slider_spec(ctx, &key).unwrap_or((0.0, 100.0, 1.0, 0.0));
                let frac = if max > min { ((cur - min) / (max - min)) as f32 } else { 0.0 };
                Node::new(format!("{rk}/sc")).row().align(taffy::AlignItems::CENTER).gap(12.0).child(k.slider(&format!("sl:{key}"), frac, 178.0, format!("sl:{key}"))).child(k.txt(format!("{rk}/v"), &fmt_num(cur), 12.5, k.c("text-dim")).with_text(|t| t.align = crate::text::TextAlign::Right))
            }
            ParamType::Color => {
                let col = Self::resolve_color(ctx, &val);
                let f = self.focus.as_ref().filter(|f| f.key == format!("hx:{key}")).map(|f| (f.caret, self.caret_on));
                Node::new(format!("{rk}/cc"))
                    .row()
                    .align(taffy::AlignItems::CENTER)
                    .gap(8.0)
                    .child(k.swatch(&format!("{rk}/sw"), col, 26.0, Some(format!("cp:{key}")), matches!(&self.open, Some(Open::Color(o)) if *o == key)))
                    .child(k.input(&format!("hx:{key}"), &self.input_text(ctx, &format!("hx:{key}")), "#rrggbb", f, 110.0, true))
            }
            ParamType::Font | ParamType::Enum => k.dropdown(&key, &self.dropdown_label(ctx, &key), CONTROL_W, matches!(&self.open, Some(Open::Dropdown(o)) if *o == key)),
            ParamType::Str => {
                let ik = format!("n:{id}:{name}");
                let f = self.focus.as_ref().filter(|f| f.key == ik).map(|f| (f.caret, self.caret_on));
                k.input(&ik, &self.input_text(ctx, &ik), "", f, CONTROL_W, false)
            }
            ParamType::Path => {
                let ik = format!("n:{id}:{name}");
                let f = self.focus.as_ref().filter(|f| f.key == ik).map(|f| (f.caret, self.caret_on));
                Node::new(format!("{rk}/pc"))
                    .col()
                    .gap(8.0)
                    .child(k.input(&ik, &self.input_text(ctx, &ik), "no folder: use the list below", f, CONTROL_W, false))
                    .child(Node::new(format!("{rk}/pb")).row().gap(8.0).child(k.button(&format!("{rk}/browse"), "Browse...", format!("folder:{id}|{name}"), false)).child(k.button(&format!("{rk}/clear"), "Clear", format!("clear:{id}|{name}"), false)))
            }
            ParamType::Shortcuts => return self.shortcuts_editor(k, ctx, cfg, pd, images),
        };
        k.row(&rk, &pd.label, &pd.help, control)
    }

    fn shortcuts_editor(&self, k: &Kit, ctx: &Ctx, cfg: &crate::workspace::InstanceCfg, pd: &ParamDef, images: &mut Vec<String>) -> Node {
        let id = &cfg.id;
        let items = cfg.items();
        let mirrored = !cfg.folder().is_empty();
        let mut col = Node::new(format!("sc/{id}")).col().gap(6.0).pad_xy(0.0, 6.0);
        col = col.child(
            Node::new(format!("sc/{id}/h"))
                .row()
                .align(taffy::AlignItems::CENTER)
                .child(k.txt(format!("sc/{id}/ht"), &pd.label, 13.0, k.c("text")).grow_text())
                .child(k.button(&format!("sc/{id}/add"), "Add shortcut...", format!("scadd:{id}"), true)),
        );
        if mirrored {
            col = col.child(k.txt(format!("sc/{id}/m"), "A folder is mirrored, so this list is not shown. Clear the folder to use it.", 11.5, k.c("text-dim")).wrap_text());
        }
        if items.is_empty() {
            col = col.child(k.txt(format!("sc/{id}/e"), "No shortcuts. Add an app, file or folder.", 12.0, k.c("text-dim")));
        }
        let pack = &ctx.ws.theme.icon_pack;
        for (i, it) in items.iter().enumerate() {
            let iid = crate::data::icon_id(pack, it);
            images.push(iid.clone());
            let nk = format!("sn:{id}:{i}");
            let tk = format!("st:{id}:{i}");
            let fnm = self.focus.as_ref().filter(|f| f.key == nk).map(|f| (f.caret, self.caret_on));
            let ftg = self.focus.as_ref().filter(|f| f.key == tk).map(|f| (f.caret, self.caret_on));
            let mut img = Node::new(format!("sc/{id}/{i}/img")).wh(28.0, 28.0).no_shrink();
            img.kind = Kind::Image(ui::ImageSpec { id: iid, w: 32.0, h: 32.0, tint: None });
            col = col.child(
                Node::new(format!("sc/{id}/{i}"))
                    .row()
                    .align(taffy::AlignItems::CENTER)
                    .gap(8.0)
                    .pad_xy(8.0, 6.0)
                    .radius(10.0)
                    .fill(Color([1.0, 1.0, 1.0, 0.05]))
                    .enter(200, 6.0, (i as u32).min(8) * 20)
                    .child(img)
                    .child(Node::new(format!("sc/{id}/{i}/f")).col().gap(4.0).child(k.input(&nk, &self.input_text(ctx, &nk), "Name", fnm, 176.0, false)).child(k.input(&tk, &self.input_text(ctx, &tk), "Path or command", ftg, 176.0, true)))
                    .child(Node::new(format!("sc/{id}/{i}/b")).col().child(
                        Node::new(format!("sc/{id}/{i}/br")).row().child(k.icon_button(&format!("sc/{id}/{i}/bf"), "folder", format!("scbrowse:{id}|{i}"), false)).child(k.icon_button(&format!("sc/{id}/{i}/x"), "delete", format!("scdel:{id}|{i}"), true)),
                    )),
            );
        }
        col
    }

    fn page_appearance(&self, k: &Kit, ctx: &Ctx) -> Node {
        let mut cards = Node::new("ap/cards").row().wrap().gap(12.0);
        for (i, a) in ctx.lib.palettes.iter().enumerate() {
            let on = a.name == ctx.ws.theme.palette;
            let chip = |n: &str| a.tokens.get(n).map(|v| v.to_string()).and_then(|s| Color::parse(&s)).unwrap_or(crate::color::MAGENTA);
            let mut chips = Node::new(format!("ap/c/{}/chips", a.name)).row().gap(5.0);
            for tok in ["accent", "text", "text-dim", "border"] {
                chips = chips.child(Node::new(format!("ap/c/{}/{tok}", a.name)).wh(16.0, 16.0).radius(8.0).fill(chip(tok)).border(1.0, Color([1.0, 1.0, 1.0, 0.25])));
            }
            cards = cards.child(
                Node::new(format!("ap/c/{}", a.name))
                    .col()
                    .w(140.0)
                    .gap(10.0)
                    .pad(12.0)
                    .radius(14.0)
                    .fill(chip("surface").with_alpha(1.0))
                    .border(if on { 2.0 } else { 1.0 }, if on { k.c("accent") } else { k.c("border") })
                    .hover_fill(chip("surface-2").with_alpha(1.0))
                    .ease(150)
                    .on(format!("pick:th:palette|{}", a.name))
                    .enter(240, 8.0, (i as u32) * 40)
                    .child(chips)
                    .child(k.bold(format!("ap/c/{}/n", a.name), &a.name, 13.0, chip("text"))),
            );
        }
        let f = |key: &str| matches!(&self.open, Some(Open::Dropdown(o)) if o == key);
        let glyph_axis = ctx.lib.glyphs(&ctx.ws.theme.glyphs);
        let mut gl = Node::new("ap/glyphs").row().gap(14.0).align(taffy::AlignItems::CENTER);
        for (i, g) in ["gear", "close", "folder", "edit", "refresh", "check"].iter().enumerate() {
            let ch = glyph_axis.tokens.get(&format!("glyph-{g}")).map(|v| v.to_string()).unwrap_or_default();
            let fam = glyph_axis.tokens.get("font-glyph").map(|v| v.to_string()).unwrap_or_default();
            gl = gl.child(Node::text(format!("ap/gl/{i}"), ch, 18.0, k.c("text")).with_text(|t| t.family = fam));
        }
        let fonts_axis = ctx.lib.fonts(&ctx.ws.theme.fonts);
        let fam = fonts_axis.tokens.get("font-body").map(|v| v.to_string()).unwrap_or_default();
        let acc = "ov:accent";
        let fk = self.focus.as_ref().filter(|f| f.key == format!("hx:{acc}")).map(|f| (f.caret, self.caret_on));
        let overridden = ctx.ws.overrides.contains_key("accent");
        let rad = self.slider_spec(ctx, "ov:radius-lg").unwrap_or((0.0, 40.0, 1.0, 22.0));
        let mut body = Node::new("ap").col().gap(4.0).pad_xy(24.0, 4.0);
        body = body
            .child(k.section("ap/s1", "Palette"))
            .child(cards)
            .child(k.section("ap/s2", "Typography and icons"))
            .child(k.row("ap/fonts", "Font set", "Body, display and monospace faces", Node::new("ap/fonts/c").col().gap(8.0).child(k.dropdown("th:fonts", &ctx.ws.theme.fonts, CONTROL_W, f("th:fonts"))).child(Node::text("ap/fonts/pv", "The quick brown fox jumps over the lazy dog", 15.0, k.c("text")).with_text(|t| {
                t.family = fam;
                t.wrap = true;
            }))))
            .child(k.row("ap/glyphs-row", "Glyph set", "Icons for buttons and chrome", Node::new("ap/glyphs/c").col().gap(8.0).child(k.dropdown("th:glyphs", &ctx.ws.theme.glyphs, CONTROL_W, f("th:glyphs"))).child(gl)))
            .child(k.row("ap/pack", "Icon pack", "Replaces app icons; drop PNGs named like the app into Wayfinder/iconpacks/<name>/", k.dropdown("th:pack", &ctx.ws.theme.icon_pack, CONTROL_W, f("th:pack"))))
            .child(k.section("ap/s3", "Tweaks"))
            .child(k.row(
                "ap/accent",
                "Accent colour",
                "Overrides the palette's accent everywhere",
                Node::new("ap/accent/c")
                    .row()
                    .align(taffy::AlignItems::CENTER)
                    .gap(8.0)
                    .child(k.swatch("ap/accent/sw", self.color_of_target(ctx, acc), 26.0, Some(format!("cp:{acc}")), matches!(&self.open, Some(Open::Color(o)) if o == acc)))
                    .child(k.input(&format!("hx:{acc}"), &self.input_text(ctx, &format!("hx:{acc}")), "#rrggbb", fk, 110.0, true))
                    .child(k.button("ap/accent/reset", "Reset", "ovreset:accent".into(), false).opacity(if overridden { 1.0 } else { 0.4 })),
            ))
            .child(k.row("ap/radius", "Card roundness", "Corner radius of large cards", Node::new("ap/radius/c").row().align(taffy::AlignItems::CENTER).gap(12.0).child(k.slider("sl:ov:radius-lg", (rad.3 / rad.1) as f32, 178.0, "sl:ov:radius-lg".into())).child(k.txt("ap/radius/v".into(), &fmt_num(rad.3), 12.5, k.c("text-dim")))))
            .child(k.row("ap/resetall", "Reset tweaks", "", k.button("ap/resetall/b", "Clear all overrides", "ovreset:*".into(), false)))
            .child(Node::new("ap/pad").h(24.0));
        self.scrolling("ap/scroll", body)
    }

    fn page_general(&self, k: &Kit, ctx: &Ctx) -> Node {
        let f = |key: &str| matches!(&self.open, Some(Open::Dropdown(o)) if o == key);
        let ws = ctx.ws;
        let grid = self.slider_spec(ctx, "grid").unwrap_or((0.0, 32.0, 4.0, 8.0));
        let note = if ws.gpu == "high" {
            "The dedicated GPU pins one CPU core at idle on some AMD drivers (see docs/adr/0005). Changes apply after a restart."
        } else {
            "Widgets are tiny; the integrated GPU is plenty and keeps the desktop at zero cost. Changes apply after a restart."
        };
        let body = Node::new("gn")
            .col()
            .gap(4.0)
            .pad_xy(24.0, 4.0)
            .child(k.section("gn/s1", "Graphics"))
            .child(k.row("gn/gpu", "Adapter", note, k.dropdown("gpu", &self.dropdown_label(ctx, "gpu"), CONTROL_W, f("gpu"))))
            .child(k.row("gn/info", "In use", "", k.txt("gn/info/t".into(), ctx.gpu_info, 12.0, k.c("text-dim")).wrap_text()))
            .child(k.section("gn/s2", "Behaviour"))
            .child(k.row("gn/auto", "Start with Windows", "Launch quietly into the tray at sign-in", k.toggle("tg:autostart", ws.autostart, "autostart:toggle".into())))
            .child(k.row("gn/grid", "Snap grid", "Edit layout snaps to this many pixels (0 = off). Hold Shift to ignore snapping.", Node::new("gn/grid/c").row().align(taffy::AlignItems::CENTER).gap(12.0).child(k.slider("sl:grid", (grid.3 / grid.1) as f32, 178.0, "sl:grid".into())).child(k.txt("gn/grid/v".into(), &fmt_num(grid.3), 12.5, k.c("text-dim")))))
            .child(k.row("gn/hk", "Edit hotkey", "Toggles Edit layout from anywhere", k.txt("gn/hk/t".into(), "Ctrl + Alt + E", 13.0, k.c("text")).with_text(|t| t.weight = 600)))
            .child(k.section("gn/s3", "Files"))
            .child(k.row("gn/files", "Your widgets", "Drop .toml widget definitions here; they reload as you save", Node::new("gn/files/c").row().gap(8.0).child(k.button("gn/open", "Open folder", "openfolder".into(), false)).child(k.button("gn/reload", "Reload all", "reload".into(), false))))
            .child(Node::new("gn/pad").h(24.0));
        self.scrolling("gn/scroll", body)
    }

    fn page_log(&self, k: &Kit, ctx: &Ctx) -> Node {
        let mut body = Node::new("lg").col().gap(3.0).pad_xy(24.0, 4.0);
        let errors = ctx.reg.errors();
        if !errors.is_empty() {
            body = body.child(k.section("lg/e", "Widget files with errors"));
            for (i, e) in errors.iter().enumerate() {
                body = body.child(k.txt(format!("lg/e/{i}"), e, 12.0, k.c("danger")).wrap_text().with_text(|t| t.family = ctx.theme.str("font-mono")));
            }
        }
        body = body.child(k.section("lg/s", "Activity"));
        if ctx.log.is_empty() {
            body = body.child(k.txt("lg/none".into(), "Nothing yet.", 12.5, k.c("text-dim")));
        }
        for (i, l) in ctx.log.iter().enumerate().rev().take(150) {
            body = body.child(k.txt(format!("lg/{i}"), l, 11.5, k.c("text-dim")).wrap_text().with_text(|t| t.family = ctx.theme.str("font-mono")));
        }
        self.scrolling("lg/scroll", body.child(Node::new("lg/pad").h(24.0)))
    }

    // -- popups --

    fn popup(&self, k: &Kit, ctx: &Ctx, open: &Open, size: (f32, f32)) -> Node {
        let scrim = Node::new("s/ov/scrim").abs_fill().overlay().on("popup-close");
        let mut root = Node::new("s/ov").abs_fill().overlay().child(scrim);
        match open {
            Open::Dropdown(key) => {
                let items = self.dropdown_items(ctx, key);
                let cur = self.dropdown_current(ctx, key);
                let (ax, ay, aw, ah) = self.anchors.get(key).copied().unwrap_or((size.0 / 2.0 - 120.0, size.1 / 2.0, 240.0, 32.0));
                let n = items.len().max(1);
                let h = (n as f32 * 34.0 + 12.0).min(280.0);
                let below = ay + ah + 6.0 + h <= size.1 - 8.0;
                let y = if below { ay + ah + 6.0 } else { (ay - 6.0 - h).max(8.0) };
                let mut col = Node::new("s/ov/items").col().gap(2.0).pad(6.0);
                for (i, (v, label)) in items.iter().enumerate() {
                    let on = *v == cur;
                    col = col.child(
                        Node::new(format!("s/ov/i/{i}"))
                            .row()
                            .h(32.0)
                            .no_shrink()
                            .align(taffy::AlignItems::CENTER)
                            .pad_xy(10.0, 0.0)
                            .gap(8.0)
                            .radius(8.0)
                            .fill(if on { k.c("accent").with_alpha(0.18) } else { Color([1.0, 1.0, 1.0, 0.0]) })
                            .hover_fill(Color([1.0, 1.0, 1.0, 0.1]))
                            .ease(100)
                            .on(format!("pick:{key}|{v}"))
                            .child(k.txt(format!("s/ov/i/{i}/t"), label, 13.0, k.c("text")).grow_text())
                            .child(k.glyph(format!("s/ov/i/{i}/c"), "check", 11.0, if on { k.c("accent") } else { Color([0.0; 4]) })),
                    );
                }
                let list = Node::new("s/ov/list").col().grow(1.0).scroll(self.scroll_of("s/ov/list")).hit().child(col);
                root = root.child(
                    Node::new("s/ov/pop")
                        .col()
                        .abs(Some(ax), Some(y), None, None)
                        .w(aw.max(220.0))
                        .h(h)
                        .radius(12.0)
                        .fill(Color([0.06, 0.07, 0.11, 0.97]))
                        .border(1.0, k.c("border"))
                        .shadow(18.0, 6.0, Color([0.0, 0.0, 0.0, 0.5]))
                        .clip()
                        .enter(160, if below { -6.0 } else { 6.0 }, 0)
                        .hit()
                        .child(list),
                );
            }
            Open::Color(target) => {
                let (ax, ay, aw, ah) = self.anchors.get(&format!("cp:{target}")).copied().unwrap_or((size.0 / 2.0 - 130.0, size.1 / 2.0 - 100.0, 26.0, 26.0));
                let pw = 252.0;
                let ph = 330.0;
                let x = (ax + aw / 2.0 - pw / 2.0).clamp(8.0, size.0 - pw - 8.0);
                let below = ay + ah + 8.0 + ph <= size.1 - 8.0;
                let y = if below { ay + ah + 8.0 } else { (ay - 8.0 - ph).max(8.0) };
                let (h, s, v) = self.hsv;
                let cur = Color::from_hsv(h, s, v);
                let mut grid = Node::new("cp/sv").col().gap(0.0).radius(8.0).clip();
                let cell = 224.0 / SV_N as f32;
                for j in 0..SV_N {
                    let mut row = Node::new(format!("cp/sv/{j}")).row();
                    for i in 0..SV_N {
                        let (cs, cv) = (i as f32 / (SV_N - 1) as f32, 1.0 - j as f32 / (SV_N - 1) as f32);
                        row = row.child(Node::new(format!("cp/sv/{j}/{i}")).wh(cell, cell).fill(Color::from_hsv(h, cs, cv)).on(format!("cpsv:{i}:{j}")));
                    }
                    grid = grid.child(row);
                }
                let mut hue = Node::new("cp/hue").row().radius(7.0).clip();
                for i in 0..HUE_N {
                    hue = hue.child(Node::new(format!("cp/hue/{i}")).wh(224.0 / HUE_N as f32, 16.0).fill(Color::from_hsv((i as f32 + 0.5) / HUE_N as f32, 1.0, 1.0)).on(format!("cph:{i}")));
                }
                // markers
                let mx = 14.0 + s * 223.0;
                let my = 14.0 + (1.0 - v) * 223.0;
                let sv_wrap = Node::new("cp/svw").w(224.0).h(224.0).child(grid).child(Node::new("cp/svm").wh(14.0, 14.0).radius(7.0).border(2.0, Color([1.0, 1.0, 1.0, 1.0])).abs(Some(mx - 21.0), Some(my - 21.0), None, None));
                let mut presets = Node::new("cp/pre").row().wrap().gap(6.0);
                for (i, tok) in ["accent", "text", "danger", "text-dim"].iter().enumerate() {
                    presets = presets.child(k.swatch(&format!("cp/pre/{i}"), ctx.theme.color(tok), 22.0, Some(format!("cpset:{}", ctx.theme.color(tok).to_hex())), false));
                }
                for (i, hx) in ["#ff6b6b", "#ffb454", "#7ee787", "#5ef2c8", "#6ea8ff", "#b18cff", "#ff8fd8", "#ffffff"].iter().enumerate() {
                    presets = presets.child(k.swatch(&format!("cp/hx/{i}"), Color::parse(hx).unwrap(), 22.0, Some(format!("cpset:{hx}")), false));
                }
                root = root.child(
                    Node::new("s/ov/pop")
                        .col()
                        .abs(Some(x), Some(y), None, None)
                        .w(pw)
                        .gap(10.0)
                        .pad(14.0)
                        .radius(14.0)
                        .fill(Color([0.06, 0.07, 0.11, 0.98]))
                        .border(1.0, k.c("border"))
                        .shadow(20.0, 8.0, Color([0.0, 0.0, 0.0, 0.55]))
                        .enter(170, if below { -6.0 } else { 6.0 }, 0)
                        .hit()
                        .child(sv_wrap)
                        .child(hue)
                        .child(Node::new("cp/cur").row().align(taffy::AlignItems::CENTER).gap(10.0).child(Node::new("cp/cur/sw").wh(28.0, 28.0).radius(14.0).fill(cur).border(1.0, Color([1.0, 1.0, 1.0, 0.4]))).child(k.txt("cp/cur/t".into(), &cur.to_hex(), 13.0, k.c("text")).with_text(|t| t.family = ctx.theme.str("font-mono"))))
                        .child(presets),
                );
            }
        }
        root
    }
}

trait DangerExt {
    fn with_danger(self, k: &Kit) -> Self;
    fn with_danger_fill(self, k: &Kit) -> Self;
}
impl DangerExt for Node {
    fn with_danger(mut self, k: &Kit) -> Self {
        self.hover.fill = Some(k.c("danger").with_alpha(0.28));
        self
    }
    fn with_danger_fill(mut self, k: &Kit) -> Self {
        self.look.fill = k.c("danger");
        self.hover.fill = Some(k.c("danger").mul_alpha(0.86));
        for c in &mut self.children {
            if let Kind::Text(t) = &mut c.kind {
                t.color = Color([1.0, 1.0, 1.0, 1.0]);
            }
        }
        self
    }
}

// ---- actions, editing and input --------------------------------------------------------------

impl UiState {
    fn instance<'a>(ctx: &'a Ctx, id: &str) -> Option<&'a crate::workspace::InstanceCfg> {
        ctx.ws.instances.iter().find(|c| c.id == id)
    }

    /// Record where the open popup's anchor control is, from the last layout.
    pub fn record_anchors(&mut self, frame: &Frame) {
        let key = match &self.open {
            Some(Open::Dropdown(k)) => k.clone(),
            Some(Open::Color(t)) => format!("cp:{t}"),
            None => return,
        };
        let rect = if let Some(t) = key.strip_prefix("cp:") {
            let want = format!("cp:{t}");
            frame.hits.iter().find(|h| h.action.as_deref() == Some(want.as_str())).map(|h| h.rect)
        } else {
            frame.rect_of(&key)
        };
        if let Some([x, y, w, h]) = rect {
            self.anchors.insert(key, (x, y, w, h));
        }
    }

    pub fn focus_input(&mut self, ctx: &Ctx, key: &str, caret: Option<usize>) {
        let text = self.input_text(ctx, key);
        let caret = caret.unwrap_or(text.len()).min(text.len());
        self.focus = Some(Focus { key: key.to_string(), text, caret });
        self.open = None;
        self.caret_on = true;
        self.caret_at = Instant::now();
    }

    pub fn blur(&mut self) {
        self.focus = None;
    }

    fn picker_emit(&mut self) -> Vec<Cmd> {
        let Some(Open::Color(t)) = &self.open else { return vec![] };
        let (h, s, v) = self.hsv;
        Self::color_cmd(t, Color::from_hsv(h, s, v).to_hex()).into_iter().collect()
    }

    fn apply_pick(&mut self, ctx: &Ctx, key: &str, value: &str) -> Vec<Cmd> {
        if let Some(id) = key.strip_prefix("z:") {
            return vec![Cmd::Z(id.into(), value.into())];
        }
        if key == "gpu" {
            return vec![Cmd::Gpu(value.into())];
        }
        if let Some(axis) = key.strip_prefix("th:") {
            let mut sel = ctx.ws.theme.clone();
            match axis {
                "palette" => sel.palette = value.into(),
                "fonts" => sel.fonts = value.into(),
                "glyphs" => sel.glyphs = value.into(),
                "pack" => sel.icon_pack = value.into(),
                _ => return vec![],
            }
            return vec![Cmd::Theme(sel)];
        }
        if let Some((id, name)) = key.strip_prefix("p:").and_then(|r| r.split_once(':')) {
            return vec![Cmd::Param(id.into(), name.into(), Value::Str(value.into()))];
        }
        vec![]
    }

    /// Handle one action string. `hwnd` parents native dialogs.
    pub fn act(&mut self, a: &str, ctx: &Ctx, hwnd: Option<windows::Win32::Foundation::HWND>) -> Vec<Cmd> {
        let (verb, rest) = a.split_once(':').unwrap_or((a, ""));
        if verb != "del" {
            self.confirm_del = None;
        }
        if !matches!(verb, "in" | "sl" | "cpsv" | "cph" | "cpset" | "pick" | "popup-close" | "drag") {
            self.focus = None;
        }
        match verb {
            "nav" => {
                self.page = Page::parse(rest);
                self.open = None;
                vec![]
            }
            "sel" => {
                self.selected = Some(rest.into());
                self.open = None;
                vec![]
            }
            "add" => {
                self.selected = Some(ctx.ws.next_id(rest));
                self.page = Page::Widgets;
                vec![Cmd::Add(rest.into())]
            }
            "del" => {
                if self.confirm_del.as_deref() == Some(rest) {
                    self.confirm_del = None;
                    self.selected = None;
                    vec![Cmd::Remove(rest.into())]
                } else {
                    self.confirm_del = Some(rest.into());
                    vec![]
                }
            }
            "tog" => {
                let Some((id, name)) = rest.split_once('|') else { return vec![] };
                let Some(cfg) = Self::instance(ctx, id) else { return vec![] };
                let Some(p) = self.def_of(ctx, &cfg.widget).and_then(|d| d.params.iter().find(|p| p.name == name)) else { return vec![] };
                let cur = Self::param_value(cfg, p).truthy();
                vec![Cmd::Param(id.into(), name.into(), Value::Bool(!cur))]
            }
            "ct" => Self::instance(ctx, rest).map(|c| vec![Cmd::ClickThrough(rest.into(), !c.click_through)]).unwrap_or_default(),
            "reset" => vec![Cmd::ResetPos(rest.into())],
            "edit" => vec![Cmd::Edit(!ctx.edit)],
            "autostart" => vec![Cmd::Autostart(!ctx.ws.autostart)],
            "dd" => {
                self.open = if matches!(&self.open, Some(Open::Dropdown(k)) if k == rest) { None } else { Some(Open::Dropdown(rest.into())) };
                vec![]
            }
            "pick" => {
                let Some((key, val)) = rest.split_once('|') else { return vec![] };
                self.open = None;
                self.apply_pick(ctx, key, val)
            }
            "popup-close" => {
                self.open = None;
                vec![]
            }
            "cp" => {
                self.hsv = self.color_of_target(ctx, rest).to_hsv().into();
                self.open = Some(Open::Color(rest.into()));
                vec![]
            }
            "cpsv" => {
                let Some((i, j)) = rest.split_once(':').and_then(|(i, j)| Some((i.parse::<usize>().ok()?, j.parse::<usize>().ok()?))) else { return vec![] };
                let n = (SV_N - 1) as f32;
                self.hsv = (self.hsv.0, i as f32 / n, 1.0 - j as f32 / n);
                self.picker_emit()
            }
            "cph" => {
                let Some(i) = rest.parse::<usize>().ok() else { return vec![] };
                self.hsv.0 = (i as f32 + 0.5) / HUE_N as f32;
                self.picker_emit()
            }
            "cpset" => {
                let Some(c) = Color::parse(rest) else { return vec![] };
                self.hsv = c.to_hsv().into();
                self.picker_emit()
            }
            "folder" => {
                let Some((id, name)) = rest.split_once('|') else { return vec![] };
                match dialog::pick_folder(hwnd) {
                    Some(p) => vec![Cmd::Param(id.into(), name.into(), Value::Str(p.to_string_lossy().into_owned()))],
                    None => vec![],
                }
            }
            "clear" => rest.split_once('|').map(|(id, name)| vec![Cmd::Param(id.into(), name.into(), Value::Str(String::new()))]).unwrap_or_default(),
            "scadd" => {
                let Some(p) = dialog::pick_file(hwnd) else { return vec![] };
                let mut items = Self::instance(ctx, rest).map(|c| c.items()).unwrap_or_default();
                let target = p.to_string_lossy().into_owned();
                items.push(Shortcut { name: crate::data::file_stem(&target), target, icon: String::new() });
                vec![Cmd::Items(rest.into(), items)]
            }
            "scbrowse" => {
                let Some((id, idx)) = rest.split_once('|') else { return vec![] };
                let Some(p) = dialog::pick_file(hwnd) else { return vec![] };
                let mut items = Self::instance(ctx, id).map(|c| c.items()).unwrap_or_default();
                if let Some(it) = items.get_mut(idx.parse::<usize>().unwrap_or(usize::MAX)) {
                    it.target = p.to_string_lossy().into_owned();
                    if it.name.is_empty() {
                        it.name = crate::data::file_stem(&it.target);
                    }
                    return vec![Cmd::Items(id.into(), items)];
                }
                vec![]
            }
            "scdel" => {
                let Some((id, idx)) = rest.split_once('|') else { return vec![] };
                let mut items = Self::instance(ctx, id).map(|c| c.items()).unwrap_or_default();
                let i = idx.parse::<usize>().unwrap_or(usize::MAX);
                if i < items.len() {
                    items.remove(i);
                    return vec![Cmd::Items(id.into(), items)];
                }
                vec![]
            }
            "ovreset" => {
                if rest == "*" {
                    ctx.ws.overrides.keys().map(|k| Cmd::Override(k.clone(), None)).collect()
                } else {
                    vec![Cmd::Override(rest.into(), None)]
                }
            }
            "openfolder" => vec![Cmd::OpenFolder],
            "reload" => vec![Cmd::Reload],
            "quit" => vec![Cmd::Quit],
            "close" => vec![Cmd::Close],
            "min" => vec![Cmd::Minimize],
            _ => vec![],
        }
    }

    /// Drag a slider so its wrapper rect maps `x` (window logical px) to a value.
    pub fn slide(&mut self, key: &str, x: f32, frame: &Frame, ctx: &Ctx) -> Vec<Cmd> {
        let Some([rx, _, rw, _]) = frame.rect_of(key) else { return vec![] };
        let target = key.strip_prefix("sl:").unwrap_or(key);
        let Some((min, max, step, cur)) = self.slider_spec(ctx, target) else { return vec![] };
        let v = slider_value(((x - rx) / rw.max(1.0)).clamp(0.0, 1.0), min, max, step);
        if (v - cur).abs() < 1e-9 {
            return vec![];
        }
        Self::slider_cmd(target, v).into_iter().collect()
    }

    /// A key press. `text` is the printable text the key produced, if any.
    pub fn on_key(&mut self, key: &Key, text: Option<&str>, ctx: &Ctx) -> Vec<Cmd> {
        let ctrl = self.mods.control_key();
        let Some(f) = self.focus.as_mut() else {
            if matches!(key, Key::Named(NamedKey::Escape)) {
                self.open = None;
            }
            return vec![];
        };
        let mut changed = true;
        match key {
            Key::Named(NamedKey::Escape | NamedKey::Enter | NamedKey::Tab) => {
                self.focus = None;
                return vec![];
            }
            Key::Named(NamedKey::Backspace) if f.caret > 0 => {
                let p = prev_boundary(&f.text, f.caret);
                f.text.replace_range(p..f.caret, "");
                f.caret = p;
            }
            Key::Named(NamedKey::Delete) if f.caret < f.text.len() => {
                let n = next_boundary(&f.text, f.caret);
                f.text.replace_range(f.caret..n, "");
            }
            Key::Named(NamedKey::ArrowLeft) => {
                f.caret = prev_boundary(&f.text, f.caret);
                changed = false;
            }
            Key::Named(NamedKey::ArrowRight) => {
                f.caret = next_boundary(&f.text, f.caret);
                changed = false;
            }
            Key::Named(NamedKey::Home) => {
                f.caret = 0;
                changed = false;
            }
            Key::Named(NamedKey::End) => {
                f.caret = f.text.len();
                changed = false;
            }
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("v") => {
                if let Some(t) = dialog::clipboard_text() {
                    f.caret = insert_at(&mut f.text, f.caret, &t);
                } else {
                    changed = false;
                }
            }
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("c") => {
                dialog::set_clipboard_text(&f.text);
                changed = false;
            }
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("x") => {
                dialog::set_clipboard_text(&f.text);
                f.text.clear();
                f.caret = 0;
            }
            _ if !ctrl => match text.filter(|t| !t.chars().any(char::is_control)) {
                Some(t) => f.caret = insert_at(&mut f.text, f.caret, t),
                None => changed = false,
            },
            _ => changed = false,
        }
        self.caret_on = true;
        self.caret_at = Instant::now();
        if !changed {
            return vec![];
        }
        let (k, t) = (f.key.clone(), f.text.clone());
        self.commit_text(ctx, &k, &t)
    }

    /// Mouse-wheel scrolling of whichever scroll region is under the pointer.
    pub fn wheel(&mut self, dy: f32, mouse: (f32, f32), frame: &Frame) {
        let region = frame.scrolls.iter().rev().find(|s| frame.rect_of(&s.key).is_some_and(|[x, y, w, h]| mouse.0 >= x && mouse.0 < x + w && mouse.1 >= y && mouse.1 < y + h));
        // a popup list takes the wheel while a popup is open
        let region = if self.open.is_some() { frame.scrolls.iter().find(|s| s.key == "s/ov/list") } else { region };
        if let Some(r) = region {
            let max = (r.content_h - r.view_h).max(0.0);
            let cur = self.scroll_of(&r.key);
            self.scroll.insert(r.key.clone(), (cur - dy).clamp(0.0, max));
        }
    }
}

// ---- the window ----------------------------------------------------------------------------------

pub struct SettingsWin {
    pub window: Arc<Window>,
    pub target: Target,
    ui: UiState,
    anim: Anim,
    hover: Option<String>,
    frame: Option<Frame>,
    mouse: (f32, f32),
    down: bool,
    drag: Option<String>,
    redraw: bool,
    animating: bool,
    last: Instant,
}

impl SettingsWin {
    pub fn open(el: &ActiveEventLoop, gpu: &mut Option<Gpu>, power: Power) -> Result<SettingsWin, String> {
        let pos = el.primary_monitor().or_else(|| el.available_monitors().next()).map(|m| {
            let (p, s, sc) = (m.position(), m.size(), m.scale_factor());
            PhysicalPosition::new(p.x + ((s.width as f64 - WIN.0 as f64 * sc) / 2.0).max(0.0) as i32, p.y + ((s.height as f64 - WIN.1 as f64 * sc) / 2.0).max(0.0) as i32)
        });
        let mut attrs = WindowAttributes::default()
            .with_title("Wayfinder Settings")
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_visible(false)
            .with_inner_size(LogicalSize::new(WIN.0 as f64, WIN.1 as f64))
            .with_no_redirection_bitmap(true);
        if let Some(p) = pos {
            attrs = attrs.with_position(p);
        }
        let window = Arc::new(el.create_window(attrs).map_err(|e| e.to_string())?);
        let target = match gpu.as_mut() {
            Some(g) => g.target_for(&window)?,
            None => {
                let (g, t) = Gpu::new(&window, power)?;
                *gpu = Some(g);
                t
            }
        };
        window.set_visible(true);
        window.focus_window();
        Ok(SettingsWin { window, target, ui: UiState::default(), anim: Anim::default(), hover: None, frame: None, mouse: (-1.0, -1.0), down: false, drag: None, redraw: true, animating: false, last: Instant::now() })
    }

    fn scale(&self) -> f64 {
        self.window.scale_factor()
    }

    fn hwnd(&self) -> Option<windows::Win32::Foundation::HWND> {
        crate::platform::win32::hwnd_of(&self.window)
    }

    fn logical(&self, p: PhysicalPosition<f64>) -> (f32, f32) {
        let s = self.scale();
        ((p.x / s) as f32, (p.y / s) as f32)
    }

    fn dispatch(&mut self, action: &str, ctx: &Ctx, text: &mut TextEngine) -> Vec<Cmd> {
        match action.split_once(':').map_or(action, |(v, _)| v) {
            "drag" => {
                let _ = self.window.drag_window();
                vec![]
            }
            "min" => {
                self.window.set_minimized(true);
                vec![]
            }
            "sl" => {
                self.drag = Some(action.to_string());
                self.slide_now(action, ctx)
            }
            "cpsv" | "cph" => {
                self.drag = Some(action.split(':').next().unwrap_or("").to_string());
                self.ui.act(action, ctx, self.hwnd())
            }
            "in" => {
                let key = action.strip_prefix("in:").unwrap_or("");
                let caret = self.frame.as_ref().and_then(|f| f.rect_of(&format!("{key}/t"))).map(|r| text.byte_at(&format!("{key}/t"), self.mouse.0 - r[0]));
                let text_now = self.ui.input_text(ctx, key);
                self.ui.focus_input(ctx, key, caret.map(|c| c.min(text_now.len())));
                vec![]
            }
            _ => {
                let hwnd = self.hwnd();
                self.ui.act(action, ctx, hwnd)
            }
        }
    }

    fn slide_now(&mut self, action: &str, ctx: &Ctx) -> Vec<Cmd> {
        let Some(frame) = &self.frame else { return vec![] };
        self.ui.slide(action, self.mouse.0, frame, ctx)
    }

    pub fn event(&mut self, ev: &WindowEvent, ctx: &Ctx, text: &mut TextEngine) -> Vec<Cmd> {
        let mut cmds = Vec::new();
        match ev {
            WindowEvent::CloseRequested => cmds.push(Cmd::Close),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => self.redraw = true,
            WindowEvent::ModifiersChanged(m) => self.ui.mods = m.state(),
            WindowEvent::CursorLeft { .. } => {
                if self.hover.take().is_some() {
                    self.redraw = true;
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse = self.logical(*position);
                let hit = self.frame.as_ref().and_then(|f| f.hit_at(self.mouse.0, self.mouse.1)).map(|h| (h.key.clone(), h.action.clone()));
                let (key, action) = match hit {
                    Some((k, a)) => (Some(k), a),
                    None => (None, None),
                };
                if self.hover != key {
                    self.hover = key;
                    self.redraw = true;
                    self.window.set_cursor(match action.as_deref() {
                        Some(a) if a.starts_with("in:") => CursorIcon::Text,
                        Some("drag") | None => CursorIcon::Default,
                        Some(_) => CursorIcon::Pointer,
                    });
                }
                if self.down {
                    match self.drag.clone().as_deref() {
                        Some(a) if a.starts_with("sl:") => cmds.extend(self.slide_now(a, ctx)),
                        Some("cpsv" | "cph") => {
                            if let Some(a) = action.filter(|a| a.starts_with("cpsv:") || a.starts_with("cph:")) {
                                cmds.extend(self.ui.act(&a, ctx, None));
                            }
                        }
                        _ => {}
                    }
                    self.redraw = true;
                }
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                self.down = true;
                let action = self.frame.as_ref().and_then(|f| f.hit_at(self.mouse.0, self.mouse.1)).and_then(|h| h.action.clone());
                match action {
                    Some(a) => cmds.extend(self.dispatch(&a, ctx, text)),
                    None => self.ui.blur(),
                }
                self.redraw = true;
            }
            WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left, .. } => {
                self.down = false;
                self.drag = None;
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * 56.0,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / self.scale() as f32,
                };
                if let Some(f) = &self.frame {
                    self.ui.wheel(dy, self.mouse, f);
                    self.redraw = true;
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                cmds.extend(self.ui.on_key(&event.logical_key, event.text.as_deref(), ctx));
                self.redraw = true;
            }
            _ => {}
        }
        if !cmds.is_empty() {
            self.redraw = true;
        }
        cmds
    }

    pub fn render(&mut self, gpu: &mut Gpu, text: &mut TextEngine, icons: &mut IconService, ctx: &Ctx) {
        let now = Instant::now();
        if self.ui.wants_caret() && now.duration_since(self.ui.caret_at) >= Duration::from_millis(530) {
            self.ui.caret_on = !self.ui.caret_on;
            self.ui.caret_at = now;
        }
        let s = self.scale() as f32;
        let phys = self.window.inner_size();
        gpu.fit(&mut self.target, phys.width, phys.height);
        let size = (phys.width as f32 / s, phys.height as f32 / s);
        let (root, images) = self.ui.build(ctx, size);
        for id in &images {
            icons.ensure(gpu, id);
        }
        let mut env = Env { text, anim: &mut self.anim, hover: self.hover.as_deref(), now, scale: s };
        let frame = ui::layout(&root, size, &mut env);
        match gpu.render(&mut self.target, &frame.list, text) {
            Ok(()) => {}
            Err(RenderError::Skip(e)) | Err(RenderError::Lost(e)) => eprintln!("wayfinder: settings render: {e}"),
        }
        self.animating = frame.animating;
        self.ui.record_anchors(&frame);
        // an open popup needs its anchor rect to position itself; one more frame settles it
        self.redraw = self.ui.has_popup() && !self.ui.anchors.contains_key(&self.ui.anchor_key());
        self.frame = Some(frame);
        self.last = now;
    }

    /// When this window next needs a frame (None = idle).
    pub fn next_frame(&self, now: Instant) -> Option<Instant> {
        if self.redraw {
            return Some(now);
        }
        if self.animating {
            return Some(self.last + Duration::from_millis(16));
        }
        if self.ui.wants_caret() {
            return Some(self.ui.caret_at + Duration::from_millis(530));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Library, Selection};
    use crate::workspace::{InstanceCfg, MonitorRef};
    use std::path::Path;

    struct World {
        ws: Workspace,
        reg: Registry,
        lib: Library,
        theme: Theme,
    }

    fn world() -> World {
        let lib = Library::load(Path::new("no-such-dir"));
        let theme = Theme::compose(&lib, &Selection::default(), &Default::default());
        let mut ws = Workspace::default();
        ws.instances.push(InstanceCfg { id: "clock-1".into(), widget: "clock".into(), monitor: MonitorRef { name: "\\\\.\\DISPLAY1".into(), width: 1920, height: 1080 }, ..Default::default() });
        let mut folder = InstanceCfg { id: "icon_folder-1".into(), widget: "icon_folder".into(), ..Default::default() };
        folder.set_items(&[Shortcut { name: "A".into(), target: "a.exe".into(), icon: String::new() }, Shortcut { name: "B".into(), target: "b.exe".into(), icon: String::new() }]);
        ws.instances.push(folder);
        World { ws, reg: Registry::load(Path::new("no-such-dir")), lib, theme }
    }

    fn ctx(w: &World) -> Ctx<'_> {
        Ctx { ws: &w.ws, reg: &w.reg, lib: &w.lib, theme: &w.theme, log: &[], gpu_info: "test gpu", fonts: &[], edit: false, parked: &[] }
    }

    #[test]
    fn slider_snaps_to_step_and_clamps() {
        assert_eq!(slider_value(0.5, 0.0, 100.0, 10.0), 50.0);
        assert_eq!(slider_value(0.53, 0.0, 100.0, 10.0), 50.0);
        assert_eq!(slider_value(2.0, 16.0, 96.0, 2.0), 96.0);
        assert_eq!(slider_value(-1.0, 16.0, 96.0, 2.0), 16.0);
        assert_eq!(slider_value(0.5, 0.0, 1.0, 0.0), 0.5);
    }

    #[test]
    fn text_editing_respects_utf8_boundaries() {
        let mut s = String::from("zażółć");
        let end = s.len();
        let p = prev_boundary(&s, end);
        assert_eq!(&s[p..], "ć");
        assert_eq!(next_boundary(&s, p), end);
        let c = insert_at(&mut s, p, "X");
        assert_eq!(s, "zażółXć");
        assert_eq!(c, p + 1);
    }

    #[test]
    fn actions_become_the_expected_commands() {
        let w = world();
        let c = ctx(&w);
        let mut ui = UiState::default();
        // a bool param toggles from its default (ticks defaults to true)
        assert_eq!(ui.act("tog:clock-1|ticks", &c, None), vec![Cmd::Param("clock-1".into(), "ticks".into(), Value::Bool(false))]);
        assert_eq!(ui.act("z:x", &c, None), vec![]);
        assert_eq!(ui.act("pick:z:clock-1|topmost", &c, None), vec![Cmd::Z("clock-1".into(), "topmost".into())]);
        assert_eq!(ui.act("pick:gpu|high", &c, None), vec![Cmd::Gpu("high".into())]);
        let t = ui.act("pick:th:palette|Daylight", &c, None);
        assert!(matches!(&t[0], Cmd::Theme(s) if s.palette == "Daylight" && s.fonts == "System"));
        assert_eq!(ui.act("edit:toggle", &c, None), vec![Cmd::Edit(true)]);
        assert_eq!(ui.act("nav:appearance", &c, None), vec![]);
        assert_eq!(ui.page, Page::Appearance);
    }

    #[test]
    fn removing_a_widget_needs_a_second_click() {
        let w = world();
        let c = ctx(&w);
        let mut ui = UiState::default();
        assert_eq!(ui.act("del:clock-1", &c, None), vec![], "first click only arms the confirmation");
        assert_eq!(ui.act("sel:icon_folder-1", &c, None), vec![], "clicking elsewhere disarms it");
        assert_eq!(ui.act("del:clock-1", &c, None), vec![]);
        assert_eq!(ui.act("del:clock-1", &c, None), vec![Cmd::Remove("clock-1".into())]);
    }

    #[test]
    fn shortcut_fields_edit_the_right_row() {
        let w = world();
        let c = ctx(&w);
        let mut ui = UiState::default();
        ui.focus_input(&c, "sn:icon_folder-1:1", None);
        assert_eq!(ui.input_text(&c, "sn:icon_folder-1:1"), "B");
        ui.mods = ModifiersState::empty();
        let cmds = ui.on_key(&Key::Character("z".into()), Some("z"), &c);
        let Cmd::Items(id, items) = &cmds[0] else { panic!("{cmds:?}") };
        assert_eq!((id.as_str(), items[0].name.as_str(), items[1].name.as_str()), ("icon_folder-1", "A", "Bz"), "row 1 changed, row 0 untouched");
        // deleting a row
        assert!(matches!(&ui.act("scdel:icon_folder-1|0", &c, None)[0], Cmd::Items(_, i) if i.len() == 1 && i[0].name == "B"));
    }

    #[test]
    fn hex_input_only_commits_valid_colours() {
        let w = world();
        let c = ctx(&w);
        let mut ui = UiState::default();
        ui.focus_input(&c, "hx:ov:accent", Some(0));
        // typing into a field that already holds a colour stays invalid until it parses
        ui.focus.as_mut().unwrap().text = String::new();
        assert_eq!(ui.on_key(&Key::Character("z".into()), Some("z"), &c), vec![], "'z' is not a hex digit");
        ui.focus.as_mut().unwrap().text = "ff8800".into();
        ui.focus.as_mut().unwrap().caret = 6;
        let cmds = ui.on_key(&Key::Named(NamedKey::Backspace), None, &c);
        assert_eq!(cmds, vec![], "ff880 is five digits: invalid");
        let cmds = ui.on_key(&Key::Character("0".into()), Some("0"), &c);
        assert_eq!(cmds, vec![Cmd::Override("accent".into(), Some("#ff8800".into()))]);
    }

    #[test]
    fn colour_picker_emits_a_command_per_pick() {
        let w = world();
        let c = ctx(&w);
        let mut ui = UiState::default();
        ui.act("cp:p:clock-1:accent", &c, None);
        let cmds = ui.act(&format!("cpsv:{}:0", SV_N - 1), &c, None); // full saturation, full value
        let Cmd::Param(id, name, Value::Str(hex)) = &cmds[0] else { panic!("{cmds:?}") };
        assert_eq!((id.as_str(), name.as_str()), ("clock-1", "accent"));
        assert!(Color::parse(hex).is_some());
        assert_eq!(ui.act("cpset:#00ff00", &c, None), vec![Cmd::Param("clock-1".into(), "accent".into(), Value::Str("#00ff00".into()))]);
    }

    #[test]
    fn every_page_builds_with_unique_keys() {
        let w = world();
        let c = ctx(&w);
        for page in Page::ALL {
            let mut ui = UiState::default();
            ui.page = page;
            ui.selected = Some("icon_folder-1".into());
            let (root, _) = ui.build(&c, WIN);
            let mut seen = std::collections::HashSet::new();
            fn walk(n: &Node, seen: &mut std::collections::HashSet<String>, page: Page) {
                assert!(seen.insert(n.key.clone()), "duplicate node key `{}` on {page:?}: hover, animation and text state would collide", n.key);
                n.children.iter().for_each(|c| walk(c, seen, page));
            }
            walk(&root, &mut seen, page);
        }
    }

    #[test]
    fn slider_target_specs_cover_params_overrides_and_grid() {
        let w = world();
        let c = ctx(&w);
        let ui = UiState::default();
        assert_eq!(ui.slider_spec(&c, "grid"), Some((0.0, 32.0, 4.0, 8.0)));
        let (min, max, _, cur) = ui.slider_spec(&c, "p:icon_folder-1:icon_size").unwrap();
        assert_eq!((min, max, cur), (24.0, 72.0, 40.0));
        assert_eq!(UiState::slider_cmd("p:icon_folder-1:icon_size", 48.0), Some(Cmd::Param("icon_folder-1".into(), "icon_size".into(), Value::Num(48.0))));
        assert_eq!(UiState::slider_cmd("ov:radius-lg", 30.0), Some(Cmd::Override("radius-lg".into(), Some("30".into()))));
    }
}
