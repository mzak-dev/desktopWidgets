//! The settings window, on the widgets' renderer (decisions 6, 9). Nodes carry
//! action strings (`tog:clock-1|ticks`) that `UiState::act` turns into `Cmd`s
//! for the app, with no window or GPU, so it is unit-testable.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::{CursorIcon, Window, WindowAttributes};

use crate::anim::{Anim, Ease};
use crate::color::{Color, TRANSPARENT};
use crate::data::Shortcut;
use crate::dialog;
use crate::draw::{Inst, KIND_ARC, KIND_CAPSULE, KIND_RECT};
use crate::elements::{Shape, ShapeCx, rgba_with_opacity};
use crate::gfx::{Gpu, Power, RenderError, Target};
use crate::icons::IconService;
use crate::modules::Arrangement;
use crate::plugins::PluginRow;
use crate::text::TextEngine;
use crate::theme::{Axis, Library, Selection, Theme, style_schema};
use crate::ui::{self, Env, Frame, Kind, Node};
use crate::value::Value;
use crate::widgets::{ParamDef, ParamType, Registry, WidgetMeta};
use crate::platform::win32::FileOwner;
use crate::workspace::{Flag, Workspace};

pub struct Ctx<'a> {
    pub ws: &'a Workspace,
    pub reg: &'a Registry,
    pub lib: &'a Library,
    pub theme: &'a Theme,
    pub log: &'a [LogLine],
    pub gpu_info: &'a str,
    pub fonts: &'a [String],
    pub edit: bool,
    /// Instances without a window, and why.
    pub hidden: &'a [(String, Hidden)],
    pub plugins: &'a [PluginRow],
    /// The outcome of the last install, for the Plugins page.
    pub plugin_note: &'a str,
    /// Every data source the app has now, for widgets' `needs`.
    pub sources: &'a [String],
    /// Which exe double-clicking a `.wfplugin` runs.
    pub plugin_files: &'a FileOwner,
    /// Live data for the widget preview.
    pub data: &'a crate::data::DataSources,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Hidden {
    /// Its monitor is missing.
    Parked,
    /// Only this switched-off Plugin provides its Widget.
    PluginOff(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Warning,
    Error,
}

impl Level {
    const ALL: [Level; 3] = [Level::Info, Level::Warning, Level::Error];

    fn id(self) -> &'static str {
        match self {
            Level::Info => "info",
            Level::Warning => "warning",
            Level::Error => "error",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Level::Info => "Info",
            Level::Warning => "Warning",
            Level::Error => "Error",
        }
    }
    /// The Log page's filter for it.
    fn plural(self) -> &'static str {
        match self {
            Level::Info => "Info",
            Level::Warning => "Warnings",
            Level::Error => "Errors",
        }
    }
}

/// One line of the Log page.
#[derive(Clone, Debug, PartialEq)]
pub struct LogLine {
    /// Local wall-clock time, `01:29:58`.
    pub time: String,
    pub level: Level,
    /// What wrote it: `core`, `gpu`, `plugins`...
    pub source: String,
    pub text: String,
}

impl LogLine {
    pub fn new(t: crate::data::Tm, line: &str) -> LogLine {
        let (level, source, text) = classify(line);
        LogLine { time: format!("{:02}:{:02}:{:02}", t.hour, t.minute, t.second), level, source: source.to_string(), text: text.to_string() }
    }
}

/// Sorts a message from `App::log` into the Log page's level and source; returns
/// (level, source, the text to show). A `gpu` warning gets a link to the adapter.
///
/// Messages are plain sentences, most with a `topic: ` prefix:
/// `gpu: AMD Radeon(TM) Graphics / Dx12 / IntegratedGpu / …`,
/// `gpu: using the software adapter (…); the integrated GPU is recommended`,
/// `monitor \\.\DISPLAY1: 1920x1080 @ 1.00x, …`, `ready: 4 instance(s), theme …`,
/// `could not install x.wfplugin: …`, `plugin agents: agents: Error: …`,
/// `weather: <a line from that Code Source>`, `refused to open …`, `GPU lost (…): rebuilding`.
fn classify(line: &str) -> (Level, &str, &str) {
    // TODO(you): choose the level and source for a message; everything is Info from `core` until then.
    (Level::Info, "core", line)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    Add(String),
    Remove(String),
    Param(String, String, Value),
    Items(String, Vec<Shortcut>),
    /// An Instance's arrangement of one tier's Modules; `None` goes back to the Widget's default.
    Layout(String, String, Option<std::collections::BTreeMap<String, Vec<String>>>),
    Z(String, String),
    ClickThrough(String, bool),
    SizeLimit(String, bool),
    ResetPos(String),
    Theme(Selection),
    /// Set (`Some`) or reset (`None`) a Style token.
    Style(Scope, String, Option<Value>),
    /// An Instance's own palette/fonts/glyphs/pack (`palette|fonts|glyphs|pack`); `None` = global.
    ThemePick(String, String, Option<String>),
    Gpu(String),
    Autostart(bool),
    Grid(f32),
    Flag(Flag, bool),
    Edit(bool),
    Reload,
    OpenFolder,
    InstallPlugin(PathBuf),
    PluginEnabled(String, bool),
    RemovePlugin(String),
    OpenPluginsFolder,
    /// A folder or file under the data folder (`iconpacks`, `wayfinder.log`).
    OpenData(&'static str),
    /// Make double-clicking a `.wfplugin` run this exe.
    ClaimPluginFiles,
    /// The first-run setup is finished or skipped.
    Onboarded,
    Quit,
    /// Save, start a fresh copy and quit this one.
    Restart,
    Close,
    Minimize,
}

/// Where a Style change lands: the Workspace or one Instance (`*` or its id in action keys).
#[derive(Debug, Clone, PartialEq)]
pub enum Scope {
    Global,
    Instance(String),
}

impl Scope {
    pub fn parse(s: &str) -> Scope {
        if s == "*" { Scope::Global } else { Scope::Instance(s.into()) }
    }

    pub fn key(&self) -> &str {
        match self {
            Scope::Global => "*",
            Scope::Instance(id) => id,
        }
    }
}

/// (axis in `tp:` keys, label) of the axes an Instance may pick for itself.
const THEME_AXES: [(&str, &str); 4] = [("palette", "Palette"), ("fonts", "Font set"), ("glyphs", "Glyph set"), ("pack", "Icon pack")];

fn axis_of<'a>(sel: &'a Selection, axis: &str) -> &'a str {
    match axis {
        "palette" => &sel.palette,
        "fonts" => &sel.fonts,
        "glyphs" => &sel.glyphs,
        _ => &sel.icon_pack,
    }
}

fn picked(pick: &crate::theme::ThemePick, axis: &str) -> Option<String> {
    match axis {
        "palette" => pick.palette.clone(),
        "fonts" => pick.fonts.clone(),
        "glyphs" => pick.glyphs.clone(),
        _ => pick.icon_pack.clone(),
    }
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Widgets,
    Appearance,
    Plugins,
    General,
    Log,
}

impl Page {
    const ALL: [Page; 5] = [Page::Widgets, Page::Appearance, Page::Plugins, Page::General, Page::Log];

    fn id(self) -> &'static str {
        match self {
            Page::Widgets => "widgets",
            Page::Appearance => "appearance",
            Page::Plugins => "plugins",
            Page::General => "general",
            Page::Log => "log",
        }
    }
    fn title(self) -> &'static str {
        match self {
            Page::Widgets => "Widgets",
            Page::Appearance => "Appearance",
            Page::Plugins => "Plugins",
            Page::General => "General",
            Page::Log => "Log",
        }
    }
    /// Its heading, where the tab's name is too short.
    fn heading(self) -> &'static str {
        match self {
            Page::Log => "Activity log",
            p => p.title(),
        }
    }
    fn subtitle(self) -> &'static str {
        match self {
            Page::Widgets => "Pick a widget to configure it. Changes show on your desktop right away.",
            Page::Appearance => "Colours, fonts and icons — applied to every widget at once.",
            Page::Plugins => "Widgets, palettes, fonts and icons made by others.",
            Page::General => "Graphics, startup and behaviour.",
            Page::Log => "What Wayfinder has been doing, and anything that went wrong.",
        }
    }
    fn glyph(self) -> &'static str {
        match self {
            Page::Widgets => "widgets",
            Page::Appearance => "palette",
            Page::Plugins => "plugin",
            Page::General => "sliders",
            Page::Log => "list",
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

/// The Edit layout hotkey, as its keys read (registered in `app::tray`).
pub const EDIT_KEYS: &[&str] = &["Ctrl", "Shift", "E"];

const WIN: (f32, f32) = (1180.0, 780.0);
const MIN_WIN: (f32, f32) = (860.0, 560.0);
const HEADER_H: f32 = 62.0;
/// Page margin either side, and the Widgets page's list of Instances.
const PAGE_PAD: f32 = 48.0;
const SIDEBAR_W: f32 = 232.0;
const CONTROL_W: f32 = 250.0;
/// Amber, for what works but not as it should; no palette has a token for it.
const WARN: Color = Color([0.95, 0.77, 0.38, 1.0]);
/// Where Add a widget lists a category; unknown ones follow in name order.
const CATEGORIES: [&str; 4] = ["Time", "System", "Media", "Launchers"];
const SV_N: usize = 14;
const HUE_N: usize = 28;

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

#[derive(Clone, Copy, PartialEq)]
enum Btn {
    Primary,
    Outline,
    Danger,
    DangerFill,
    Warn,
}

struct Kit<'a> {
    t: &'a Theme,
    /// Light text on a dark palette.
    dark: bool,
}

impl Kit<'_> {
    fn new(t: &Theme) -> Kit<'_> {
        let [r, g, b, _] = t.color("text").0;
        Kit { t, dark: r + g + b > 1.5 }
    }

    fn c(&self, n: &str) -> Color {
        self.t.color(n)
    }

    /// The text colour, faint: panels, hairlines and hovers that read on any palette.
    fn ink(&self, a: f32) -> Color {
        self.c("text").with_alpha(a)
    }

    /// Amber that reads on this palette.
    fn warn(&self) -> Color {
        if self.dark { WARN } else { Color([0.62, 0.42, 0.02, 1.0]) }
    }

    fn line(&self) -> Color {
        self.ink(0.09)
    }

    fn panel(&self) -> Color {
        self.ink(0.03)
    }

    /// `ink(a)` over the window, opaque: for fills that overlap themselves (`half_round`).
    fn solid(&self, a: f32) -> Color {
        self.bg().lerp(self.c("text").with_alpha(1.0), a)
    }

    /// The window: the palette's surface, deepened on a dark palette.
    fn bg(&self) -> Color {
        let s = self.c("surface").with_alpha(1.0);
        if self.dark { s.lerp(Color([0.0, 0.0, 0.0, 1.0]), 0.45) } else { s }
    }

    fn txt(&self, key: String, s: &str, size: f32, col: Color) -> Node {
        let fam = self.t.str("font-body");
        Node::text(key, s, size, col).with_text(|t| t.family = fam)
    }

    fn bold(&self, key: String, s: &str, size: f32, col: Color) -> Node {
        self.txt(key, s, size, col).with_text(|t| t.weight = 600)
    }

    fn mono(&self, key: String, s: &str, size: f32, col: Color) -> Node {
        let fam = self.t.str("font-mono");
        Node::text(key, s, size, col).with_text(|t| t.family = fam)
    }

    fn glyph(&self, key: String, name: &str, size: f32, col: Color) -> Node {
        let (fam, ch) = (self.t.str("font-glyph"), self.t.str(&format!("glyph-{name}")));
        Node::text(key, ch, size, col).with_text(|t| t.family = fam)
    }

    /// A Widget's own glyph, or the generic one when the glyph set has no such name.
    fn widget_glyph(&self, meta: Option<&WidgetMeta>) -> String {
        meta.map(|m| m.icon.clone()).filter(|i| !i.is_empty() && self.t.get(&format!("glyph-{i}")).is_some()).unwrap_or_else(|| "widgets".into())
    }

    fn heading(&self, key: String, title: &str) -> Node {
        self.bold(key, title, 15.0, self.c("text"))
    }

    /// The big title and line under it that open a page, with its buttons on the right.
    fn page_head(&self, key: &str, title: &str, sub: &str, actions: Option<Node>) -> Node {
        let left = Node::new(format!("{key}/l"))
            .col()
            .grow(1.0)
            .min_w(0.0)
            .gap(5.0)
            .child(self.bold(format!("{key}/t"), title, 26.0, self.c("text")).with_text(|t| t.weight = 700))
            .child(self.txt(format!("{key}/s"), sub, 13.0, self.c("text-dim")).wrap_text());
        let mut n = Node::new(key).row().align(taffy::AlignItems::FLEX_END).gap(16.0).child(left);
        if let Some(a) = actions {
            n = n.child(a);
        }
        n
    }

    fn btn(&self, key: &str, glyph: Option<&str>, label: &str, action: String, kind: Btn) -> Node {
        let (accent, danger) = (self.c("accent"), self.c("danger"));
        let (fill, hover, border, col) = match kind {
            Btn::Primary => (accent, accent.mul_alpha(0.86), TRANSPARENT, self.c("accent-text")),
            Btn::Outline => (self.ink(0.02), self.ink(0.07), self.line(), self.c("text")),
            Btn::Danger => (TRANSPARENT, danger.with_alpha(0.12), danger.with_alpha(0.45), danger),
            Btn::DangerFill => (danger, danger.mul_alpha(0.86), TRANSPARENT, Color([1.0; 4])),
            Btn::Warn => (WARN, WARN.mul_alpha(0.86), TRANSPARENT, Color([0.14, 0.1, 0.02, 1.0])),
        };
        let mut n = Node::new(key).row().h(34.0).no_shrink().pad_xy(14.0, 0.0).gap(8.0).center().radius(9.0).fill(fill).hover_fill(hover).border(1.0, border).ease(120).on(action);
        if let Some(g) = glyph {
            n = n.child(self.glyph(format!("{key}/g"), g, 13.0, col));
        }
        n.child(self.bold(format!("{key}/t"), label, 13.0, col))
    }

    fn icon_btn(&self, key: &str, glyph: &str, action: String, col: Color, hover: Color) -> Node {
        Node::new(key).wh(34.0, 34.0).no_shrink().center().radius(9.0).hover_fill(hover).ease(120).on(action).child(self.glyph(format!("{key}/g"), glyph, 15.0, col))
    }

    /// Text that works like a link.
    fn link(&self, key: &str, label: &str, action: String) -> Node {
        let mut n = self.bold(key.into(), label, 12.5, self.c("accent")).ease(120).on(action);
        n.hover.text_color = Some(self.c("accent").mul_alpha(0.7));
        n
    }

    fn toggle(&self, key: &str, on: bool, action: String) -> Node {
        let knob = Node::new(format!("{key}/k"))
            .wh(18.0, 18.0)
            .radius(9.0)
            .fill(if on { self.c("accent-text") } else { self.c("text-dim") })
            .abs(Some(3.0), Some(3.0), None, None)
            .offset(if on { 18.0 } else { 0.0 }, 0.0)
            .transition(220, Ease::Back)
            .shadow(3.0, 1.0, Color([0.0, 0.0, 0.0, 0.25]));
        Node::new(key).wh(42.0, 24.0).no_shrink().radius(12.0).fill(if on { self.c("accent") } else { self.ink(0.12) }).ease(180).on(action).child(knob)
    }

    fn slider(&self, key: &str, frac: f32, w: f32, action: String) -> Node {
        let f = frac.clamp(0.0, 1.0);
        let fill = Node::new(format!("{key}/f")).abs(Some(0.0), Some(0.0), None, Some(0.0)).w(w * f).radius(3.0).fill(self.c("accent"));
        let thumb = Node::new(format!("{key}/th"))
            .wh(16.0, 16.0)
            .radius(8.0)
            .fill(self.c("accent"))
            .border(3.0, self.bg())
            .abs(Some(w * f - 8.0), Some(-5.5), None, None)
            .shadow(4.0, 1.0, Color([0.0, 0.0, 0.0, 0.35]));
        let rail = Node::new(format!("{key}/r")).w(w).h(5.0).radius(3.0).fill(self.ink(0.12)).child(fill).child(thumb);
        // the taller wrapper is the hit target and the rect the drag maps onto
        Node::new(key).w(w).h(24.0).no_shrink().align(taffy::AlignItems::CENTER).on(action).child(rail)
    }

    fn input(&self, key: &str, text: &str, placeholder: &str, focus: Option<(usize, bool)>, w: f32, mono: bool) -> Node {
        let focused = focus.is_some();
        let fam = if mono { self.t.str("font-mono") } else { self.t.str("font-body") };
        let shown = if text.is_empty() && !focused { placeholder } else { text };
        let col = if text.is_empty() { self.c("text-dim").mul_alpha(0.8) } else { self.c("text") };
        let caret = focus.and_then(|(c, on)| on.then_some(c));
        let t = Node::text(format!("{key}/t"), shown, 13.0, col).with_text(|s| {
            s.family = fam;
            s.caret = caret;
            s.wrap = false;
        });
        Node::new(key)
            .w(w)
            .h(36.0)
            .no_shrink()
            .pad_xy(12.0, 0.0)
            .align(taffy::AlignItems::CENTER)
            .radius(9.0)
            .fill(self.ink(0.03))
            .border(if focused { 1.5 } else { 1.0 }, if focused { self.c("accent") } else { self.line() })
            .ease(120)
            .clip()
            .on(format!("in:{key}"))
            .child(t)
    }

    fn dropdown(&self, key: &str, label: &str, w: f32, open: bool) -> Node {
        Node::new(key)
            .w(w)
            .h(36.0)
            .no_shrink()
            .row()
            .align(taffy::AlignItems::CENTER)
            .pad_xy(12.0, 0.0)
            .gap(6.0)
            .radius(9.0)
            .fill(self.ink(0.03))
            .hover_fill(self.ink(0.07))
            .border(if open { 1.5 } else { 1.0 }, if open { self.c("accent") } else { self.line() })
            .ease(120)
            .on(format!("dd:{key}"))
            .child(self.txt(format!("{key}/t"), label, 13.0, self.c("text")).grow_text())
            .child(self.glyph(format!("{key}/g"), "chevron-down", 11.0, self.c("text-dim")))
    }

    fn swatch(&self, key: &str, col: Color, size: f32, action: Option<String>, selected: bool) -> Node {
        let mut n = Node::new(key).wh(size, size).no_shrink().radius(size / 2.0).fill(col).border(if selected { 2.5 } else { 1.0 }, if selected { self.c("text") } else { self.line() }).ease(120);
        if let Some(a) = action {
            n = n.on(a).hover_fill(col.mul_alpha(0.8));
        }
        n
    }

    /// A key of a shortcut, as on the keyboard.
    fn kbd(&self, key: String, label: &str) -> Node {
        Node::new(key.clone()).h(22.0).no_shrink().pad_xy(7.0, 0.0).center().radius(5.0).fill(self.ink(0.05)).border(1.0, self.line()).child(self.mono(format!("{key}/t"), label, 11.5, self.c("text-dim")))
    }

    fn keys(&self, key: &str, keys: &[&str]) -> Node {
        Node::new(key).row().gap(4.0).align(taffy::AlignItems::CENTER).kids(keys.iter().enumerate().map(|(i, l)| self.kbd(format!("{key}/{i}"), l)))
    }

    fn badge(&self, key: String, label: &str, col: Color) -> Node {
        Node::new(key.clone()).h(20.0).no_shrink().pad_xy(7.0, 0.0).center().radius(5.0).fill(col.with_alpha(0.14)).child(self.bold(format!("{key}/t"), label, 11.0, col))
    }

    /// A small outlined label: `Extra`, a version.
    fn tag(&self, key: String, label: &str, mono: bool) -> Node {
        let t = if mono { self.mono(format!("{key}/t"), label, 11.5, self.c("text-dim")) } else { self.txt(format!("{key}/t"), label, 11.0, self.c("text-dim")) };
        Node::new(key).h(20.0).no_shrink().pad_xy(6.0, 0.0).center().radius(5.0).border(1.0, self.line()).child(t)
    }

    /// A glyph on a rounded square.
    fn tile(&self, key: String, glyph: &str, size: f32, col: Color) -> Node {
        Node::new(key.clone()).wh(size, size).no_shrink().center().radius(size * 0.26).fill(self.ink(0.05)).border(1.0, self.line()).child(self.glyph(format!("{key}/g"), glyph, (size * 0.4).round(), col))
    }

    /// The Wayfinder mark: an arrow on an accent square.
    fn logo(&self, key: &str, size: f32) -> Node {
        let arrow = shape_node(format!("{key}/a"), Arrow { color: self.c("accent-text"), width: size * 0.12 }).abs_fill();
        Node::new(key).wh(size, size).no_shrink().radius(size * 0.28).fill(self.c("accent")).child(arrow)
    }

    /// A setting in a group card: what it is on the left, its control on the right.
    fn row(&self, key: &str, label: &str, help: &str, control: Node) -> Node {
        self.row_with(key, self.bold(format!("{key}/lt"), label, 13.5, self.c("text")).wrap_text(), help, None, control)
    }

    fn row_with(&self, key: &str, title: Node, help: &str, extra: Option<Node>, control: Node) -> Node {
        let mut left = Node::new(format!("{key}/l")).col().grow(1.0).min_w(0.0).gap(3.0).child(title);
        if !help.is_empty() {
            left = left.child(self.txt(format!("{key}/lh"), help, 12.0, self.c("text-dim")).wrap_text());
        }
        if let Some(e) = extra {
            left = left.child(e);
        }
        let ctl = Node::new(format!("{key}/ctl")).row().no_shrink().align(taffy::AlignItems::CENTER).gap(10.0).child(control);
        Node::new(key).row().gap(20.0).align(taffy::AlignItems::CENTER).pad_xy(20.0, 13.0).child(left).child(ctl)
    }

    /// A heading over a card of rows, with hairlines between them.
    fn group(&self, key: &str, title: &str, rows: Vec<Node>) -> Node {
        let mut card = Node::new(format!("{key}/card")).col().radius(12.0).fill(self.panel()).border(1.0, self.line()).clip();
        for (i, r) in rows.into_iter().enumerate() {
            if i > 0 {
                card = card.child(Node::new(format!("{key}/hr{i}")).h(1.0).no_shrink().fill(self.line()));
            }
            card = card.child(r);
        }
        let mut g = Node::new(key).col().gap(10.0);
        if !title.is_empty() {
            g = g.child(self.heading(format!("{key}/t"), title));
        }
        g.child(card)
    }
}

fn flag_row(k: &Kit, ctx: &Ctx, key: &str, f: Flag) -> Node {
    k.row(key, f.label(), f.help(), k.toggle(&format!("tg:{}", f.id()), ctx.ws.flag(f), format!("flag:{}", f.id())))
}

/// "Corner roundness (px)" is "Corner roundness", shown with its value as "15 px".
fn split_unit(label: &str) -> (&str, &str) {
    match label.strip_suffix(')').and_then(|l| l.rsplit_once(" (")) {
        Some((l, u)) => (l, u),
        None => (label, ""),
    }
}

fn with_unit(v: f64, unit: &str) -> String {
    match unit {
        "" => fmt_num(v),
        "%" => format!("{}%", fmt_num(v)),
        u => format!("{} {u}", fmt_num(v)),
    }
}

fn shape_node(key: String, s: impl Shape + 'static) -> Node {
    let mut n = Node::new(key);
    n.kind = Kind::shape(s);
    n
}

/// A dot grid filling its parent, behind the parent's other children.
fn dots(key: String, col: Color) -> Node {
    shape_node(key, Dots { gap: 18.0, color: col }).abs_fill()
}

/// A dashed outline filling its parent.
fn dashed(key: String, radius: f32, col: Color) -> Node {
    shape_node(key, Dashed { radius, width: 1.2, dash: 5.0, gap: 4.0, color: col }).abs_fill()
}

/// A fill rounded only at the top (or the bottom): the renderer rounds all four corners
/// or none, so a square one covers the other two, from the corner radius on. The two
/// overlap, so `col` must be opaque.
fn half_round(key: &str, col: Color, r: f32, top: bool) -> [Node; 2] {
    let (t, b) = if top { (r, 0.0) } else { (0.0, r) };
    [Node::new(format!("{key}/bg")).abs_fill().radius(r).fill(col), Node::new(format!("{key}/sq")).abs(Some(0.0), Some(t), Some(0.0), Some(b)).fill(col)]
}

#[derive(Debug)]
struct Dots {
    gap: f32,
    color: Color,
}

impl Shape for Dots {
    fn name(&self) -> &'static str {
        "dots"
    }

    fn emit(&self, cx: &ShapeCx, out: &mut Vec<Inst>) {
        let (s, (w, h)) = (cx.scale, cx.logical_size);
        let (x0, y0) = (cx.center_px[0] - w * s / 2.0, cx.center_px[1] - h * s / 2.0);
        let c = rgba_with_opacity(self.color, cx.inherited_opacity);
        let r = 0.75 * s;
        // ponytail: one instance per dot; a shader pattern if pages ever grow past a few thousand
        let (nx, ny) = ((w / self.gap) as usize, (h / self.gap) as usize);
        for j in 0..ny.min(80) {
            for i in 0..nx.min(120) {
                let (x, y) = (x0 + (i as f32 + 0.5) * self.gap * s, y0 + (j as f32 + 0.5) * self.gap * s);
                out.push(Inst { a: [x, y], b: [r, r], radius: r, kind: KIND_RECT, fill_top: c, fill_bot: c, clip: cx.clip_px, ..Default::default() });
            }
        }
    }
}

#[derive(Debug)]
struct Dashed {
    radius: f32,
    width: f32,
    dash: f32,
    gap: f32,
    color: Color,
}

impl Shape for Dashed {
    fn name(&self) -> &'static str {
        "dashed"
    }

    fn emit(&self, cx: &ShapeCx, out: &mut Vec<Inst>) {
        let s = cx.scale;
        let (w, h) = (cx.logical_size.0 * s, cx.logical_size.1 * s);
        let half = self.width * s / 2.0;
        let (x0, y0) = (cx.center_px[0] - w / 2.0 + half, cx.center_px[1] - h / 2.0 + half);
        let (x1, y1) = (x0 + w - 2.0 * half, y0 + h - 2.0 * half);
        let r = (self.radius * s).min((x1 - x0) / 2.0).min((y1 - y0) / 2.0).max(0.0);
        let c = rgba_with_opacity(self.color, cx.inherited_opacity);
        let (dash, gap) = (self.dash * s, self.gap * s);
        let mut edge = |a: [f32; 2], b: [f32; 2]| {
            let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
            let d = [(b[0] - a[0]) / len.max(1e-3), (b[1] - a[1]) / len.max(1e-3)];
            let mut t = 0.0;
            while t < len {
                let e = (t + dash).min(len);
                out.push(Inst { a: [a[0] + d[0] * t, a[1] + d[1] * t], b: [a[0] + d[0] * e, a[1] + d[1] * e], radius: half, kind: KIND_CAPSULE, fill_top: c, fill_bot: c, clip: cx.clip_px, ..Default::default() });
                t = e + gap;
            }
        };
        edge([x0 + r, y0], [x1 - r, y0]);
        edge([x1, y0 + r], [x1, y1 - r]);
        edge([x1 - r, y1], [x0 + r, y1]);
        edge([x0, y1 - r], [x0, y0 + r]);
        // the corners are solid quarter arcs, clockwise from 12 o'clock
        for (ax, ay, start) in [(x0 + r, y0 + r, 270.0f32), (x1 - r, y0 + r, 0.0), (x1 - r, y1 - r, 90.0), (x0 + r, y1 - r, 180.0)] {
            out.push(Inst { a: [ax, ay], b: [start.to_radians(), 90f32.to_radians()], radius: r, border: half, kind: KIND_ARC, fill_top: c, fill_bot: c, clip: cx.clip_px, ..Default::default() });
        }
    }
}

#[derive(Debug)]
struct Arrow {
    color: Color,
    width: f32,
}

impl Shape for Arrow {
    fn name(&self) -> &'static str {
        "arrow"
    }

    fn emit(&self, cx: &ShapeCx, out: &mut Vec<Inst>) {
        let (s, (w, h)) = (cx.scale, cx.logical_size);
        let p = |fx: f32, fy: f32| [cx.center_px[0] + (fx - 0.5) * w * s, cx.center_px[1] + (fy - 0.5) * h * s];
        let c = rgba_with_opacity(self.color, cx.inherited_opacity);
        for (a, b) in [(p(0.31, 0.69), p(0.68, 0.32)), (p(0.4, 0.32), p(0.68, 0.32)), (p(0.68, 0.32), p(0.68, 0.6))] {
            out.push(Inst { a, b, radius: self.width * s / 2.0, kind: KIND_CAPSULE, fill_top: c, fill_bot: c, clip: cx.clip_px, ..Default::default() });
        }
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
    pub popup_anchor_rects: HashMap<String, (f32, f32, f32, f32)>,
    /// A file is being dragged over the window.
    pub drop_hover: bool,
    /// The size tier the preview shows; none = the one the Instance's own size falls in.
    pub tier_tab: Option<String>,
    sel_module: Option<String>,
    advanced: bool,
    mdrag: Option<ModDrag>,
    /// What the last preview build placed, and where the last frame drew it.
    preview_arr: RefCell<Option<Arrangement>>,
    tray_key: RefCell<String>,
    preview_rects: HashMap<String, [f32; 4]>,
    /// The Widgets page shows Add a widget instead of the selected Instance.
    adding: bool,
    /// Add a widget's category; none = all.
    category: Option<String>,
    /// Search fields' text by input key (`q:widgets`, `q:log`).
    queries: HashMap<String, String>,
    /// The Log page's level; none = all.
    log_level: Option<Level>,
    /// The first-run setup's step, 0-based.
    step: usize,
    /// The adapter was changed here, so it waits for a restart.
    gpu_picked: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self { page: Page::Widgets, selected: None, scroll: HashMap::new(), focus: None, open: None, confirm_del: None, hsv: (0.6, 0.6, 1.0), caret_on: true, caret_at: Instant::now(), mods: ModifiersState::empty(), popup_anchor_rects: HashMap::new(), drop_hover: false, tier_tab: None, sel_module: None, advanced: false, mdrag: None, preview_arr: RefCell::new(None), tray_key: RefCell::new(String::new()), preview_rects: HashMap::new(), adding: false, category: None, queries: HashMap::new(), log_level: None, step: 0, gpu_picked: false }
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

    fn def_of<'a>(&self, ctx: &'a Ctx, widget: &str) -> Option<&'a WidgetMeta> {
        ctx.reg.get(widget).and_then(|d| d.as_ref().ok()).map(|w| w.meta())
    }

    /// "  ·  needs agents" when a data source the widget needs is missing.
    fn needs_note(&self, ctx: &Ctx, widget: &str) -> Option<String> {
        let unmet = self.def_of(ctx, widget)?.unmet(|n| ctx.sources.iter().any(|s| s == n));
        (!unmet.is_empty()).then(|| format!("  ·  needs {}", unmet.join(", ")))
    }

    fn param_value(cfg: &crate::workspace::InstanceCfg, p: &ParamDef) -> Value {
        cfg.params.get(&p.name).map(Value::from).unwrap_or_else(|| p.default.clone())
    }

    /// The Theme a Style row shows: the Workspace's, or the Instance's with its overrides.
    fn scope_theme(ctx: &Ctx, scope: &Scope) -> Theme {
        match scope {
            Scope::Instance(id) => match Self::instance(ctx, id) {
                Some(cfg) => ctx.ws.theme_for(ctx.lib, cfg),
                None => ctx.ws.global_theme(ctx.lib),
            },
            Scope::Global => ctx.ws.global_theme(ctx.lib),
        }
    }

    /// Whether `scope` sets `token` itself rather than inheriting it.
    fn style_is_set(ctx: &Ctx, scope: &Scope, token: &str) -> bool {
        match scope {
            Scope::Global => ctx.ws.style.contains_key(token),
            Scope::Instance(id) => Self::instance(ctx, id).is_some_and(|c| c.style.contains_key(token)),
        }
    }

    /// `<scope>:<token>`, the rest of a `sy:` key.
    fn style_target(rest: &str) -> Option<(Scope, &str)> {
        rest.split_once(':').map(|(s, t)| (Scope::parse(s), t))
    }

    fn resolve_color(ctx: &Ctx, v: &Value) -> Color {
        let s = v.to_string();
        match s.strip_prefix('$') {
            Some(tok) => ctx.theme.color(tok),
            None => Color::parse(&s).unwrap_or(crate::color::MAGENTA),
        }
    }

    fn dropdown_items(&self, ctx: &Ctx, key: &str) -> Vec<(String, String)> {
        let two = |v: &[(&str, &str)]| v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        if let Some(_id) = key.strip_prefix("z:") {
            return two(&[("desktop", "On the desktop (stays visible on Show Desktop)"), ("bottom", "Bottom (hidden by Show Desktop)"), ("normal", "Normal window"), ("topmost", "Always on top")]);
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
        if let Some((_, axis)) = key.strip_prefix("tp:").and_then(|r| r.split_once(':')) {
            let names: Vec<String> = match axis {
                "palette" => ctx.lib.palettes.iter().map(|a| a.name.clone()).collect(),
                "fonts" => ctx.lib.fonts.iter().map(|a| a.name.clone()).collect(),
                "glyphs" => ctx.lib.glyphs.iter().map(|a| a.name.clone()).collect(),
                _ => ctx.lib.icon_packs.clone(),
            };
            let global = (String::new(), format!("Global ({})", axis_of(&ctx.ws.theme, axis)));
            return std::iter::once(global).chain(names.into_iter().map(|n| (n.clone(), n))).collect();
        }
        if let Some((_, tok)) = key.strip_prefix("sy:").and_then(Self::style_target) {
            let Some(p) = style_schema().iter().find(|p| p.name == tok) else { return Vec::new() };
            if p.ty == ParamType::Font {
                return std::iter::once((String::new(), "Theme font".to_string())).chain(ctx.fonts.iter().map(|f| (f.clone(), f.clone()))).collect();
            }
            return p.choices.iter().map(|c| (c.value.clone(), capitalized(&c.label))).collect();
        }
        if let Some(rest) = key.strip_prefix("p:") {
            if let Some((id, name)) = rest.split_once(':') {
                let def = ctx.ws.instances.iter().find(|c| c.id == id).and_then(|c| self.def_of(ctx, &c.widget));
                if let Some(p) = def.and_then(|d| d.params.iter().find(|p| p.name == name)) {
                    return match p.ty {
                        ParamType::Font => std::iter::once((String::new(), "Theme font".to_string())).chain(ctx.fonts.iter().map(|f| (f.clone(), f.clone()))).collect(),
                        _ => p.choices.iter().map(|c| (c.value.clone(), c.label.clone())).collect(),
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
        if let Some((id, axis)) = key.strip_prefix("tp:").and_then(|r| r.split_once(':')) {
            return Self::instance(ctx, id).and_then(|c| picked(&c.theme, axis)).unwrap_or_default();
        }
        if let Some((scope, tok)) = key.strip_prefix("sy:").and_then(Self::style_target) {
            if self.is_font_key(ctx, key) {
                // a font nobody set is the theme's own: the list's empty "Theme font"
                let own = match &scope {
                    Scope::Instance(id) => Self::instance(ctx, id).and_then(|c| c.style.get(tok)),
                    Scope::Global => None,
                };
                return own.or(ctx.ws.style.get(tok)).map(|v| Value::from(v).to_string()).unwrap_or_default();
            }
            return Self::scope_theme(ctx, &scope).str(tok);
        }
        if let Some((id, name)) = key.strip_prefix("p:").and_then(|r| r.split_once(':')) {
            // an unsaved param is its default, as the widget draws it
            let cfg = Self::instance(ctx, id);
            let p = cfg.and_then(|c| self.def_of(ctx, &c.widget)).and_then(|d| d.params.iter().find(|p| p.name == name));
            return cfg.zip(p).map(|(c, p)| Self::param_value(c, p).to_string()).unwrap_or_default();
        }
        String::new()
    }

    fn dropdown_label(&self, ctx: &Ctx, key: &str) -> String {
        let cur = self.dropdown_current(ctx, key);
        self.dropdown_items(ctx, key).into_iter().find(|(v, _)| *v == cur).map(|(_, l)| l).unwrap_or_else(|| if cur.is_empty() { "Choose…".into() } else { cur })
    }

    /// A param's or Style token's dropdown, showing its choice; a font in its own face.
    fn dropdown_for(&self, k: &Kit, ctx: &Ctx, key: &str) -> Node {
        let open = matches!(&self.open, Some(Open::Dropdown(o)) if o == key);
        let mut n = k.dropdown(key, &self.dropdown_label(ctx, key), CONTROL_W, open);
        let cur = self.dropdown_current(ctx, key);
        if !cur.is_empty() && self.is_font_key(ctx, key) {
            if let Some(Kind::Text(t)) = n.children.first_mut().map(|c| &mut c.kind) {
                t.family = cur;
            }
        }
        n
    }

    /// Whether a dropdown lists fonts, which it then draws each in its own face.
    fn is_font_key(&self, ctx: &Ctx, key: &str) -> bool {
        if let Some((_, tok)) = key.strip_prefix("sy:").and_then(Self::style_target) {
            return style_schema().iter().any(|p| p.name == tok && p.ty == ParamType::Font);
        }
        let param = key.strip_prefix("p:").and_then(|r| r.split_once(':')).and_then(|(id, name)| {
            let cfg = Self::instance(ctx, id)?;
            self.def_of(ctx, &cfg.widget)?.params.iter().find(|p| p.name == name).cloned()
        });
        param.is_some_and(|p| p.ty == ParamType::Font)
    }

    /// A dropdown's items, narrowed by its search when the list is long enough to have one.
    fn dropdown_shown(&self, ctx: &Ctx, key: &str) -> Vec<(String, String)> {
        let items = self.dropdown_items(ctx, key);
        let q = self.input_text(ctx, "q:dd").to_lowercase();
        if items.len() <= SEARCH_FROM || q.is_empty() {
            return items;
        }
        items.into_iter().filter(|(_, l)| l.to_lowercase().contains(&q)).collect()
    }

    fn color_of_target(&self, ctx: &Ctx, target: &str) -> Color {
        if let Some((scope, tok)) = target.strip_prefix("sy:").and_then(Self::style_target) {
            return Self::scope_theme(ctx, &scope).color(tok);
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
        if let Some((scope, tok)) = target.strip_prefix("sy:").and_then(Self::style_target) {
            return Some(Cmd::Style(scope, tok.into(), Some(Value::Str(hex))));
        }
        let (id, name) = target.strip_prefix("p:")?.split_once(':')?;
        Some(Cmd::Param(id.to_string(), name.to_string(), Value::Str(hex)))
    }

    /// (min, max, step, current) for a slider key.
    fn slider_spec(&self, ctx: &Ctx, key: &str) -> Option<(f64, f64, f64, f64)> {
        if key == "grid" {
            return Some((0.0, 32.0, 4.0, ctx.ws.grid as f64));
        }
        if let Some((scope, tok)) = key.strip_prefix("sy:").and_then(Self::style_target) {
            let p = style_schema().iter().find(|p| p.name == tok)?;
            let cur = Self::scope_theme(ctx, &scope).num(tok) as f64;
            return Some((p.min.unwrap_or(0.0), p.max.unwrap_or(100.0), p.step.unwrap_or(1.0), cur));
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
        if let Some((scope, tok)) = key.strip_prefix("sy:").and_then(Self::style_target) {
            return Some(Cmd::Style(scope, tok.into(), Some(Value::Num(v))));
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
        if key.starts_with("q:") {
            return self.query(key);
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

    /// A search field's text.
    fn query(&self, key: &str) -> String {
        self.queries.get(key).cloned().unwrap_or_default()
    }

    fn scroll_of(&self, key: &str) -> f32 {
        self.scroll.get(key).copied().unwrap_or(0.0)
    }

    /// Also returns the image ids to upload.
    pub fn build(&self, ctx: &Ctx, size: (f32, f32)) -> (Node, Vec<String>) {
        let k = Kit::new(ctx.theme);
        let mut images = Vec::new();
        let mut card = Node::new("s/card").col().wh(size.0, size.1).fill(k.bg()).clip();
        if ctx.ws.onboarded {
            let body = match self.page {
                Page::Widgets => self.page_widgets(&k, ctx, size, &mut images),
                Page::Appearance => self.page_appearance(&k, ctx, size),
                Page::Plugins => self.page_plugins(&k, ctx, size),
                Page::General => self.page_general(&k, ctx, size),
                Page::Log => self.page_log(&k, ctx, size),
            };
            // a new key per page replays the enter animation
            let page = Node::new(format!("s/page/{}", self.page.id())).col().grow(1.0).min_h(0.0).enter(240, 10.0, 0).child(body);
            card = card.child(self.header(&k, ctx, size)).child(page);
        } else {
            card = card.child(self.setup(&k, ctx, size, &mut images));
        }
        let mut root = Node::new("s").wh(size.0, size.1).child(card);
        if let Some(o) = self.module_overlay(&k, ctx, &mut images) {
            root = root.child(o);
        }
        if let Some(o) = &self.open {
            root = root.child(self.popup(&k, ctx, o, size));
        }
        (root, images)
    }

    /// The brand, the page tabs, Edit layout and Quit.
    fn header(&self, k: &Kit, ctx: &Ctx, size: (f32, f32)) -> Node {
        let mut brand = Node::new("s/brand").row().grow(1.0).align(taffy::AlignItems::CENTER).gap(10.0).child(k.logo("s/logo", 30.0));
        if size.0 >= 960.0 {
            brand = brand.child(k.bold("s/brand/t".into(), "Wayfinder", 16.0, k.c("text")).with_text(|t| t.weight = 700));
        }
        let mut tabs = Node::new("s/tabs").row().no_shrink().gap(2.0).pad(4.0).radius(12.0).fill(k.ink(0.025)).border(1.0, k.line());
        for p in Page::ALL {
            let on = p == self.page;
            let col = if on { k.c("text") } else { k.c("text-dim") };
            tabs = tabs.child(
                Node::new(format!("s/tab/{}", p.id()))
                    .row()
                    .h(34.0)
                    .pad_xy(14.0, 0.0)
                    .gap(8.0)
                    .align(taffy::AlignItems::CENTER)
                    .radius(9.0)
                    .fill(k.c("accent").with_alpha(if on { 0.16 } else { 0.0 }))
                    .hover_fill(if on { k.c("accent").with_alpha(0.2) } else { k.ink(0.05) })
                    .ease(170)
                    .on(format!("nav:{}", p.id()))
                    .child(k.glyph(format!("s/tab/{}/g", p.id()), p.glyph(), 14.0, if on { k.c("accent") } else { col }))
                    .child(k.txt(format!("s/tab/{}/t", p.id()), p.title(), 13.5, col).with_text(|t| t.weight = if on { 600 } else { 400 })),
            );
        }
        let on = ctx.edit;
        let fg = if on { k.c("accent-text") } else { k.c("text") };
        let mut edit = Node::new("s/edit")
            .row()
            .h(38.0)
            .pad_xy(12.0, 0.0)
            .gap(10.0)
            .align(taffy::AlignItems::CENTER)
            .radius(10.0)
            .fill(if on { k.c("accent") } else { k.ink(0.02) })
            .hover_fill(if on { k.c("accent").mul_alpha(0.86) } else { k.ink(0.06) })
            .border(1.0, if on { TRANSPARENT } else { k.line() })
            .ease(160)
            .on("edit:toggle")
            .child(k.glyph("s/edit/g".into(), "move", 15.0, if on { fg } else { k.c("accent") }))
            .child(k.bold("s/edit/t".into(), if on { "Finish editing" } else { "Edit layout" }, 13.5, fg));
        if !on && size.0 >= 1060.0 {
            edit = edit.child(k.keys("s/edit/k", EDIT_KEYS));
        }
        let quit = k.icon_btn("s/quit", "power", "quit".into(), k.c("text-dim"), k.c("danger").with_alpha(0.2));
        let right = Node::new("s/right").row().grow(1.0).align(taffy::AlignItems::CENTER).justify(taffy::JustifyContent::FLEX_END).gap(8.0).child(edit).child(quit);
        Node::new("s/header")
            .row()
            .h(HEADER_H)
            .no_shrink()
            .align(taffy::AlignItems::CENTER)
            .pad_xy(18.0, 0.0)
            .gap(16.0)
            .fill(k.ink(0.02))
            .child(brand)
            .child(tabs)
            .child(right)
            .child(Node::new("s/header/hr").abs(Some(0.0), None, Some(0.0), Some(0.0)).h(1.0).fill(k.line()))
    }

    fn scrolling(&self, key: &str, content: Node) -> Node {
        Node::new(key).col().grow(1.0).min_h(0.0).scroll(self.scroll_of(key)).hit().child(content)
    }

    /// A page's scrolling body: a column `w` wide (see `content_w`), centred in the window.
    fn page(&self, key: &str, w: f32, content: Node) -> Node {
        // a set width: a percentage of a scrolling parent resolves as unknown
        let wrap = Node::new(format!("{key}/wrap")).col().align(taffy::AlignItems::CENTER).pad_xy(PAGE_PAD, 34.0).child(content.w(w));
        self.scrolling(key, wrap)
    }

    fn page_widgets(&self, k: &Kit, ctx: &Ctx, size: (f32, f32), images: &mut Vec<String>) -> Node {
        let right_w = content_w(size, 1040.0) - SIDEBAR_W - 24.0;
        let sel = if self.adding { None } else { self.selected_cfg(ctx) };
        let mut list = Node::new("w/list")
            .col()
            .w(SIDEBAR_W)
            .no_shrink()
            .gap(2.0)
            .pad(8.0)
            .radius(14.0)
            .fill(k.panel())
            .border(1.0, k.line())
            .child(Node::new("w/list/h").pad_xy(10.0, 8.0).child(k.bold("w/list/h/t".into(), &format!("On your desktop · {}", ctx.ws.instances.len()), 12.0, k.c("text-dim"))));
        if ctx.ws.instances.is_empty() {
            list = list.child(Node::new("w/empty").pad_xy(10.0, 6.0).child(k.txt("w/empty/t".into(), "Nothing yet. Add one to get started.", 12.5, k.c("text-dim")).wrap_text()));
        }
        for (i, c) in ctx.ws.instances.iter().enumerate() {
            list = list.child(self.instance_item(k, ctx, c, sel.is_some_and(|s| s.id == c.id), i));
        }
        let gallery = sel.is_none();
        let add = Node::new("w/addbtn")
            .row()
            .h(46.0)
            .pad_xy(10.0, 0.0)
            .gap(12.0)
            .align(taffy::AlignItems::CENTER)
            .radius(10.0)
            .fill(k.c("accent").with_alpha(if gallery { 0.1 } else { 0.03 }))
            .hover_fill(k.c("accent").with_alpha(0.13))
            .ease(140)
            .on("gallery:open")
            .child(dashed("w/addbtn/d".into(), 10.0, k.c("accent").with_alpha(0.55)))
            .child(Node::new("w/addbtn/ic").wh(26.0, 26.0).no_shrink().radius(7.0).center().fill(k.c("accent").with_alpha(0.16)).child(k.glyph("w/addbtn/ic/g".into(), "add", 12.0, k.c("accent"))))
            .child(k.bold("w/addbtn/t".into(), "Add widget", 13.5, k.c("text")).grow_text())
            .child(k.txt("w/addbtn/n".into(), &format!("{} types", ctx.reg.ids().len()), 11.5, k.c("text-dim")));
        list = list.child(Node::new("w/list/sp").h(6.0)).child(add);
        let right = match sel {
            Some(cfg) => self.instance_panel(k, ctx, cfg, images),
            None => self.gallery(k, ctx, right_w, !ctx.ws.instances.is_empty(), images),
        };
        // min_w(0): text reports its unwrapped width as min-content, so a long description would push Remove out
        let cols = Node::new("w/cols").row().gap(24.0).align(taffy::AlignItems::FLEX_START).child(list).child(Node::new("w/right").col().grow(1.0).min_w(0.0).child(right));
        let body = Node::new("w").col().gap(26.0).child(k.page_head("w/head", Page::Widgets.heading(), Page::Widgets.subtitle(), None)).child(cols);
        self.page("w/scroll-r", content_w(size, 1040.0), body)
    }

    /// An Instance in the list on the left of the Widgets page.
    fn instance_item(&self, k: &Kit, ctx: &Ctx, c: &crate::workspace::InstanceCfg, on: bool, i: usize) -> Node {
        let def = self.def_of(ctx, &c.widget);
        let mut name = def.map_or(c.widget.clone(), |d| d.name.clone());
        if ctx.ws.instances.iter().filter(|o| o.widget == c.widget).count() > 1 {
            name = format!("{name} {}", c.id.rsplit('-').next().unwrap_or(""));
        }
        let note = ctx.hidden.iter().find(|(id, _)| *id == c.id).map(|(_, h)| match h {
            Hidden::Parked => "Parked: monitor missing".to_string(),
            Hidden::PluginOff(p) => format!("Hidden: plugin {p} is off"),
        });
        let note = note.or_else(|| self.needs_note(ctx, &c.widget).map(|n| capitalized(n.trim_start_matches("  ·  "))));
        let key = format!("w/i/{}", c.id);
        let mut text = Node::new(format!("{key}/c")).col().grow(1.0).min_w(0.0).gap(1.0).child(k.txt(format!("{key}/n"), &name, 13.5, k.c("text")).with_text(|t| t.weight = if on { 600 } else { 400 }));
        if let Some(n) = &note {
            text = text.child(k.txt(format!("{key}/why"), n, 11.5, k.c("danger")).wrap_text());
        }
        let icon = Node::new(format!("{key}/ic")).wh(30.0, 30.0).no_shrink().radius(8.0).center().fill(k.ink(0.05)).child(k.glyph(format!("{key}/ic/g"), &k.widget_glyph(def), 13.0, if on { k.c("accent") } else { k.c("text-dim") }));
        let mut row = Node::new(key.clone())
            .row()
            .min_h(44.0)
            .pad_xy(8.0, 6.0)
            .gap(12.0)
            .align(taffy::AlignItems::CENTER)
            .radius(9.0)
            .fill(if on { k.c("accent").with_alpha(0.12) } else { k.ink(0.0) })
            .hover_fill(if on { k.c("accent").with_alpha(0.16) } else { k.ink(0.05) })
            .ease(150)
            .on(format!("sel:{}", c.id))
            .enter(220, 6.0, (i as u32).min(10) * 24)
            .child(icon)
            .child(text);
        if let Some(b) = self.plugin_badge(ctx, &c.widget) {
            row = row.child(k.tag(format!("{key}/b"), &b, false));
        }
        row
    }

    /// The Plugin only a Widget comes from, as its badge: `Wayfinder Extra` is `Extra`.
    fn plugin_badge(&self, ctx: &Ctx, widget: &str) -> Option<String> {
        let r = ctx.plugins.iter().find(|r| r.sole_widgets.iter().any(|w| w == widget))?;
        Some(r.name.strip_prefix("Wayfinder ").unwrap_or(&r.name).to_string())
    }

    /// Every Widget the catalog has, with its category, in category order and then by name.
    fn catalog(&self, ctx: &Ctx) -> Vec<(String, String)> {
        let rank = |c: &str| CATEGORIES.iter().position(|x| *x == c).unwrap_or(if c == "Other" { CATEGORIES.len() + 1 } else { CATEGORIES.len() });
        let mut v: Vec<(String, String, String)> = ctx
            .reg
            .ids()
            .into_iter()
            .map(|id| {
                let m = self.def_of(ctx, &id);
                let cat = m.map(|m| m.category.clone()).filter(|c| !c.is_empty()).unwrap_or_else(|| "Other".into());
                (cat, m.map_or(id.clone(), |m| m.name.clone()), id)
            })
            .collect();
        v.sort_by(|a, b| (rank(&a.0), &a.0, &a.1).cmp(&(rank(&b.0), &b.0, &b.1)));
        v.into_iter().map(|(c, _, id)| (id, c)).collect()
    }

    /// Add a widget: search, categories and a card per Widget.
    fn gallery(&self, k: &Kit, ctx: &Ctx, w: f32, closable: bool, images: &mut Vec<String>) -> Node {
        let all = self.catalog(ctx);
        let q = self.query("q:widgets").to_lowercase();
        let matches = |id: &str| {
            q.is_empty() || self.def_of(ctx, id).map_or(id.to_string(), |m| format!("{} {}", m.name, m.description)).to_lowercase().contains(&q)
        };
        let shown: Vec<String> = all.iter().filter(|(id, c)| self.category.as_ref().is_none_or(|x| x == c) && matches(id)).map(|(id, _)| id.clone()).collect();
        let mut cats: Vec<(&str, usize)> = Vec::new();
        for (_, c) in &all {
            match cats.last_mut() {
                Some((last, n)) if *last == c.as_str() => *n += 1,
                _ => cats.push((c, 1)),
            }
        }
        let mut head = Node::new("w/g/h")
            .row()
            .align(taffy::AlignItems::FLEX_START)
            .gap(12.0)
            .child(Node::new("w/g/h/l").col().grow(1.0).min_w(0.0).gap(4.0).child(k.bold("w/g/h/t".into(), "Add a widget", 19.0, k.c("text")).with_text(|t| t.weight = 700)).child(k.txt("w/g/h/s".into(), "Pick one and it lands on your desktop — then drag it where you want it.", 12.5, k.c("text-dim")).wrap_text()));
        if closable {
            head = head.child(k.btn("w/g/close", Some("close"), "Close", "gallery:close".into(), Btn::Outline));
        }
        let fq = self.focus.as_ref().filter(|f| f.key == "q:widgets").map(|f| (f.caret, self.caret_on));
        let mut filters = Node::new("w/g/f").row().wrap().gap(8.0).align(taffy::AlignItems::CENTER).child(k.input("q:widgets", &self.input_text(ctx, "q:widgets"), "Search widgets…", fq, 220.0, false));
        filters = filters.child(self.chip(k, "w/g/cat/all", "All", all.len(), self.category.is_none(), "cat:".into()));
        for (c, n) in cats {
            filters = filters.child(self.chip(k, &format!("w/g/cat/{c}"), c, n, self.category.as_deref() == Some(c), format!("cat:{c}")));
        }
        Node::new("w/g").col().gap(18.0).child(head).child(filters).child(self.widget_grid(k, ctx, w, "w/g/grid", &shown, images))
    }

    fn chip(&self, k: &Kit, key: &str, label: &str, count: usize, on: bool, action: String) -> Node {
        Node::new(key)
            .row()
            .h(36.0)
            .pad_xy(12.0, 0.0)
            .gap(7.0)
            .align(taffy::AlignItems::CENTER)
            .radius(9.0)
            .fill(k.c("accent").with_alpha(if on { 0.12 } else { 0.0 }))
            .hover_fill(if on { k.c("accent").with_alpha(0.16) } else { k.ink(0.05) })
            .border(1.0, if on { k.c("accent").with_alpha(0.5) } else { k.line() })
            .ease(140)
            .on(action)
            .child(k.txt(format!("{key}/t"), label, 13.0, if on { k.c("text") } else { k.c("text-dim") }).with_text(|t| t.weight = if on { 600 } else { 400 }))
            .child(k.txt(format!("{key}/n"), &count.to_string(), 11.5, k.c("text-dim")))
    }

    fn widget_grid(&self, k: &Kit, ctx: &Ctx, w: f32, key: &str, ids: &[String], images: &mut Vec<String>) -> Node {
        if ids.is_empty() {
            return Node::new(key).pad_xy(0.0, 20.0).child(k.txt(format!("{key}/none"), "No widget matches.", 13.0, k.c("text-dim")));
        }
        let (cols, cw) = columns(w, 200.0, 14.0, 4);
        grid(key, cols, 14.0, ids.iter().enumerate().map(|(i, id)| self.widget_card(k, ctx, id, cw, key, i, images)).collect())
    }

    fn widget_card(&self, k: &Kit, ctx: &Ctx, id: &str, w: f32, prefix: &str, i: usize, images: &mut Vec<String>) -> Node {
        let key = format!("{prefix}/c/{id}");
        let def = self.def_of(ctx, id);
        let accent = k.c("accent");
        let preview = self.widget_preview(ctx, id, (w - 24.0, GALLERY_PV_H - 24.0), &format!("{key}/pv"), images);
        let mut top = Node::new(format!("{key}/top")).h(GALLERY_PV_H).no_shrink().center().clip().child(dots(format!("{key}/dots"), k.ink(0.1)));
        // the Widget itself, live; its icon when it cannot show (a data source is missing)
        top = top.child(preview.unwrap_or_else(|| k.tile(format!("{key}/ic"), &k.widget_glyph(def), 46.0, accent)));
        if ctx.ws.instances.iter().any(|c| c.widget == id) {
            top = top.child(k.badge(format!("{key}/on"), "On desktop", accent).abs(None, Some(8.0), Some(8.0), None));
        }
        let mut title = Node::new(format!("{key}/tt")).row().wrap().gap(8.0).align(taffy::AlignItems::CENTER).child(k.bold(format!("{key}/n"), &def.map_or(id.to_string(), |d| d.name.clone()), 13.5, k.c("text")));
        if let Some(b) = self.plugin_badge(ctx, id) {
            title = title.child(k.tag(format!("{key}/badge"), &b, false));
        }
        let mut body = Node::new(format!("{key}/b")).col().grow(1.0).pad(14.0).gap(6.0).child(title);
        match def {
            Some(d) => body = body.child(k.txt(format!("{key}/d"), &d.description, 12.0, k.c("text-dim")).wrap_text()),
            None => body = body.child(k.txt(format!("{key}/d"), "Its definition failed to load; see the Log page.", 12.0, k.c("danger")).wrap_text()),
        }
        if let Some(n) = self.needs_note(ctx, id) {
            body = body.child(k.txt(format!("{key}/needs"), &capitalized(n.trim_start_matches("  ·  ")), 12.0, k.c("danger")));
        }
        let add = Node::new(format!("{key}/add"))
            .row()
            .h(36.0)
            .gap(8.0)
            .center()
            .radius(9.0)
            .fill(accent.with_alpha(0.06))
            .hover_fill(accent.with_alpha(0.15))
            .border(1.0, accent.with_alpha(0.4))
            .ease(130)
            .on(format!("add:{id}"))
            .child(k.glyph(format!("{key}/add/g"), "add", 12.0, accent))
            .child(k.bold(format!("{key}/add/t"), "Add to desktop", 13.0, accent));
        body = body.child(Node::new(format!("{key}/sp")).grow(1.0).min_h(8.0)).child(add);
        Node::new(key.clone())
            .col()
            .w(w)
            .radius(12.0)
            .fill(k.panel())
            .border(1.0, k.line())
            .clip()
            .enter(220, 8.0, (i as u32).min(12) * 25)
            .child(top)
            .child(Node::new(format!("{key}/hr")).h(1.0).no_shrink().fill(k.line()))
            .child(body)
    }

    /// Widget `id` as it looks on a desktop, live and inert, at its default shape made to fit
    /// `fit` (never above its default size, never below its minimum). A Widget already placed
    /// shows as its first Instance is set up; one that cannot run (a missing data source) shows none.
    // ponytail: every card builds its Widget each frame; cache them if Add a widget ever feels slow
    fn widget_preview(&self, ctx: &Ctx, id: &str, fit: (f32, f32), key: &str, images: &mut Vec<String>) -> Option<Node> {
        let meta = self.def_of(ctx, id)?;
        if !meta.unmet(|n| ctx.sources.iter().any(|s| s == n)).is_empty() {
            return None;
        }
        let (dw, dh) = meta.default_card_size;
        let k = (fit.0 / dw.max(1.0)).min(fit.1 / dh.max(1.0)).min(1.0);
        let size = ((dw * k).max(meta.min_card_size.0), (dh * k).max(meta.min_card_size.1));
        // a placed one shares its Instance's data; any other borrows an id no Instance has
        let cfg = match ctx.ws.instances.iter().find(|c| c.widget == id) {
            Some(c) => crate::workspace::InstanceCfg { w: size.0, h: size.1, ..c.clone() },
            None => crate::workspace::InstanceCfg { id: format!("preview:{id}"), widget: id.into(), w: size.0, h: size.1, ..Default::default() },
        };
        let b = self.build_widget(ctx, &cfg, &crate::workspace::Layout::new(), None, size, key).ok()?;
        images.extend(b.image_ids.iter().cloned());
        let mut root = b.root;
        inert(&mut root);
        Some(root)
    }

    fn instance_panel(&self, k: &Kit, ctx: &Ctx, cfg: &crate::workspace::InstanceCfg, images: &mut Vec<String>) -> Node {
        let id = &cfg.id;
        let def = self.def_of(ctx, &cfg.widget);
        let title = def.map_or(cfg.widget.clone(), |d| d.name.clone());
        let remove = if self.confirm_del.as_deref() == Some(id.as_str()) {
            k.btn(&format!("ip/{id}/del"), Some("delete"), "Really remove?", format!("del:{id}"), Btn::DangerFill)
        } else {
            k.btn(&format!("ip/{id}/del"), Some("delete"), "Remove", format!("del:{id}"), Btn::Danger)
        };
        let head = Node::new(format!("ip/{id}/h"))
            .row()
            .align(taffy::AlignItems::CENTER)
            .gap(14.0)
            .child(k.tile(format!("ip/{id}/ic"), &k.widget_glyph(def), 46.0, k.c("accent")))
            .child(Node::new(format!("ip/{id}/hl")).col().grow(1.0).min_w(0.0).gap(3.0).child(k.bold(format!("ip/{id}/ht"), &title, 19.0, k.c("text")).with_text(|t| t.weight = 700)).child(k.txt(format!("ip/{id}/hs"), def.map_or("", |d| d.description.as_str()), 12.5, k.c("text-dim")).wrap_text()))
            .child(remove);
        let mut p = Node::new(format!("ip/{id}")).col().gap(22.0).child(head).child(self.preview_block(k, ctx, cfg, images));
        match def {
            Some(d) if !d.modules.is_empty() => p = p.child(self.module_options(k, ctx, cfg, d, images)),
            Some(d) => {
                for (i, (group, params)) in ParamDef::grouped(&d.params).into_iter().enumerate() {
                    let rows = params.into_iter().map(|pd| self.param_row(k, ctx, cfg, pd, images)).collect();
                    p = p.child(k.group(&format!("ip/{id}/s2/{i}"), group.unwrap_or("Options"), rows));
                }
            }
            None => p = p.child(k.txt(format!("ip/{id}/err"), "This widget's definition failed to load; see the Log page.", 12.5, k.c("danger")).wrap_text()),
        }
        let adv = Node::new(format!("ip/{id}/adv"))
            .row()
            .h(36.0)
            .align(taffy::AlignItems::CENTER)
            .gap(10.0)
            .pad_xy(6.0, 0.0)
            .radius(8.0)
            .hover_fill(k.ink(0.05))
            .ease(120)
            .on("adv:toggle")
            .child(k.glyph(format!("ip/{id}/adv/g"), if self.advanced { "chevron-down" } else { "chevron-right" }, 11.0, k.c("text-dim")))
            .child(k.bold(format!("ip/{id}/adv/t"), "Advanced: layer, click-through, size limit, style", 13.0, k.c("text-dim")));
        p = p.child(adv);
        if self.advanced {
            let zk = format!("z:{id}");
            let z = k.dropdown(&zk, &self.dropdown_label(ctx, &zk), CONTROL_W, matches!(&self.open, Some(Open::Dropdown(o)) if *o == zk));
            let mut place = vec![
                k.row(&format!("ip/{id}/z"), "Layer", "Where it sits relative to other windows", z),
                k.row(&format!("ip/{id}/ct"), "Click-through", "Clicks pass to whatever is underneath", k.toggle(&format!("ct:{id}"), cfg.click_through, format!("ct:{id}"))),
            ];
            if def.is_some_and(|d| d.max_card_size.is_some()) {
                place.push(k.row(&format!("ip/{id}/lim"), "Size limit", "Keep it within the size it was designed for. Off lets it grow larger; the minimum always applies.", k.toggle(&format!("lim:{id}"), cfg.size_limit, format!("lim:{id}"))));
            }
            let pos = Node::new(format!("ip/{id}/pl")).row().gap(8.0).child(k.btn(&format!("ip/{id}/edit"), Some("move"), "Edit layout", "edit:toggle".into(), Btn::Outline)).child(k.btn(&format!("ip/{id}/reset"), None, "Reset position", format!("reset:{id}"), Btn::Outline));
            place.push(k.row(&format!("ip/{id}/pos"), "Position", &format!("{:.0}, {:.0}  ·  {:.0} × {:.0}", cfg.x, cfg.y, cfg.w, cfg.h), pos));
            p = p.child(k.group(&format!("ip/{id}/s1"), "Placement", place));
            let mut style: Vec<Node> = THEME_AXES
                .iter()
                .map(|(axis, label)| {
                    let key = format!("tp:{id}:{axis}");
                    let open = matches!(&self.open, Some(Open::Dropdown(o)) if *o == key);
                    k.row(&format!("ip/{id}/tp/{axis}"), label, "", k.dropdown(&key, &self.dropdown_label(ctx, &key), CONTROL_W, open))
                })
                .collect();
            let scope = Scope::Instance(id.clone());
            style.extend(style_schema().iter().map(|pd| self.style_row(k, ctx, &scope, pd)));
            if !cfg.style.is_empty() || !cfg.theme.is_empty() {
                style.push(k.row(&format!("ip/{id}/rs"), "Reset style", "Back to the global style", k.btn(&format!("ip/{id}/rs/b"), None, "Reset style", format!("syreset:{id}|*"), Btn::Outline)));
            }
            p = p.child(k.group(&format!("ip/{id}/s3"), "Style", style));
        }
        p.child(Node::new(format!("ip/{id}/pad")).h(8.0))
    }

    /// One Style token at one scope, built like a Widget param's row.
    fn style_row(&self, k: &Kit, ctx: &Ctx, scope: &Scope, pd: &ParamDef) -> Node {
        let (sk, tok) = (scope.key(), pd.name.as_str());
        let key = format!("sy:{sk}:{tok}");
        let rk = format!("sr/{sk}/{tok}");
        let theme = Self::scope_theme(ctx, scope);
        let (label, unit) = split_unit(&pd.label);
        let control = match pd.ty {
            ParamType::Bool => k.toggle(&format!("tg:{key}"), theme.flag(tok), format!("sy:{sk}|{tok}")),
            ParamType::Number | ParamType::Duration => {
                let (min, max, _step, cur) = self.slider_spec(ctx, &key).unwrap_or((0.0, 100.0, 1.0, 0.0));
                let frac = if max > min { ((cur - min) / (max - min)) as f32 } else { 0.0 };
                Node::new(format!("{rk}/sc")).row().align(taffy::AlignItems::CENTER).gap(12.0).child(k.slider(&format!("sl:{key}"), frac, 190.0, format!("sl:{key}"))).child(Node::new(format!("{rk}/vw")).w(48.0).child(k.txt(format!("{rk}/v"), &with_unit(cur, unit), 12.5, k.c("text"))))
            }
            ParamType::Color if tok == "accent" => self.accent_swatches(k, ctx, &key, &rk, theme.color(tok)),
            ParamType::Color => {
                let f = self.focus.as_ref().filter(|f| f.key == format!("hx:{key}")).map(|f| (f.caret, self.caret_on));
                Node::new(format!("{rk}/cc"))
                    .row()
                    .align(taffy::AlignItems::CENTER)
                    .gap(8.0)
                    .child(k.swatch(&format!("{rk}/sw"), theme.color(tok), 28.0, Some(format!("cp:{key}")), matches!(&self.open, Some(Open::Color(o)) if *o == key)))
                    .child(k.input(&format!("hx:{key}"), &self.input_text(ctx, &format!("hx:{key}")), "#rrggbb", f, 104.0, true))
            }
            _ => self.dropdown_for(k, ctx, &key),
        };
        let set = Self::style_is_set(ctx, scope, tok);
        let mut title = Node::new(format!("{rk}/tt")).row().wrap().gap(8.0).align(taffy::AlignItems::CENTER).child(k.bold(format!("{rk}/lt"), label, 13.5, k.c("text")));
        let mut c = Node::new(format!("{rk}/c")).row().align(taffy::AlignItems::CENTER).gap(10.0).child(control);
        if set {
            title = title.child(k.badge(format!("{rk}/badge"), if matches!(scope, Scope::Instance(_)) { "Own" } else { "Changed" }, k.c("accent")));
            c = c.child(
                Node::new(format!("{rk}/reset"))
                    .h(28.0)
                    .pad_xy(8.0, 0.0)
                    .center()
                    .radius(7.0)
                    .hover_fill(k.ink(0.07))
                    .ease(120)
                    .on(format!("syreset:{sk}|{tok}"))
                    .child(k.txt(format!("{rk}/reset/t"), "Reset", 12.5, k.c("text-dim"))),
            );
        }
        k.row_with(&rk, title, &pd.help, None, c)
    }

    /// The palette's own accent, four others, a custom one and its hex.
    fn accent_swatches(&self, k: &Kit, ctx: &Ctx, key: &str, rk: &str, cur: Color) -> Node {
        let own = ctx.lib.palette(&ctx.ws.theme.palette).tokens.get("accent").map(|v| v.to_string()).and_then(|s| Color::parse(&s)).unwrap_or(cur);
        let presets = [own, Color::parse("#6b8cff").unwrap(), Color::parse("#f0913a").unwrap(), Color::parse("#c39bff").unwrap(), Color::parse("#ff6b8a").unwrap()];
        let hex = cur.to_hex();
        let mut row = Node::new(format!("{rk}/ac")).row().align(taffy::AlignItems::CENTER).gap(8.0);
        for (i, c) in presets.iter().enumerate() {
            row = row.child(k.swatch(&format!("{rk}/p{i}"), *c, 28.0, Some(format!("setc:{key}|{}", c.to_hex())), c.to_hex() == hex));
        }
        let custom = !presets.iter().any(|c| c.to_hex() == hex);
        let picker = Node::new(format!("{rk}/cp"))
            .wh(28.0, 28.0)
            .no_shrink()
            .radius(14.0)
            .center()
            .fill(if custom { cur } else { k.ink(0.0) })
            .border(if custom { 2.5 } else { 1.0 }, if custom { k.c("text") } else { k.line() })
            .hover_fill(if custom { cur.mul_alpha(0.8) } else { k.ink(0.07) })
            .ease(120)
            .on(format!("cp:{key}"));
        let picker = if custom { picker } else { picker.child(k.glyph(format!("{rk}/cp/g"), "edit", 11.0, k.c("text-dim"))) };
        let f = self.focus.as_ref().filter(|f| f.key == format!("hx:{key}")).map(|f| (f.caret, self.caret_on));
        row.child(picker).child(Node::new(format!("{rk}/gap")).w(4.0)).child(k.input(&format!("hx:{key}"), &self.input_text(ctx, &format!("hx:{key}")), "#rrggbb", f, 104.0, true))
    }

    fn param_row(&self, k: &Kit, ctx: &Ctx, cfg: &crate::workspace::InstanceCfg, pd: &ParamDef, images: &mut Vec<String>) -> Node {
        let id = &cfg.id;
        let name = &pd.name;
        let key = format!("p:{id}:{name}");
        let rk = format!("pr/{id}/{name}");
        let val = Self::param_value(cfg, pd);
        let (label, unit) = split_unit(&pd.label);
        let control = match pd.ty {
            ParamType::Bool => k.toggle(&format!("tg:{id}:{name}"), val.truthy(), format!("tog:{id}|{name}")),
            ParamType::Number | ParamType::Duration => {
                let (min, max, _step, cur) = self.slider_spec(ctx, &key).unwrap_or((0.0, 100.0, 1.0, 0.0));
                let frac = if max > min { ((cur - min) / (max - min)) as f32 } else { 0.0 };
                Node::new(format!("{rk}/sc")).row().align(taffy::AlignItems::CENTER).gap(12.0).child(k.slider(&format!("sl:{key}"), frac, 190.0, format!("sl:{key}"))).child(Node::new(format!("{rk}/vw")).w(48.0).child(k.txt(format!("{rk}/v"), &with_unit(cur, unit), 12.5, k.c("text"))))
            }
            ParamType::Color => {
                let col = Self::resolve_color(ctx, &val);
                let f = self.focus.as_ref().filter(|f| f.key == format!("hx:{key}")).map(|f| (f.caret, self.caret_on));
                Node::new(format!("{rk}/cc"))
                    .row()
                    .align(taffy::AlignItems::CENTER)
                    .gap(8.0)
                    .child(k.swatch(&format!("{rk}/sw"), col, 28.0, Some(format!("cp:{key}")), matches!(&self.open, Some(Open::Color(o)) if *o == key)))
                    .child(k.input(&format!("hx:{key}"), &self.input_text(ctx, &format!("hx:{key}")), "#rrggbb", f, 104.0, true))
            }
            ParamType::Font | ParamType::Enum => self.dropdown_for(k, ctx, &key),
            ParamType::Str => {
                let ik = format!("n:{id}:{name}");
                let f = self.focus.as_ref().filter(|f| f.key == ik).map(|f| (f.caret, self.caret_on));
                k.input(&ik, &self.input_text(ctx, &ik), "", f, CONTROL_W, false)
            }
            ParamType::Path | ParamType::File => {
                let ik = format!("n:{id}:{name}");
                let pick = if pd.ty == ParamType::File { "file" } else { "folder" };
                let f = self.focus.as_ref().filter(|f| f.key == ik).map(|f| (f.caret, self.caret_on));
                let has_list = self.def_of(ctx, &cfg.widget).is_some_and(|d| d.params.iter().any(|p| p.ty == ParamType::Shortcuts));
                let empty = if has_list { "no folder: use the list below" } else if pd.ty == ParamType::File { "no file chosen" } else { "no folder chosen" };
                Node::new(format!("{rk}/pc"))
                    .col()
                    .gap(8.0)
                    .child(k.input(&ik, &self.input_text(ctx, &ik), empty, f, CONTROL_W, false))
                    .child(Node::new(format!("{rk}/pb")).row().gap(8.0).child(k.btn(&format!("{rk}/browse"), Some("folder"), "Browse…", format!("{pick}:{id}|{name}"), Btn::Outline)).child(k.btn(&format!("{rk}/clear"), None, "Clear", format!("clear:{id}|{name}"), Btn::Outline)))
            }
            ParamType::Shortcuts => return self.shortcuts_editor(k, ctx, cfg, pd, images),
        };
        k.row(&rk, label, &pd.help, control)
    }

    fn shortcuts_editor(&self, k: &Kit, ctx: &Ctx, cfg: &crate::workspace::InstanceCfg, pd: &ParamDef, images: &mut Vec<String>) -> Node {
        let id = &cfg.id;
        let items = cfg.items();
        let mirrored = !cfg.folder().is_empty();
        let mut col = Node::new(format!("sc/{id}")).col().gap(8.0).pad_xy(20.0, 14.0);
        col = col.child(
            Node::new(format!("sc/{id}/h"))
                .row()
                .align(taffy::AlignItems::CENTER)
                .child(k.bold(format!("sc/{id}/ht"), &pd.label, 13.5, k.c("text")).grow_text())
                .child(k.btn(&format!("sc/{id}/add"), Some("add"), "Add shortcut…", format!("scadd:{id}"), Btn::Primary)),
        );
        if mirrored {
            col = col.child(k.txt(format!("sc/{id}/m"), "A folder is mirrored, so this list is not shown. Clear the folder to use it.", 12.0, k.c("text-dim")).wrap_text());
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
            img.kind = Kind::Image(ui::ImageSpec { id: iid, w: 32.0, h: 32.0, ..Default::default() });
            let fields = Node::new(format!("sc/{id}/{i}/f")).row().wrap().grow(1.0).min_w(0.0).gap(6.0).child(k.input(&nk, &self.input_text(ctx, &nk), "Name", fnm, 170.0, false)).child(k.input(&tk, &self.input_text(ctx, &tk), "Path or command", ftg, 230.0, true));
            col = col.child(
                Node::new(format!("sc/{id}/{i}"))
                    .row()
                    .align(taffy::AlignItems::CENTER)
                    .gap(10.0)
                    .pad_xy(10.0, 8.0)
                    .radius(10.0)
                    .fill(k.ink(0.03))
                    .border(1.0, k.line())
                    .enter(200, 6.0, (i as u32).min(8) * 20)
                    .child(img)
                    .child(fields)
                    .child(k.icon_btn(&format!("sc/{id}/{i}/bf"), "folder", format!("scbrowse:{id}|{i}"), k.c("text-dim"), k.ink(0.08)))
                    .child(k.icon_btn(&format!("sc/{id}/{i}/x"), "delete", format!("scdel:{id}|{i}"), k.c("danger"), k.c("danger").with_alpha(0.15))),
            );
        }
        col
    }

    fn page_appearance(&self, k: &Kit, ctx: &Ctx, size: (f32, f32)) -> Node {
        let cw = content_w(size, 1040.0);
        let side = cw >= 860.0;
        let main_w = if side { cw - PREVIEW_W - 28.0 } else { cw };
        let f = |key: &str| matches!(&self.open, Some(Open::Dropdown(o)) if o == key);
        let glyph_axis = ctx.lib.glyphs(&ctx.ws.theme.glyphs);
        let fam = glyph_axis.tokens.get("font-glyph").map(|v| v.to_string()).unwrap_or_default();
        let mut gl = Node::new("ap/glyphs").row().gap(14.0).align(taffy::AlignItems::CENTER).pad_xy(0.0, 3.0);
        for (i, g) in ["gear", "close", "folder", "edit", "check"].iter().enumerate() {
            let ch = glyph_axis.tokens.get(&format!("glyph-{g}")).map(|v| v.to_string()).unwrap_or_default();
            let fam = fam.clone();
            gl = gl.child(Node::text(format!("ap/gl/{i}"), ch, 15.0, k.c("text-dim")).with_text(|t| t.family = fam));
        }
        let typo = k.group(
            "ap/ty",
            "Typography & icons",
            vec![
                k.row("ap/fonts", "Font set", "Body, display and monospace faces", k.dropdown("th:fonts", &ctx.ws.theme.fonts, CONTROL_W, f("th:fonts"))),
                k.row_with("ap/glyphs-row", k.bold("ap/glyphs-row/lt".into(), "Glyph set", 13.5, k.c("text")), "", Some(gl), k.dropdown("th:glyphs", &ctx.ws.theme.glyphs, CONTROL_W, f("th:glyphs"))),
                k.row_with("ap/pack", k.bold("ap/pack/lt".into(), "App icon pack", 13.5, k.c("text")), "Replaces icons in Drawer and Icon List.", Some(k.link("ap/pack/open", "Open icon packs folder", "openpacks".into())), k.dropdown("th:pack", &ctx.ws.theme.icon_pack, CONTROL_W, f("th:pack"))),
            ],
        );
        let mut surface: Vec<Node> = style_schema().iter().map(|pd| self.style_row(k, ctx, &Scope::Global, pd)).collect();
        if !ctx.ws.style.is_empty() {
            surface.push(k.row("ap/resetall", "Reset all", "Back to the defaults and each palette's own colours", k.btn("ap/resetall/b", None, "Reset all", "syreset:*|*".into(), Btn::Outline)));
        }
        let palettes = Node::new("ap/pal/g").col().gap(12.0).child(k.heading("ap/pal/t".into(), "Palette")).child(self.palette_grid(k, ctx, main_w, "ap/pal", true));
        let main = Node::new("ap/main").col().grow(1.0).min_w(0.0).gap(30.0).child(palettes).child(typo).child(k.group("ap/sf", "Surface", surface));
        let mut cols = Node::new("ap/cols").row().gap(28.0).align(taffy::AlignItems::FLEX_START).child(main);
        if side {
            cols = cols.child(self.theme_preview(k, ctx));
        }
        let body = Node::new("ap").col().gap(26.0).child(k.page_head("ap/head", Page::Appearance.heading(), Page::Appearance.subtitle(), None)).child(cols);
        self.page("ap/scroll", cw, body)
    }

    /// A card per palette showing its colours, and one leading to the Plugins page.
    fn palette_grid(&self, k: &Kit, ctx: &Ctx, w: f32, key: &str, more: bool) -> Node {
        let (cols, pw) = columns(w, 150.0, 12.0, 4);
        let mut cards: Vec<Node> = ctx.lib.palettes.iter().enumerate().map(|(i, a)| self.palette_card(k, ctx, a, pw, key, i)).collect();
        if more {
            cards.push(
                Node::new(format!("{key}/more"))
                    .col()
                    .w(pw)
                    .h(118.0)
                    .center()
                    .gap(8.0)
                    .radius(12.0)
                    .hover_fill(k.ink(0.04))
                    .ease(150)
                    .on("nav:plugins")
                    .child(dashed(format!("{key}/more/d"), 12.0, k.ink(0.25)))
                    .child(k.glyph(format!("{key}/more/g"), "add", 15.0, k.c("text-dim")))
                    .child(k.txt(format!("{key}/more/t"), "Get more from plugins", 12.5, k.c("text-dim"))),
            );
        }
        grid(key, cols, 12.0, cards)
    }

    fn palette_card(&self, k: &Kit, ctx: &Ctx, a: &Axis, w: f32, prefix: &str, i: usize) -> Node {
        let on = a.name == ctx.ws.theme.palette;
        let tok = |n: &str, or: Color| a.tokens.get(n).map(|v| v.to_string()).and_then(|s| Color::parse(&s)).unwrap_or(or);
        let (surface, text, dim, accent) = (tok("surface", k.bg()).with_alpha(1.0), tok("text", k.c("text")), tok("text-dim", k.c("text-dim")), tok("accent", k.c("accent")));
        let key = format!("{prefix}/{}", a.name);
        let bar = (w - 32.0).max(20.0);
        let top = Node::new(format!("{key}/top"))
            .col()
            .h(76.0)
            .pad_xy(14.0, 13.0)
            .gap(14.0)
            .kids(half_round(&format!("{key}/top"), surface, 10.0, true))
            .child(Node::new(format!("{key}/r1")).row().align(taffy::AlignItems::CENTER).child(k.bold(format!("{key}/tm"), "01:30", 19.0, text).grow_text()).child(Node::new(format!("{key}/dot")).wh(8.0, 8.0).radius(4.0).fill(accent)))
            .child(Node::new(format!("{key}/bars")).row().gap(6.0).child(Node::new(format!("{key}/b1")).wh((bar * 0.5).floor(), 4.0).radius(2.0).fill(accent)).child(Node::new(format!("{key}/b2")).wh((bar * 0.25).floor(), 4.0).radius(2.0).fill(dim)));
        let foot = Node::new(format!("{key}/ft"))
            .row()
            .h(36.0)
            .pad_xy(14.0, 0.0)
            .align(taffy::AlignItems::CENTER)
            .kids(half_round(&format!("{key}/ft"), k.solid(0.05), 10.0, false))
            .child(k.bold(format!("{key}/n"), &a.name, 13.0, k.c("text")).grow_text())
            .child(k.glyph(format!("{key}/ck"), "check", 12.0, k.c("accent").with_alpha(if on { 1.0 } else { 0.0 })));
        let mut card = Node::new(key.clone())
            .col()
            .w(w)
            .pad(2.0)
            .radius(12.0)
            .border(if on { 2.0 } else { 1.0 }, if on { k.c("accent") } else { k.line() })
            .hover_fill(k.ink(0.05))
            .ease(150)
            .on(format!("pick:th:palette|{}", a.name))
            .enter(240, 8.0, (i as u32).min(10) * 40)
            .child(top)
            .child(foot);
        if on {
            card = card.shadow(16.0, 0.0, k.c("accent").with_alpha(0.28));
        }
        card
    }

    /// Two sample widgets in the global Theme, beside the Appearance settings.
    fn theme_preview(&self, k: &Kit, ctx: &Ctx) -> Node {
        let t = ctx.theme;
        let fam = t.str("font-body");
        let tx = |key: &str, s: &str, size: f32, col: Color, weight: u16| {
            let fam = fam.clone();
            Node::text(key, s, size, col).with_text(|x| {
                x.family = fam;
                x.weight = weight;
            })
        };
        let card = |key: &str| {
            let n = Node::new(key).col().pad(16.0).gap(6.0).radius((t.num("radius-lg") * 0.6).clamp(0.0, 18.0)).fill(t.color("surface"));
            if t.flag("outlines") { n.border(1.0, t.color("border")) } else { n }
        };
        const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
        const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
        let now = crate::data::now_local();
        let date = format!("{}, {} {}", DAYS[now.dow as usize % 7], now.day, MONTHS[(now.month as usize).clamp(1, 12) - 1]);
        let clock = card("ap/pv/clock").child(tx("ap/pv/date", &date, 11.5, t.color("text-dim"), 400)).child(tx("ap/pv/time", &format!("{:02}:{:02}", now.hour, now.minute), 38.0, t.color("text"), 700));
        let gauge = |key: &str, label: &str, pct: f32| {
            Node::new(key)
                .col()
                .gap(7.0)
                .child(Node::new(format!("{key}/r")).row().child(tx(&format!("{key}/l"), label, 11.5, t.color("text-dim"), 400).grow_text()).child(tx(&format!("{key}/v"), &format!("{pct:.0}%"), 11.5, t.color("text"), 600)))
                .child(Node::new(format!("{key}/tr")).row().h(5.0).radius(3.0).fill(t.color("track")).child(Node::new(format!("{key}/f")).w_pct(pct).radius(3.0).fill(t.color("accent"))))
        };
        let sys = card("ap/pv/sys").gap(14.0).child(gauge("ap/pv/cpu", "CPU", 34.0)).child(gauge("ap/pv/mem", "Memory", 61.0));
        let stage = Node::new("ap/pv/stage").col().gap(12.0).pad(16.0).radius(14.0).fill(k.ink(0.02)).border(1.0, k.line()).clip().child(dots("ap/pv/dots".into(), k.ink(0.1))).child(clock).child(sys);
        Node::new("ap/pv")
            .col()
            .w(PREVIEW_W)
            .no_shrink()
            .gap(10.0)
            .child(k.bold("ap/pv/t".into(), "Preview", 13.0, k.c("text-dim")))
            .child(stage)
            .child(k.txt("ap/pv/cap".into(), "Sample widgets. Your desktop updates as you change settings.", 12.0, k.c("text-dim")).wrap_text())
    }

    fn page_plugins(&self, k: &Kit, ctx: &Ctx, size: (f32, f32)) -> Node {
        let accent = k.c("accent");
        let actions = Node::new("pl/acts").row().gap(8.0).child(k.btn("pl/folder", Some("folder"), "Open plugins folder", "pfolder".into(), Btn::Outline)).child(k.btn("pl/install", Some("download"), "Install from file…", "pinstall".into(), Btn::Primary));
        let hot = self.drop_hover;
        let drop = Node::new("pl/drop")
            .row()
            .gap(16.0)
            .align(taffy::AlignItems::CENTER)
            .pad_xy(20.0, 18.0)
            .radius(14.0)
            .fill(if hot { accent.with_alpha(0.1) } else { k.ink(0.015) })
            .ease(150)
            .clip()
            .child(dots("pl/drop/dots".into(), k.ink(0.08)))
            .child(dashed("pl/drop/d".into(), 14.0, if hot { accent } else { k.ink(0.22) }))
            .child(Node::new("pl/drop/ic").wh(44.0, 44.0).no_shrink().radius(11.0).center().fill(accent.with_alpha(0.14)).child(k.glyph("pl/drop/ic/g".into(), "upload", 16.0, accent)))
            .child(
                Node::new("pl/drop/tx")
                    .col()
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(3.0)
                    .child(k.bold("pl/drop/t".into(), if hot { "Drop to install" } else { "Drop a .wfplugin file here to install it" }, 14.0, k.c("text")))
                    .child(k.txt("pl/drop/s".into(), "A plugin with code asks first and names what it can read and reach. Only install plugins from people you trust.", 12.5, k.c("text-dim")).wrap_text()),
            );
        let mut body = Node::new("pl").col().gap(24.0).child(k.page_head("pl/head", Page::Plugins.heading(), Page::Plugins.subtitle(), Some(actions))).child(drop);
        if !ctx.plugin_note.is_empty() {
            let bad = ctx.plugin_note.starts_with("Could not");
            body = body.child(k.txt("pl/note".into(), ctx.plugin_note, 13.0, if bad { k.c("danger") } else { accent }).wrap_text());
        }
        let mut list = Node::new("pl/list").col().gap(14.0).child(k.heading("pl/s1".into(), &format!("Installed · {}", ctx.plugins.len())));
        if ctx.plugins.is_empty() {
            list = list.child(k.txt("pl/none".into(), "No plugins yet. Drop a .wfplugin file on this window, or put a plugin's folder in Wayfinder\\plugins.", 13.0, k.c("text-dim")).wrap_text());
        }
        for (i, r) in ctx.plugins.iter().enumerate() {
            list = list.child(self.plugin_card(k, ctx, r, i));
        }
        self.page("pl/scroll", content_w(size, 860.0), body.child(list))
    }

    fn plugin_card(&self, k: &Kit, ctx: &Ctx, r: &PluginRow, i: usize) -> Node {
        let key = format!("pl/p/{}", r.id);
        let (accent, dim) = (k.c("accent"), k.c("text-dim"));
        let confirm = self.confirm_del.as_deref() == Some(format!("plugin/{}", r.id).as_str());
        let mut name = Node::new(format!("{key}/nm")).row().wrap().gap(10.0).align(taffy::AlignItems::CENTER).child(k.bold(format!("{key}/n"), &r.name, 16.0, k.c("text")).with_text(|t| t.weight = 700));
        if !r.version.is_empty() {
            name = name.child(k.tag(format!("{key}/v"), &format!("v{}", r.version), true));
        }
        if !r.author.is_empty() {
            name = name.child(k.txt(format!("{key}/by"), &format!("by {}", r.author), 12.5, dim));
        }
        let mut info = Node::new(format!("{key}/info")).col().grow(1.0).min_w(0.0).gap(6.0).child(name);
        if !r.description.is_empty() {
            info = info.child(k.txt(format!("{key}/d"), &r.description, 13.0, dim).wrap_text());
        }
        let switch = Node::new(format!("{key}/sw"))
            .row()
            .no_shrink()
            .gap(10.0)
            .align(taffy::AlignItems::CENTER)
            .child(k.bold(format!("{key}/st"), if r.enabled { "On" } else { "Off" }, 13.0, if r.enabled { accent } else { dim }))
            .child(k.toggle(&format!("{key}/on"), r.enabled, format!("pon:{}", r.id)));
        let head = Node::new(format!("{key}/h")).row().gap(16.0).align(taffy::AlignItems::FLEX_START).pad(20.0).child(k.tile(format!("{key}/ic"), "plugin", 44.0, accent)).child(info).child(switch);
        let hr = |n: &str| Node::new(format!("{key}/hr{n}")).h(1.0).no_shrink().fill(k.line());
        let mut card = Node::new(key.clone()).col().radius(14.0).fill(k.panel()).border(1.0, if confirm { k.c("danger") } else { k.line() }).clip().ease(150).enter(220, 6.0, (i as u32).min(8) * 30).child(head);

        let c = &r.contents;
        let mut adds = Node::new(format!("{key}/adds")).row();
        let mut any = false;
        for (kind, items) in [("widget", &c.widgets), ("palette", &c.palettes), ("font set", &c.fonts), ("glyph set", &c.glyphs), ("icon pack", &c.icon_packs)] {
            if items.is_empty() {
                continue;
            }
            if any {
                adds = adds.child(Node::new(format!("{key}/vr/{kind}")).w(1.0).no_shrink().fill(k.line()));
            }
            any = true;
            let mut chips = Node::new(format!("{key}/a/{kind}/c")).row().wrap().gap(8.0);
            for it in items {
                let ck = format!("{key}/a/{kind}/{it}");
                let mut chip = Node::new(ck.clone()).row().h(32.0).pad_xy(10.0, 0.0).gap(8.0).align(taffy::AlignItems::CENTER).radius(8.0).fill(k.ink(0.03)).border(1.0, k.line());
                let label = match kind {
                    "widget" => {
                        let m = self.def_of(ctx, it);
                        chip = chip.child(k.glyph(format!("{ck}/g"), &k.widget_glyph(m), 12.0, accent));
                        m.map_or(it.clone(), |m| m.name.clone())
                    }
                    "palette" => {
                        let tok = |n: &str| ctx.lib.palettes.iter().find(|a| a.name == *it).and_then(|a| a.tokens.get(n)).map(|v| v.to_string()).and_then(|s| Color::parse(&s));
                        let dot = Node::new(format!("{ck}/i")).wh(7.0, 7.0).radius(3.5).fill(tok("accent").unwrap_or(accent));
                        chip = chip.child(Node::new(format!("{ck}/dot")).wh(16.0, 16.0).no_shrink().radius(8.0).center().fill(tok("surface").unwrap_or(k.bg()).with_alpha(1.0)).border(1.0, k.line()).child(dot));
                        it.clone()
                    }
                    _ => it.clone(),
                };
                chips = chips.child(chip.child(k.txt(format!("{ck}/t"), &label, 13.0, k.c("text"))));
            }
            let n = items.len();
            adds = adds.child(Node::new(format!("{key}/a/{kind}")).col().grow(n as f32).min_w(0.0).gap(10.0).pad_xy(20.0, 16.0).child(k.bold(format!("{key}/a/{kind}/t"), &format!("Adds {n} {kind}{}", if n == 1 { "" } else { "s" }), 12.5, dim)).child(chips));
        }
        if any {
            card = card.child(hr("a")).child(adds);
        }

        if !r.code.is_empty() {
            let mut code = Node::new(format!("{key}/code")).col().gap(2.0).pad_xy(20.0, 14.0).child(k.bold(format!("{key}/code/t"), "Runs sandboxed code", 12.5, dim));
            for c in &r.code {
                let ck = format!("{key}/code/{}", c.source);
                let chip = |n: &str, g: &str, s: &str| {
                    Node::new(format!("{ck}/{n}")).row().h(28.0).pad_xy(9.0, 0.0).gap(7.0).align(taffy::AlignItems::CENTER).radius(7.0).fill(k.ink(0.04)).child(k.glyph(format!("{ck}/{n}/g"), g, 11.0, dim)).child(k.txt(format!("{ck}/{n}/t"), s, 12.0, k.c("text")))
                };
                let mut chips = Node::new(format!("{ck}/chips")).row().wrap().grow(1.0).min_w(0.0).gap(6.0);
                chips = chips.child(if c.net.is_empty() { chip("net", "blocked", "No network") } else { chip("net", "open", &format!("Reaches {}", c.net.join(", "))) });
                if !c.reads.is_empty() {
                    chips = chips.child(chip("rd", "folder", &format!("Reads {}", c.reads.join(", "))));
                }
                if !c.opens.is_empty() {
                    chips = chips.child(chip("op", "launch", &format!("Opens {}", c.opens.join(", "))));
                }
                let (label, col) = code_status(k, r.enabled, c);
                let status = Node::new(format!("{ck}/s"))
                    .row()
                    .no_shrink()
                    .max_w(240.0)
                    .gap(7.0)
                    .align(taffy::AlignItems::CENTER)
                    .child(Node::new(format!("{ck}/s/d")).wh(7.0, 7.0).no_shrink().radius(3.5).fill(col))
                    .child(k.txt(format!("{ck}/s/t"), &label, 12.5, col).wrap_text());
                code = code.child(
                    Node::new(ck.clone())
                        .row()
                        .min_h(42.0)
                        .gap(14.0)
                        .align(taffy::AlignItems::CENTER)
                        .child(Node::new(format!("{ck}/nw")).w(110.0).no_shrink().child(k.mono(format!("{ck}/n"), &c.source, 13.0, k.c("text")).with_text(|t| t.weight = 600)))
                        .child(chips)
                        .child(status),
                );
            }
            card = card.child(hr("c")).child(code);
        }

        if !r.notes.is_empty() || !r.problems.is_empty() {
            let mut msgs = Node::new(format!("{key}/msgs")).col().gap(6.0).pad_xy(20.0, 14.0);
            for (j, n) in r.notes.iter().enumerate() {
                msgs = msgs.child(k.txt(format!("{key}/note/{j}"), n, 12.5, dim).wrap_text());
            }
            for (j, e) in r.problems.iter().enumerate() {
                msgs = msgs.child(Node::new(format!("{key}/err/{j}")).row().gap(8.0).child(k.glyph(format!("{key}/err/{j}/g"), "warning", 12.0, k.c("danger"))).child(k.txt(format!("{key}/err/{j}/t"), e, 12.5, k.c("danger")).wrap_text().grow_text()));
            }
            card = card.child(hr("m")).child(msgs);
        }

        let left = if confirm {
            let orphans = r.orphans(ctx.ws);
            let what = if orphans.is_empty() { "Nothing on your desktop uses it.".to_string() } else { format!("This also removes from your desktop: {}.", orphans.join(", ")) };
            k.txt(format!("{key}/orph"), &what, 12.5, k.c("danger")).wrap_text()
        } else {
            k.mono(format!("{key}/dir"), &format!("plugins\\{}", r.id), 12.0, dim)
        };
        let remove = if confirm {
            k.btn(&format!("{key}/rm"), Some("delete"), "Really uninstall?", format!("prm:{}", r.id), Btn::DangerFill)
        } else {
            k.btn(&format!("{key}/rm"), Some("delete"), "Uninstall", format!("prm:{}", r.id), Btn::Danger)
        };
        let foot = Node::new(format!("{key}/f")).row().gap(12.0).align(taffy::AlignItems::CENTER).pad_xy(20.0, 12.0).child(Node::new(format!("{key}/fl")).col().grow(1.0).min_w(0.0).child(left)).child(remove);
        card.child(hr("f")).child(foot)
    }

    fn page_general(&self, k: &Kit, ctx: &Ctx, size: (f32, f32)) -> Node {
        let f = |key: &str| matches!(&self.open, Some(Open::Dropdown(o)) if o == key);
        let ws = ctx.ws;
        let mut gfx = Vec::new();
        if ctx.gpu_info.split(" / ").nth(2) == Some("Cpu") && ws.gpu != "software" {
            let warn = k.warn();
            gfx.push(
                Node::new("gn/warn")
                    .row()
                    .gap(14.0)
                    .align(taffy::AlignItems::CENTER)
                    .pad_xy(20.0, 14.0)
                    .kids(half_round("gn/warn", k.solid(0.03).lerp(warn, 0.08), 11.0, true))
                    .child(k.glyph("gn/warn/g".into(), "warning", 18.0, warn))
                    .child(Node::new("gn/warn/tx").col().grow(1.0).min_w(0.0).gap(2.0).child(k.bold("gn/warn/t".into(), "Widgets are rendering on the CPU", 13.5, warn)).child(k.txt("gn/warn/s".into(), "Your integrated GPU is plenty for widgets and keeps the desktop at near-zero cost.", 12.0, warn.mul_alpha(0.8)).wrap_text()))
                    .child(k.btn("gn/warn/b", None, "Use integrated GPU", "gpufix".into(), Btn::Warn)),
            );
        }
        let help = if ws.gpu == "high" { "The dedicated GPU keeps a CPU core busy on some AMD drivers. Applies after a restart." } else { "Applies after a restart." };
        let mut adapter = Node::new("gn/gpu/c").row().gap(10.0).align(taffy::AlignItems::CENTER).child(k.dropdown("gpu", &self.dropdown_label(ctx, "gpu"), CONTROL_W, f("gpu")));
        if self.gpu_picked {
            adapter = adapter.child(k.btn("gn/restart", Some("refresh"), "Restart", "restart".into(), Btn::Primary));
        }
        gfx.push(k.row("gn/gpu", "Adapter", help, adapter));
        let chips = Node::new("gn/info/c").row().wrap().gap(6.0).justify(taffy::JustifyContent::FLEX_END).kids(gpu_parts(ctx.gpu_info).iter().enumerate().map(|(i, p)| k.tag(format!("gn/info/{i}"), p, true)));
        gfx.push(k.row("gn/info", "In use now", "", chips));
        let grid = self.slider_spec(ctx, "grid").unwrap_or((0.0, 32.0, 4.0, 8.0));
        let snap = Node::new("gn/grid/c").row().align(taffy::AlignItems::CENTER).gap(12.0).child(k.slider("sl:grid", (grid.3 / grid.1) as f32, 190.0, "sl:grid".into())).child(Node::new("gn/grid/vw").w(48.0).child(k.txt("gn/grid/v".into(), &with_unit(grid.3, "px"), 12.5, k.c("text"))));
        let behaviour = vec![
            k.row("gn/auto", "Start with Windows", "Launch quietly into the tray when you sign in.", k.toggle("tg:autostart", ws.autostart, "autostart:toggle".into())),
            flag_row(k, ctx, "gn/hd", Flag::HeaderDrag),
            k.row("gn/grid", "Snap grid", "0 turns snapping off. Hold Shift to ignore it while dragging.", snap),
            k.row("gn/hk", "Edit layout shortcut", "Works from anywhere in Windows.", k.keys("gn/hk/k", EDIT_KEYS)),
        ];
        let folder = Node::new("gn/files/c").row().gap(8.0).child(k.btn("gn/open", Some("folder"), "Open folder", "openfolder".into(), Btn::Outline)).child(k.btn("gn/reload", Some("refresh"), "Reload all", "reload".into(), Btn::Outline));
        let files = vec![k.row("gn/files", "Your widgets folder", "Drop .toml widget definitions here — they reload as you save.", folder), self.plugin_files_row(k, ctx)];
        let body = Node::new("gn")
            .col()
            .gap(30.0)
            .child(k.page_head("gn/head", Page::General.heading(), Page::General.subtitle(), None))
            .child(k.group("gn/gfx", "Graphics", gfx))
            .child(k.group("gn/beh", "Behaviour", behaviour))
            .child(k.group("gn/fs", "Files", files));
        self.page("gn/scroll", content_w(size, 780.0), body)
    }

    /// Which app a double-clicked `.wfplugin` opens, and a way to make it this one.
    fn plugin_files_row(&self, k: &Kit, ctx: &Ctx) -> Node {
        let help = match ctx.plugin_files {
            FileOwner::Me => "Double-click one in Explorer to install it.".to_string(),
            FileOwner::Other(p) => format!("Double-clicking one opens {}", p.display()),
            FileOwner::Nobody => "Nothing installs them on a double-click yet.".to_string(),
        };
        let control = match ctx.plugin_files {
            FileOwner::Me => Node::new("gn/pf/t").row().gap(8.0).align(taffy::AlignItems::CENTER).child(k.glyph("gn/pf/t/g".into(), "check", 12.0, k.c("accent"))).child(k.bold("gn/pf/t/t".into(), "Opens with Wayfinder", 13.0, k.c("accent"))),
            _ => k.btn("gn/pf/b", None, "Use this app", "claimfiles".into(), Btn::Outline),
        };
        k.row("gn/pf", ".wfplugin files", &help, control)
    }

    /// The log lines the Log page's level and search let through.
    fn log_rows<'a>(&self, ctx: &Ctx<'a>) -> Vec<&'a LogLine> {
        let q = self.query("q:log").to_lowercase();
        ctx.log.iter().filter(|l| self.log_level.is_none_or(|v| l.level == v)).filter(|l| q.is_empty() || l.text.to_lowercase().contains(&q) || l.source.to_lowercase().contains(&q)).collect()
    }

    fn page_log(&self, k: &Kit, ctx: &Ctx, size: (f32, f32)) -> Node {
        let actions = Node::new("lg/acts").row().gap(8.0).child(k.btn("lg/copy", Some("copy"), "Copy", "logcopy".into(), Btn::Outline)).child(k.btn("lg/open", Some("open"), "Open log file", "openlog".into(), Btn::Outline));
        let mut seg = Node::new("lg/lv").row().gap(2.0).pad(4.0).radius(12.0).fill(k.panel()).border(1.0, k.line());
        let levels = std::iter::once((None, "all", "All")).chain(Level::ALL.into_iter().map(|l| (Some(l), l.id(), l.plural())));
        for (lvl, id, label) in levels {
            let on = self.log_level == lvl;
            let n = ctx.log.iter().filter(|l| lvl.is_none_or(|v| l.level == v)).count();
            seg = seg.child(
                Node::new(format!("lg/lv/{id}"))
                    .row()
                    .h(32.0)
                    .pad_xy(12.0, 0.0)
                    .gap(8.0)
                    .align(taffy::AlignItems::CENTER)
                    .radius(9.0)
                    .fill(k.ink(if on { 0.08 } else { 0.0 }))
                    .hover_fill(k.ink(if on { 0.1 } else { 0.05 }))
                    .ease(140)
                    .on(format!("lvl:{id}"))
                    .child(Node::new(format!("lg/lv/{id}/d")).wh(7.0, 7.0).radius(3.5).fill(lvl.map_or(k.c("text-dim"), |l| level_color(k, l))))
                    .child(k.txt(format!("lg/lv/{id}/t"), label, 13.0, if on { k.c("text") } else { k.c("text-dim") }).with_text(|t| t.weight = if on { 600 } else { 400 }))
                    .child(k.txt(format!("lg/lv/{id}/n"), &n.to_string(), 11.5, k.c("text-dim"))),
            );
        }
        let fq = self.focus.as_ref().filter(|f| f.key == "q:log").map(|f| (f.caret, self.caret_on));
        let tools = Node::new("lg/tools").row().wrap().gap(12.0).align(taffy::AlignItems::CENTER).child(seg).child(Node::new("lg/tools/sp").grow(1.0)).child(k.input("q:log", &self.input_text(ctx, "q:log"), "Search the log…", fq, 260.0, false));
        let cell = |key: String, w: f32, n: Node| Node::new(key).w(w).h(22.0).no_shrink().align(taffy::AlignItems::CENTER).child(n);
        let head = Node::new("lg/th")
            .row()
            .gap(16.0)
            .pad_xy(20.0, 12.0)
            .child(cell("lg/th/tm".into(), 96.0, k.bold("lg/th/tm/t".into(), "Time", 12.5, k.c("text-dim"))))
            .child(cell("lg/th/lv".into(), 104.0, k.bold("lg/th/lv/t".into(), "Level", 12.5, k.c("text-dim"))))
            .child(cell("lg/th/src".into(), 96.0, k.bold("lg/th/src/t".into(), "Source", 12.5, k.c("text-dim"))))
            .child(Node::new("lg/th/m").h(22.0).grow(1.0).align(taffy::AlignItems::CENTER).child(k.bold("lg/th/m/t".into(), "Message", 12.5, k.c("text-dim"))));
        let mut table = Node::new("lg/table").col().radius(12.0).fill(k.panel()).border(1.0, k.line()).clip().child(head);
        let rows = self.log_rows(ctx);
        // ponytail: the last 200 match; the log file keeps the rest
        let skip = rows.len().saturating_sub(200);
        for (i, l) in rows.iter().enumerate().skip(skip) {
            let col = level_color(k, l.level);
            let key = format!("lg/r{i}");
            let mut msg = Node::new(format!("{key}/m")).col().grow(1.0).min_w(0.0).gap(6.0).pad_xy(0.0, 3.0).child(k.mono(format!("{key}/m/t"), &l.text, 12.5, k.c("text")).wrap_text());
            if l.level == Level::Warning && l.source == "gpu" {
                msg = msg.child(k.link(&format!("{key}/go"), "Change adapter in General →", "nav:general".into()));
            }
            let badge = Node::new(format!("{key}/b")).row().h(22.0).pad_xy(8.0, 0.0).gap(6.0).align(taffy::AlignItems::CENTER).radius(6.0).fill(col.with_alpha(0.14)).child(Node::new(format!("{key}/b/d")).wh(6.0, 6.0).radius(3.0).fill(col)).child(k.bold(format!("{key}/b/t"), l.level.label(), 11.5, col));
            let fill = if l.level == Level::Info { TRANSPARENT } else { col.with_alpha(0.05) };
            table = table.child(Node::new(format!("{key}/hr")).h(1.0).no_shrink().fill(k.line())).child(
                Node::new(key.clone())
                    .row()
                    .gap(16.0)
                    .align(taffy::AlignItems::FLEX_START)
                    .pad_xy(20.0, 11.0)
                    .fill(fill)
                    .child(cell(format!("{key}/tm"), 96.0, k.mono(format!("{key}/tm/t"), &l.time, 12.5, k.c("text-dim"))))
                    .child(cell(format!("{key}/lv"), 104.0, badge))
                    .child(cell(format!("{key}/src"), 96.0, k.mono(format!("{key}/src/t"), &l.source, 12.5, k.c("text"))))
                    .child(msg),
            );
        }
        if rows.is_empty() {
            let what = if ctx.log.is_empty() { "Nothing yet." } else { "Nothing matches." };
            table = table.child(Node::new("lg/none/hr").h(1.0).fill(k.line())).child(Node::new("lg/none").pad_xy(20.0, 16.0).child(k.txt("lg/none/t".into(), what, 13.0, k.c("text-dim"))));
        }
        let body = Node::new("lg")
            .col()
            .gap(20.0)
            .child(k.page_head("lg/head", Page::Log.heading(), Page::Log.subtitle(), Some(actions)))
            .child(tools)
            .child(table)
            .child(k.txt("lg/foot".into(), "Showing this session. Older entries are in the log file.", 12.0, k.c("text-dim")));
        self.page("lg/scroll", content_w(size, 1040.0), body)
    }

    /// The first-run setup, filling the window until it is finished or skipped.
    fn setup(&self, k: &Kit, ctx: &Ctx, size: (f32, f32), images: &mut Vec<String>) -> Node {
        let step = self.step.min(SETUP_STEPS - 1);
        let (accent, text, dim) = (k.c("accent"), k.c("text"), k.c("text-dim"));
        let title = |key: &str, s: &str| k.bold(key.into(), s, 30.0, text).with_text(|t| t.weight = 700);
        let sub = |key: &str, s: &str| k.txt(key.into(), s, 15.0, dim).wrap_text().with_text(|t| t.align = crate::text::TextAlign::Center).max_w(500.0);
        let wide = (size.0 - 2.0 * PAGE_PAD).min(840.0);
        let content = match step {
            0 => Node::new("ob/0")
                .col()
                .align(taffy::AlignItems::CENTER)
                .gap(18.0)
                .child(k.logo("ob/logo", 88.0).shadow(36.0, 6.0, accent.with_alpha(0.35)))
                .child(Node::new("ob/0/sp").h(12.0))
                .child(k.bold("ob/0/t".into(), "Wayfinder", 46.0, text).with_text(|t| t.weight = 700))
                .child(sub("ob/0/s", "Small, fast widgets for your Windows desktop. Let's set things up — it takes under a minute.")),
            1 => Node::new("ob/1").col().align(taffy::AlignItems::CENTER).gap(12.0).child(title("ob/1/t", "Pick a look")).child(sub("ob/1/s", "Every widget follows the palette. You can change it any time in Appearance.")).child(Node::new("ob/1/sp").h(14.0)).child(self.palette_grid(k, ctx, wide.min(720.0), "ob/pal", false)),
            2 => {
                let ids: Vec<String> = self.catalog(ctx).into_iter().map(|(id, _)| id).collect();
                Node::new("ob/2").col().align(taffy::AlignItems::CENTER).gap(12.0).child(title("ob/2/t", "Put a few widgets out")).child(sub("ob/2/s", "Some are on your desktop already. Add more now, or later from Widgets.")).child(Node::new("ob/2/sp").h(14.0)).child(Node::new("ob/2/g").w(wide).child(self.widget_grid(k, ctx, wide, "ob/w", &ids, images)))
            }
            _ => {
                let rows = vec![
                    k.row("ob/auto", "Start with Windows", "Launch quietly into the tray when you sign in.", k.toggle("tg:autostart", ctx.ws.autostart, "autostart:toggle".into())),
                    flag_row(k, ctx, "ob/hd", Flag::HeaderDrag),
                    k.row("ob/hk", "Edit layout shortcut", "Move and resize widgets from anywhere in Windows.", k.keys("ob/hk/k", EDIT_KEYS)),
                ];
                Node::new("ob/3").col().align(taffy::AlignItems::CENTER).gap(12.0).child(title("ob/3/t", "You're all set")).child(sub("ob/3/s", "Two last choices. They're in General too, whenever you want them.")).child(Node::new("ob/3/sp").h(14.0)).child(Node::new("ob/3/g").w(wide.min(580.0)).child(k.group("ob/3/grp", "", rows)))
            }
        };
        let inner = Node::new("ob/inner").col().align(taffy::AlignItems::CENTER).justify(taffy::JustifyContent::CENTER).min_h(size.1 - 72.0).pad_xy(PAGE_PAD, 56.0).child(content);
        let stage = Node::new("ob/stage").col().grow(1.0).min_h(0.0).scroll(self.scroll_of("ob/stage")).hit().enter(260, 12.0, 0).child(inner);
        // a new key per step replays the enter animation
        let stage = Node::new(format!("ob/step/{step}")).col().grow(1.0).min_h(0.0).child(stage);
        let dots_row = Node::new("ob/dots").row().gap(8.0).align(taffy::AlignItems::CENTER).kids((0..SETUP_STEPS).map(|i| Node::new(format!("ob/dot/{i}")).h(6.0).w(if i == step { 22.0 } else { 6.0 }).radius(3.0).fill(if i == step { accent } else { k.ink(0.22) }).ease(220)));
        let mut btns = Node::new("ob/btns").row().grow(1.0).gap(8.0).justify(taffy::JustifyContent::FLEX_END);
        if step > 0 {
            btns = btns.child(k.btn("ob/back", None, "Back", "ob:back".into(), Btn::Outline).h(40.0));
        }
        let (label, next) = if step + 1 == SETUP_STEPS { ("Finish", "ob:done") } else if step == 0 { ("Get started", "ob:next") } else { ("Continue", "ob:next") };
        btns = btns.child(k.btn("ob/next", None, label, next.into(), Btn::Primary).h(40.0).pad_xy(18.0, 0.0).child(k.glyph("ob/next/chev".into(), "chevron-right", 11.0, k.c("accent-text"))));
        let foot = Node::new("ob/foot")
            .row()
            .h(72.0)
            .no_shrink()
            .align(taffy::AlignItems::CENTER)
            .pad_xy(28.0, 0.0)
            .child(Node::new("ob/foot/hr").abs(Some(0.0), Some(0.0), Some(0.0), None).h(1.0).fill(k.line()))
            .child(Node::new("ob/foot/l").grow(1.0).child(k.txt("ob/foot/n".into(), &format!("Step {} of {SETUP_STEPS}", step + 1), 13.0, dim)))
            .child(dots_row)
            .child(btns);
        let glow = Node::new("ob/glow").abs(Some(size.0 / 2.0 - 280.0), Some(-220.0), None, None).wh(560.0, 400.0).radius(200.0).shadow(170.0, 0.0, accent.with_alpha(0.13));
        let skip = Node::new("ob/skip").abs(None, Some(16.0), Some(20.0), None).h(32.0).pad_xy(10.0, 0.0).center().radius(8.0).hover_fill(k.ink(0.06)).ease(120).on("ob:done").child(k.txt("ob/skip/t".into(), "Skip setup", 13.0, dim));
        Node::new("ob").col().grow(1.0).min_h(0.0).child(dots("ob/bg".into(), k.ink(0.06))).child(glow).child(stage).child(foot).child(skip)
    }

    fn popup(&self, k: &Kit, ctx: &Ctx, open: &Open, size: (f32, f32)) -> Node {
        let pop_fill = k.bg().lerp(k.c("text"), 0.05).with_alpha(0.98);
        let scrim = Node::new("s/ov/scrim").abs_fill().overlay().on("popup-close");
        let mut root = Node::new("s/ov").abs_fill().overlay().child(scrim);
        match open {
            Open::Dropdown(key) => {
                let search = self.dropdown_items(ctx, key).len() > SEARCH_FROM;
                let font = self.is_font_key(ctx, key);
                let items = self.dropdown_shown(ctx, key);
                let cur = self.dropdown_current(ctx, key);
                let (ax, ay, aw, ah) = self.popup_anchor_rects.get(key).copied().unwrap_or((size.0 / 2.0 - 120.0, size.1 / 2.0, 240.0, 32.0));
                let list_h = (items.len().max(1) as f32 * DD_ROW + 10.0).min(DD_LIST_H);
                let h = list_h + if search { 49.0 } else { 0.0 };
                let below = ay + ah + 6.0 + h <= size.1 - 8.0;
                let y = if below { ay + ah + 6.0 } else { (ay - 6.0 - h).max(8.0) };
                let w = aw.max(if font { 280.0 } else { 220.0 });
                // ponytail: only the rows in view are built, so every installed font, each in its own face, stays cheap
                let off = self.scroll_of("s/ov/list");
                let first = ((off - DD_ROW * 2.0) / DD_ROW).floor().max(0.0) as usize;
                let last = (((off + list_h) / DD_ROW).ceil() as usize + 2).min(items.len());
                let mut col = Node::new("s/ov/items").col().pad(5.0).child(Node::new("s/ov/above").h(first.min(last) as f32 * DD_ROW));
                for (i, (v, label)) in items.iter().enumerate().take(last).skip(first) {
                    let on = *v == cur;
                    let mut t = k.txt(format!("s/ov/i/{i}/t"), label, 13.0, k.c("text")).grow_text();
                    if font && !v.is_empty() {
                        t = t.with_text(|t| t.family = v.clone());
                    }
                    col = col.child(
                        Node::new(format!("s/ov/i/{i}"))
                            .row()
                            .h(DD_ROW)
                            .no_shrink()
                            .align(taffy::AlignItems::CENTER)
                            .pad_xy(10.0, 0.0)
                            .gap(8.0)
                            .radius(8.0)
                            .fill(k.c("accent").with_alpha(if on { 0.16 } else { 0.0 }))
                            .hover_fill(k.ink(0.08))
                            .ease(100)
                            .on(format!("pick:{key}|{v}"))
                            .child(t)
                            .child(k.glyph(format!("s/ov/i/{i}/c"), "check", 11.0, k.c("accent").with_alpha(if on { 1.0 } else { 0.0 }))),
                    );
                }
                col = col.child(Node::new("s/ov/below").h(items.len().saturating_sub(last) as f32 * DD_ROW));
                if items.is_empty() {
                    col = col.child(Node::new("s/ov/none").h(DD_ROW).pad_xy(10.0, 0.0).align(taffy::AlignItems::CENTER).child(k.txt("s/ov/none/t".into(), "Nothing matches.", 13.0, k.c("text-dim"))));
                }
                let list = Node::new("s/ov/list").col().grow(1.0).min_h(0.0).scroll(self.scroll_of("s/ov/list")).hit().child(col);
                let mut pop = Node::new("s/ov/pop")
                    .col()
                    .abs(Some(ax), Some(y), None, None)
                    .w(w)
                    .h(h)
                    .radius(12.0)
                    .fill(pop_fill)
                    .border(1.0, k.line())
                    .shadow(18.0, 6.0, Color([0.0, 0.0, 0.0, 0.45]))
                    .clip()
                    .enter(160, if below { -6.0 } else { 6.0 }, 0)
                    .hit();
                if search {
                    let f = self.focus.as_ref().filter(|f| f.key == "q:dd").map(|f| (f.caret, self.caret_on));
                    let field = k.input("q:dd", &self.input_text(ctx, "q:dd"), if font { "Search fonts…" } else { "Search…" }, f, w - 12.0, false);
                    pop = pop.child(Node::new("s/ov/q").h(48.0).no_shrink().pad(6.0).child(field)).child(Node::new("s/ov/q/hr").h(1.0).no_shrink().fill(k.line()));
                }
                root = root.child(pop.child(list));
            }
            Open::Color(target) => {
                let (ax, ay, aw, ah) = self.popup_anchor_rects.get(&format!("cp:{target}")).copied().unwrap_or((size.0 / 2.0 - 130.0, size.1 / 2.0 - 100.0, 26.0, 26.0));
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
                        .fill(pop_fill)
                        .border(1.0, k.line())
                        .shadow(20.0, 8.0, Color([0.0, 0.0, 0.0, 0.5]))
                        .enter(170, if below { -6.0 } else { 6.0 }, 0)
                        .hit()
                        .child(sv_wrap)
                        .child(hue)
                        .child(Node::new("cp/cur").row().align(taffy::AlignItems::CENTER).gap(10.0).child(Node::new("cp/cur/sw").wh(28.0, 28.0).radius(14.0).fill(cur).border(1.0, k.line())).child(k.mono("cp/cur/t".into(), &cur.to_hex(), 13.0, k.c("text"))))
                        .child(presets),
                );
            }
        }
        root
    }
}

const SETUP_STEPS: usize = 4;
/// The live Widget at the top of each Add a widget card.
const GALLERY_PV_H: f32 = 150.0;
/// A dropdown longer than this gets a search.
const SEARCH_FROM: usize = 12;
/// A dropdown row, and the most of its list that shows at once.
const DD_ROW: f32 = 36.0;
const DD_LIST_H: f32 = 300.0;

/// Where a dropdown list of `n` rows can scroll to.
fn dd_max_scroll(n: usize) -> f32 {
    let content = n as f32 * DD_ROW + 10.0;
    (content - content.min(DD_LIST_H)).max(0.0)
}
const PREVIEW_W: f32 = 290.0;

/// The width a page's column gets in a window `size` wide.
fn content_w(size: (f32, f32), max_w: f32) -> f32 {
    (size.0 - 2.0 * PAGE_PAD).min(max_w)
}

/// How many cards of at least `min` fit in `w`, at most `max`, and how wide each is.
fn columns(w: f32, min: f32, gap: f32, max: usize) -> (usize, f32) {
    let cols = (((w + gap) / (min + gap)).floor() as usize).clamp(1, max);
    (cols, ((w - gap * (cols - 1) as f32) / cols as f32).floor())
}

/// Cards in rows of `cols`; a row's cards share the tallest one's height.
fn grid(key: &str, cols: usize, gap: f32, cards: Vec<Node>) -> Node {
    let mut g = Node::new(key).col().gap(gap);
    let mut row: Option<Node> = None;
    for (i, c) in cards.into_iter().enumerate() {
        if i % cols == 0 {
            if let Some(r) = row.take() {
                g = g.child(r);
            }
            row = Some(Node::new(format!("{key}/r{}", i / cols)).row().gap(gap));
        }
        row = row.map(|r| r.child(c));
    }
    match row {
        Some(r) => g.child(r),
        None => g,
    }
}

/// A Module's preview, small: one with a set width (a gauge) squeezed narrow, one that stretches
/// (a graph) in a short, wide box. Its text keeps its size, so a gauge only gets so small.
fn thumb_stage(key: String, thumb: Node) -> Node {
    let set_width = !thumb.style.max_size.width.is_auto() || thumb.style.size.width.into_option().is_some();
    let stage = Node::new(key).row().no_shrink().justify(taffy::JustifyContent::CENTER).clip();
    if set_width { stage.w(64.0).align(taffy::AlignItems::CENTER).child(thumb) } else { stage.wh(128.0, 64.0).child(thumb) }
}

/// Nothing in `n` takes a click: a preview's Modules would otherwise start a drag.
fn inert(n: &mut Node) {
    n.action = None;
    n.hit_testable = false;
    n.children.iter_mut().for_each(inert);
}

fn find_node<'a>(n: &'a Node, key: &str) -> Option<&'a Node> {
    if n.key == key {
        return Some(n);
    }
    n.children.iter().find_map(|c| find_node(c, key))
}

/// `AMD Radeon(TM) Graphics / Dx12 / IntegratedGpu / alpha … / present Mailbox` as chips.
fn gpu_parts(info: &str) -> Vec<String> {
    let parts: Vec<&str> = info.split(" / ").collect();
    let mut out: Vec<String> = parts.iter().take(1).map(|s| s.to_string()).collect();
    if let Some(b) = parts.get(1) {
        out.push(b.to_uppercase());
    }
    if let Some(d) = parts.get(2) {
        out.push(match *d {
            "Cpu" => "CPU".into(),
            "IntegratedGpu" => "Integrated GPU".into(),
            "DiscreteGpu" => "Dedicated GPU".into(),
            other => other.to_string(),
        });
    }
    out.extend(parts.iter().filter_map(|p| p.strip_prefix("present ")).map(String::from));
    out
}

fn level_color(k: &Kit, l: Level) -> Color {
    match l {
        Level::Info => Color([0.43, 0.66, 1.0, 1.0]),
        Level::Warning => k.warn(),
        Level::Error => k.c("danger"),
    }
}

/// A Code Source's state on its Plugin's card, and its colour.
fn code_status(k: &Kit, plugin_on: bool, c: &crate::plugins::CodeRow) -> (String, Color) {
    if !plugin_on {
        return ("Off".into(), k.c("text-dim"));
    }
    if !c.runs {
        return ("Not running".into(), k.c("text-dim"));
    }
    match c.status.as_str() {
        "" | "Starting" => ("Starting…".into(), k.warn()),
        "Running" => ("Running".into(), k.c("accent")),
        s => (s.to_string(), k.c("danger")),
    }
}

impl UiState {
    fn instance<'a>(ctx: &'a Ctx, id: &str) -> Option<&'a crate::workspace::InstanceCfg> {
        ctx.ws.instances.iter().find(|c| c.id == id)
    }

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
            self.popup_anchor_rects.insert(key, (x, y, w, h));
        }
    }

    pub fn focus_input(&mut self, ctx: &Ctx, key: &str, caret: Option<usize>) {
        let text = self.input_text(ctx, key);
        let caret = caret.unwrap_or(text.len()).min(text.len());
        self.focus = Some(Focus { key: key.to_string(), text, caret });
        if key != "q:dd" {
            self.open = None; // the dropdown's own search stays in it
        }
        self.caret_on = true;
        self.caret_at = Instant::now();
    }

    /// A click on nothing; an open dropdown keeps its search.
    pub fn blur(&mut self) {
        if !self.searching() {
            self.focus = None;
        }
    }

    fn searching(&self) -> bool {
        self.open.is_some() && self.focus.as_ref().is_some_and(|f| f.key == "q:dd")
    }

    fn close_popup(&mut self) {
        if self.searching() {
            self.focus = None;
        }
        self.open = None;
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
            self.gpu_picked = true;
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
        if let Some((scope, tok)) = key.strip_prefix("sy:").and_then(Self::style_target) {
            // the empty pick of a font is "Theme font": no override at all
            return vec![Cmd::Style(scope, tok.into(), (!value.is_empty()).then(|| Value::Str(value.into())))];
        }
        if let Some((id, axis)) = key.strip_prefix("tp:").and_then(|r| r.split_once(':')) {
            return vec![Cmd::ThemePick(id.into(), axis.into(), (!value.is_empty()).then(|| value.to_string()))];
        }
        if let Some((id, name)) = key.strip_prefix("p:").and_then(|r| r.split_once(':')) {
            return vec![Cmd::Param(id.into(), name.into(), Value::Str(value.into()))];
        }
        vec![]
    }

    /// `hwnd` parents native dialogs.
    pub fn act(&mut self, a: &str, ctx: &Ctx, hwnd: Option<windows::Win32::Foundation::HWND>) -> Vec<Cmd> {
        let (verb, rest) = a.split_once(':').unwrap_or((a, ""));
        if verb != "del" && verb != "prm" {
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
                self.adding = false;
                self.open = None;
                self.tier_tab = None;
                self.sel_module = None;
                vec![]
            }
            "tier" => {
                self.tier_tab = Some(rest.into());
                vec![]
            }
            "adv" => {
                self.advanced = !self.advanced;
                vec![]
            }
            "layreset" => rest.split_once('|').map(|(id, tier)| vec![Cmd::Layout(id.into(), tier.into(), None)]).unwrap_or_default(),
            "layresetall" => {
                let tiers: Vec<String> = Self::instance(ctx, rest).map(|c| c.layout.keys().cloned().collect()).unwrap_or_default();
                tiers.into_iter().map(|t| Cmd::Layout(rest.into(), t, None)).collect()
            }
            "add" => {
                self.tier_tab = None;
                self.sel_module = None;
                self.adding = false;
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
            "lim" => Self::instance(ctx, rest).map(|c| vec![Cmd::SizeLimit(rest.into(), !c.size_limit)]).unwrap_or_default(),
            "reset" => vec![Cmd::ResetPos(rest.into())],
            "edit" => vec![Cmd::Edit(!ctx.edit)],
            "autostart" => vec![Cmd::Autostart(!ctx.ws.autostart)],
            "flag" => Flag::parse(rest).map(|f| vec![Cmd::Flag(f, !ctx.ws.flag(f))]).unwrap_or_default(),
            "dd" => {
                let opening = !matches!(&self.open, Some(Open::Dropdown(k)) if k == rest);
                self.open = opening.then(|| Open::Dropdown(rest.into()));
                self.queries.remove("q:dd");
                let items = self.dropdown_items(ctx, rest);
                let cur = self.dropdown_current(ctx, rest);
                let at = items.iter().position(|(v, _)| *v == cur).unwrap_or(0);
                // every dropdown shares one list: open this one on its own pick, not where the last was left
                self.scroll.insert("s/ov/list".into(), (at as f32 * DD_ROW - DD_LIST_H / 2.0 + DD_ROW).clamp(0.0, dd_max_scroll(items.len())));
                if opening && items.len() > SEARCH_FROM {
                    self.focus = Some(Focus { key: "q:dd".into(), text: String::new(), caret: 0 });
                    self.caret_on = true;
                    self.caret_at = Instant::now();
                }
                vec![]
            }
            "pick" => {
                let Some((key, val)) = rest.split_once('|') else { return vec![] };
                self.close_popup();
                self.apply_pick(ctx, key, val)
            }
            "popup-close" => {
                self.close_popup();
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
            "file" => {
                let Some((id, name)) = rest.split_once('|') else { return vec![] };
                let types = [("Images and GIFs", "*.gif;*.png;*.jpg;*.jpeg;*.webp;*.bmp"), ("All files", "*.*")];
                match dialog::pick_file_of(hwnd, &types) {
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
            "sy" => {
                let Some((scope, tok)) = rest.split_once('|').map(|(s, t)| (Scope::parse(s), t)) else { return vec![] };
                let cur = Self::scope_theme(ctx, &scope).flag(tok);
                vec![Cmd::Style(scope, tok.into(), Some(Value::Bool(!cur)))]
            }
            "syreset" => {
                let Some((scope, tok)) = rest.split_once('|').map(|(s, t)| (Scope::parse(s), t)) else { return vec![] };
                if tok != "*" {
                    return vec![Cmd::Style(scope, tok.into(), None)];
                }
                let (set, picks): (Vec<String>, Vec<&str>) = match &scope {
                    Scope::Global => (ctx.ws.style.keys().cloned().collect(), vec![]),
                    Scope::Instance(id) => match Self::instance(ctx, id) {
                        Some(c) => (c.style.keys().cloned().collect(), THEME_AXES.iter().map(|(a, _)| *a).filter(|a| picked(&c.theme, a).is_some()).collect()),
                        None => (vec![], vec![]),
                    },
                };
                let mut cmds: Vec<Cmd> = set.into_iter().map(|k| Cmd::Style(scope.clone(), k, None)).collect();
                cmds.extend(picks.into_iter().map(|a| Cmd::ThemePick(scope.key().into(), a.into(), None)));
                cmds
            }
            "pon" => ctx.plugins.iter().find(|r| r.id == rest).map(|r| vec![Cmd::PluginEnabled(rest.into(), !r.enabled)]).unwrap_or_default(),
            "prm" => {
                let armed = format!("plugin/{rest}");
                if self.confirm_del.as_deref() == Some(armed.as_str()) {
                    self.confirm_del = None;
                    vec![Cmd::RemovePlugin(rest.into())]
                } else {
                    self.confirm_del = Some(armed);
                    vec![]
                }
            }
            "pfolder" => vec![Cmd::OpenPluginsFolder],
            "pinstall" => dialog::pick_file_of(hwnd, &[("Wayfinder plugin", "*.wfplugin;*.zip")]).map(|p| vec![Cmd::InstallPlugin(p)]).unwrap_or_default(),
            "openfolder" => vec![Cmd::OpenFolder],
            "claimfiles" => vec![Cmd::ClaimPluginFiles],
            "reload" => vec![Cmd::Reload],
            "quit" => vec![Cmd::Quit],
            "restart" => vec![Cmd::Restart],
            "gpufix" => vec![Cmd::Gpu("low".into()), Cmd::Restart],
            "gallery" => {
                self.adding = rest == "open";
                vec![]
            }
            "cat" => {
                self.category = (!rest.is_empty()).then(|| rest.to_string());
                vec![]
            }
            "lvl" => {
                self.log_level = Level::ALL.into_iter().find(|l| l.id() == rest);
                vec![]
            }
            "logcopy" => {
                let lines: Vec<String> = self.log_rows(ctx).iter().map(|l| format!("{} {:<7} {:<8} {}", l.time, l.level.label(), l.source, l.text)).collect();
                dialog::set_clipboard_text(&lines.join("\r\n"));
                vec![]
            }
            "openlog" => vec![Cmd::OpenData("wayfinder.log")],
            "openpacks" => vec![Cmd::OpenData("iconpacks")],
            "setc" => rest.split_once('|').and_then(|(t, hex)| Self::color_cmd(t, hex.into())).into_iter().collect(),
            "ob" => match rest {
                "next" => {
                    self.step = (self.step + 1).min(SETUP_STEPS - 1);
                    vec![]
                }
                "back" => {
                    self.step = self.step.saturating_sub(1);
                    vec![]
                }
                _ => {
                    self.step = 0;
                    self.page = Page::Widgets;
                    vec![Cmd::Onboarded]
                }
            },
            "close" => vec![Cmd::Close],
            "min" => vec![Cmd::Minimize],
            _ => vec![],
        }
    }

    /// A file or folder dropped on the window is a Plugin to install.
    pub fn dropped(&mut self, path: &Path) -> Vec<Cmd> {
        self.drop_hover = false;
        self.page = Page::Plugins;
        self.open = None;
        vec![Cmd::InstallPlugin(path.to_path_buf())]
    }

    /// `x` in window logical px.
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

    pub fn on_key(&mut self, key: &Key, text: Option<&str>, ctx: &Ctx) -> Vec<Cmd> {
        let ctrl = self.mods.control_key();
        if self.searching() && matches!(key, Key::Named(NamedKey::Enter | NamedKey::Escape)) {
            // Enter takes the first match, Escape closes the list
            let first = match (&self.open, key) {
                (Some(Open::Dropdown(dk)), Key::Named(NamedKey::Enter)) => self.dropdown_shown(ctx, dk).first().map(|(v, _)| format!("pick:{dk}|{v}")),
                _ => None,
            };
            self.close_popup();
            return first.map(|a| self.act(&a, ctx, None)).unwrap_or_default();
        }
        let Some(f) = self.focus.as_mut() else {
            if matches!(key, Key::Named(NamedKey::Escape)) {
                self.open = None;
                self.mdrag = None;
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
        if k.starts_with("q:") {
            if k == "q:dd" {
                self.scroll.insert("s/ov/list".into(), 0.0); // a shorter list must not open scrolled past its end
            }
            self.queries.insert(k, t);
            return vec![];
        }
        self.commit_text(ctx, &k, &t)
    }

    pub fn wheel(&mut self, dy: f32, mouse: (f32, f32), frame: &Frame) {
        let region = frame.scrolls.iter().rev().filter(|s| !s.horizontal).find(|s| frame.rect_of(&s.key).is_some_and(|[x, y, w, h]| mouse.0 >= x && mouse.0 < x + w && mouse.1 >= y && mouse.1 < y + h));
        // a popup list takes the wheel while a popup is open
        let region = if self.open.is_some() { frame.scrolls.iter().find(|s| s.key == "s/ov/list") } else { region };
        if let Some(r) = region {
            let max = (r.content - r.view).max(0.0);
            let cur = self.scroll_of(&r.key);
            self.scroll.insert(r.key.clone(), (cur - dy).clamp(0.0, max));
        }
    }
}


/// A Module being dragged in the preview or from the tray.
struct ModDrag {
    id: String,
    label: String,
    pos: (f32, f32),
    start: (f32, f32),
    /// Moved far enough to be a drag rather than a click.
    active: bool,
}

/// Where a dragged Module would land: a slot and a place in it, or the tray (`slot: None`).
#[derive(Debug, PartialEq)]
struct Drop {
    slot: Option<String>,
    index: usize,
}

fn inside(r: [f32; 4], p: (f32, f32), slack: f32) -> bool {
    p.0 >= r[0] - slack && p.0 <= r[0] + r[2] + slack && p.1 >= r[1] - slack && p.1 <= r[1] + r[3] + slack
}

/// Whether a param's `module = "gauge:cpu,graph:gpu*"` names Module `id`; a bare name (`gauge`) is every item of it.
fn names_module(list: &str, id: &str) -> bool {
    list.split(',').map(str::trim).any(|p| p == id || id.split(':').next() == Some(p) || p.strip_suffix('*').is_some_and(|x| id.starts_with(x)))
}

impl UiState {
    /// The preview of Instance `cfg`'s widget as a card on a dim backdrop, and what it placed.
    fn preview(&self, k: &Kit, ctx: &Ctx, cfg: &crate::workspace::InstanceCfg, images: &mut Vec<String>) -> (Node, Option<Arrangement>) {
        let id = &cfg.id;
        let backdrop = |child: Node| Node::new(format!("ip/{id}/pv")).col().align(taffy::AlignItems::CENTER).pad(20.0).radius(14.0).fill(k.ink(0.02)).border(1.0, k.line()).clip().child(dots(format!("ip/{id}/pv/dots"), k.ink(0.1))).child(child);
        let note = |s: &str| backdrop(k.txt(format!("ip/{id}/pvn"), s, 12.5, k.c("text-dim")).wrap_text());
        let Some(Ok(w)) = ctx.reg.get(&cfg.widget) else { return (note("No preview: the definition failed to load."), None) };
        let meta = w.meta();
        let unmet = meta.unmet(|n| ctx.sources.iter().any(|s| s == n));
        if !unmet.is_empty() {
            return (note(&format!("No preview: {}", crate::widgets::needs_message(&unmet))), None);
        }
        let forced = self.tier_tab.as_deref().and_then(|t| meta.tiers.iter().find(|x| x.name == t));
        let size = forced.map_or((cfg.w.min(560.0), cfg.h), |t| t.size);
        match self.build_widget(ctx, cfg, &cfg.layout, forced.map(|t| t.name.as_str()), size, &format!("pv/{id}")) {
            Ok(b) => {
                images.extend(b.image_ids.iter().cloned());
                (backdrop(b.root), b.arrangement)
            }
            Err(e) => (note(&format!("No preview: {e}")), None),
        }
    }

    /// Instance `cfg`'s widget as Settings previews it: inert but for its Modules, arranged by
    /// `layout`, in `tier` whatever `size` says.
    fn build_widget(&self, ctx: &Ctx, cfg: &crate::workspace::InstanceCfg, layout: &crate::workspace::Layout, tier: Option<&str>, size: (f32, f32), key: &str) -> Result<crate::widgets::Built, String> {
        let Some(Ok(w)) = ctx.reg.get(&cfg.widget) else { return Err("the definition failed to load".into()) };
        let theme = ctx.ws.theme_for(ctx.lib, cfg);
        let params = w.meta().effective_params(&cfg.params_map());
        let scx = crate::data::SourceCx { cfg, params: &params, tm: crate::data::now_local(), icon_pack: &cfg.theme.resolve(&ctx.ws.theme).icon_pack };
        let read = |n: &str| ctx.data.value(n, &scx);
        let state = std::collections::BTreeMap::new();
        let arrange = crate::modules::Arrange { layout, tier, preview: true };
        let inp = crate::widgets::Inputs { params: &params, state: &state, card_size: size, key_prefix: key, read_source: &read, arrange: Some(arrange) };
        w.build(&inp, &theme, &|_| None)
    }

    /// Hidden Module `m` on its own, for the tray: the widget built with nothing but `m`, in the
    /// first slot of this tier that takes it, and `m`'s node lifted out. With its kind (`Graph`)
    /// when its label alone does not say it, as a gauge and a graph of one GPU share a label.
    // ponytail: a build per hidden Module per frame; cache them per layout and tier if the Widgets page ever drags
    fn module_thumb(&self, ctx: &Ctx, cfg: &crate::workspace::InstanceCfg, a: &Arrangement, m: &crate::modules::Placed, prefix: &str, images: &mut Vec<String>) -> Option<(Node, Option<String>)> {
        let meta = self.def_of(ctx, &cfg.widget)?;
        let def = meta.modules.iter().find(|d| d.name == m.module)?;
        let slot = a.slots.iter().find(|s| def.slots.is_empty() || def.slots.contains(&s.name))?;
        let size = meta.tiers.iter().find(|t| t.name == a.tier).map_or((cfg.w, cfg.h), |t| t.size);
        let layout = crate::workspace::Layout::from([(a.tier.clone(), std::collections::BTreeMap::from([(slot.name.clone(), vec![m.id.clone()])]))]);
        let b = self.build_widget(ctx, cfg, &layout, Some(&a.tier), size, &format!("{prefix}/{}/{}", cfg.id, m.id)).ok()?;
        images.extend(b.image_ids.iter().cloned());
        let key = &b.arrangement.as_ref()?.slots.iter().flat_map(|s| &s.modules).find(|p| p.id == m.id)?.key;
        let mut node = find_node(&b.root, key)?.clone();
        // the tile around it is what drags
        node.action = None;
        node.hit_testable = false;
        Some((node, (def.label != m.label).then(|| capitalized(&def.label))))
    }

    /// Tier tabs, the preview, the tray of hidden Modules and the reset buttons.
    fn preview_block(&self, k: &Kit, ctx: &Ctx, cfg: &crate::workspace::InstanceCfg, images: &mut Vec<String>) -> Node {
        let id = &cfg.id;
        let (pv, arr) = self.preview(k, ctx, cfg, images);
        *self.tray_key.borrow_mut() = format!("ip/{id}/tray");
        *self.preview_arr.borrow_mut() = arr.clone();
        let mut b = Node::new(format!("ip/{id}/pvb")).col().gap(10.0);
        let Some(meta) = self.def_of(ctx, &cfg.widget) else { return b.child(pv) };
        if let Some(a) = &arr {
            let mut tabs = Node::new(format!("ip/{id}/tabs")).row().wrap().align(taffy::AlignItems::CENTER).gap(6.0);
            for t in &meta.tiers {
                let on = t.name == a.tier;
                tabs = tabs.child(
                    Node::new(format!("ip/{id}/tab/{}", t.name))
                        .h(32.0)
                        .pad_xy(12.0, 0.0)
                        .center()
                        .radius(9.0)
                        .fill(k.c("accent").with_alpha(if on { 0.12 } else { 0.0 }))
                        .border(1.0, if on { k.c("accent").with_alpha(0.5) } else { k.line() })
                        .hover_fill(if on { k.c("accent").with_alpha(0.16) } else { k.ink(0.05) })
                        .ease(120)
                        .on(format!("tier:{}", t.name))
                        .child(k.bold(format!("ip/{id}/tab/{}/t", t.name), &t.label, 12.5, if on { k.c("text") } else { k.c("text-dim") })),
                );
            }
            tabs = tabs.child(Node::new(format!("ip/{id}/tabs/sp")).grow(1.0));
            if cfg.layout.contains_key(&a.tier) {
                tabs = tabs.child(k.btn(&format!("ip/{id}/lr"), None, "Reset this size", format!("layreset:{id}|{}", a.tier), Btn::Outline));
            }
            if cfg.layout.len() > 1 || (cfg.layout.len() == 1 && !cfg.layout.contains_key(&a.tier)) {
                tabs = tabs.child(k.btn(&format!("ip/{id}/lra"), None, "Reset all sizes", format!("layresetall:{id}"), Btn::Outline));
            }
            b = b.child(tabs).child(pv);
            let head = Node::new(format!("ip/{id}/tray/h"))
                .col()
                .gap(2.0)
                .child(k.bold(format!("ip/{id}/tray/h/t"), "Available modules", 13.5, k.c("text")))
                .child(k.txt(format!("ip/{id}/tray/h/s"), "Drag one into the preview to show it, or a module out of it to put it here.", 12.0, k.c("text-dim")).wrap_text());
            let mut tiles = Node::new(format!("ip/{id}/tray/l")).row().wrap().gap(8.0).align(taffy::AlignItems::FLEX_START);
            if a.hidden.is_empty() {
                tiles = tiles.child(k.txt(format!("ip/{id}/tray/e"), "Everything fits on the widget at this size.", 12.5, k.c("text-dim")));
            }
            for m in &a.hidden {
                let tk = format!("ip/{id}/tray/{}", m.id);
                let (stage, kind) = match self.module_thumb(ctx, cfg, a, m, "tray", images) {
                    Some((thumb, kind)) => (thumb_stage(format!("{tk}/pv"), thumb), kind),
                    None => (Node::new(format!("{tk}/pv")), None),
                };
                let mut cap = Node::new(format!("{tk}/cap")).row().gap(5.0).align(taffy::AlignItems::CENTER).child(k.bold(format!("{tk}/n"), &m.label, 11.5, k.c("text")));
                if let Some(kind) = kind {
                    cap = cap.child(k.txt(format!("{tk}/k"), &kind, 11.0, k.c("text-dim")));
                }
                tiles = tiles.child(Node::new(tk.clone()).col().gap(6.0).pad(6.0).align(taffy::AlignItems::CENTER).radius(9.0).fill(k.ink(0.03)).border(1.0, k.line()).hover_fill(k.ink(0.07)).ease(120).on(format!("mod:{}", m.id)).child(stage).child(cap));
            }
            b = b.child(Node::new(format!("ip/{id}/tray")).col().gap(12.0).pad(14.0).min_h(64.0).radius(12.0).fill(k.panel()).border(1.0, k.line()).child(head).child(tiles));
            let cut: Vec<String> = a.slots.iter().flat_map(|s| s.cut.iter().map(|m| m.label.clone())).collect();
            if !cut.is_empty() {
                b = b.child(k.txt(format!("ip/{id}/cut"), &format!("No room at this size, left out: {}.", cut.join(", ")), 11.5, k.c("text-dim")).wrap_text());
            }
            b = b.child(k.txt(format!("ip/{id}/hint"), "Drag modules to rearrange them for this size. Click one to change its options.", 11.5, k.c("text-dim")).wrap_text());
        } else {
            b = b.child(pv);
        }
        b
    }

    /// Widget-wide options, then the selected Module's own.
    fn module_options(&self, k: &Kit, ctx: &Ctx, cfg: &crate::workspace::InstanceCfg, meta: &WidgetMeta, images: &mut Vec<String>) -> Node {
        let id = &cfg.id;
        let mut c = Node::new(format!("ip/{id}/mo")).col().gap(22.0);
        let mut section = |c: Node, key: String, title: &str, params: Vec<ParamDef>| {
            let mut c = c;
            for (i, (group, ps)) in ParamDef::grouped(&params).into_iter().enumerate() {
                let title = match group {
                    Some(g) => format!("{title}: {g}"),
                    None => title.to_string(),
                };
                let rows = ps.into_iter().map(|pd| self.param_row(k, ctx, cfg, pd, images)).collect();
                c = c.child(k.group(&format!("{key}/{i}"), &title, rows));
            }
            c
        };
        let wide: Vec<ParamDef> = meta.params.iter().filter(|p| p.module.is_none()).cloned().collect();
        c = section(c, format!("ip/{id}/wo"), "Widget options", wide);
        let arr = self.preview_arr.borrow();
        let picked = self.sel_module.as_deref().and_then(|s| {
            let all = arr.iter().flat_map(|a| a.slots.iter().flat_map(|s| s.modules.iter().chain(&s.cut)).chain(&a.hidden));
            all.into_iter().find(|m| m.id == s).map(|m| (m.id.clone(), m.label.clone()))
        });
        match picked {
            Some((mid, label)) => {
                let own: Vec<ParamDef> = meta.params.iter().filter(|p| p.module.as_deref().is_some_and(|l| names_module(l, &mid))).cloned().collect();
                if own.is_empty() {
                    let none = Node::new(format!("ip/{id}/mo/n")).pad_xy(20.0, 14.0).child(k.txt(format!("ip/{id}/mo/n/t"), "This module has no options of its own.", 12.5, k.c("text-dim")));
                    c = c.child(k.group(&format!("ip/{id}/mo/t"), &label, vec![none]));
                } else {
                    c = section(c, format!("ip/{id}/mo/o"), &label, own);
                }
            }
            None => {}
        }
        c
    }

    /// The tab a Module drag starts from, or a click selects.
    fn mod_press(&mut self, id: &str, pos: (f32, f32)) {
        let label = self.preview_arr.borrow().iter().flat_map(|a| a.slots.iter().flat_map(|s| s.modules.iter().chain(&s.cut)).chain(&a.hidden)).find(|m| m.id == id).map(|m| m.label.clone()).unwrap_or_else(|| id.to_string());
        self.mdrag = Some(ModDrag { id: id.into(), label, pos, start: pos, active: false });
    }

    /// True while a drag is in progress, so the window redraws with the marker.
    fn mod_move(&mut self, pos: (f32, f32)) -> bool {
        let Some(d) = self.mdrag.as_mut() else { return false };
        d.pos = pos;
        if !d.active && ((pos.0 - d.start.0).powi(2) + (pos.1 - d.start.1).powi(2)).sqrt() > 5.0 {
            d.active = true;
        }
        true
    }

    /// Where the drag would land at `pos`, from the rects of the last frame.
    fn drop_at(&self, ctx: &Ctx, pos: (f32, f32)) -> Option<Drop> {
        let d = self.mdrag.as_ref()?;
        let arr = self.preview_arr.borrow();
        let arr = arr.as_ref()?;
        let cfg = self.selected_cfg(ctx)?;
        let meta = self.def_of(ctx, &cfg.widget)?;
        let module = arr.slots.iter().flat_map(|s| s.modules.iter().chain(&s.cut)).chain(&arr.hidden).find(|m| m.id == d.id)?.module.clone();
        let allowed = meta.modules.iter().find(|m| m.name == module).map(|m| m.slots.clone()).unwrap_or_default();
        for s in &arr.slots {
            let Some(r) = self.preview_rects.get(&format!("slot:{}", s.name)).copied() else { continue };
            if !inside(r, pos, 6.0) || !(allowed.is_empty() || allowed.iter().any(|a| *a == s.name)) {
                continue;
            }
            let rest: Vec<[f32; 4]> = s.modules.iter().filter(|m| m.id != d.id).filter_map(|m| self.preview_rects.get(&format!("mod:{}", m.id)).copied()).collect();
            // in reading order, a Module is before the pointer when it is on an earlier row, or left of it on the same one
            let before = rest.iter().filter(|m| m[1] + m[3] <= pos.1 || (pos.1 >= m[1] && m[0] + m[2] / 2.0 < pos.0)).count();
            return Some(Drop { slot: Some(s.name.clone()), index: before });
        }
        let tray = self.preview_rects.get("tray").copied()?;
        inside(tray, pos, 0.0).then_some(Drop { slot: None, index: 0 })
    }

    /// A click selects the Module; a drag moves it to where it was let go.
    fn mod_release(&mut self, pos: (f32, f32), ctx: &Ctx) -> Vec<Cmd> {
        let Some(d) = self.mdrag.as_ref() else { return vec![] };
        let (id, active) = (d.id.clone(), d.active);
        let target = if active { self.drop_at(ctx, pos) } else { None };
        self.mdrag = None;
        if !active {
            self.sel_module = Some(id);
            return vec![];
        }
        let (Some(t), Some(arr), Some(cfg)) = (target, self.preview_arr.borrow().clone(), self.selected_cfg(ctx)) else { return vec![] };
        let mut layout = arr.as_layout();
        for v in layout.values_mut() {
            v.retain(|m| *m != id);
        }
        if let Some(slot) = t.slot {
            let v = layout.entry(slot).or_default();
            let at = t.index.min(v.len());
            v.insert(at, id.clone());
        }
        self.sel_module = Some(id);
        vec![Cmd::Layout(cfg.id.clone(), arr.tier, Some(layout))]
    }

    /// Rects of the preview's Modules, slots and the tray, for the next frame's drop marker.
    pub fn record_preview(&mut self, frame: &Frame) {
        self.preview_rects.clear();
        for h in &frame.hits {
            if let Some(id) = h.action.as_deref().and_then(|a| a.strip_prefix("mod:")) {
                self.preview_rects.insert(format!("mod:{id}"), h.rect);
            }
        }
        if let Some(a) = self.preview_arr.borrow().as_ref() {
            for s in &a.slots {
                if let Some(r) = frame.rect_of(&s.key) {
                    self.preview_rects.insert(format!("slot:{}", s.name), r);
                }
            }
        }
        let tray = frame.rect_of(&self.tray_key.borrow());
        if let Some(r) = tray {
            self.preview_rects.insert("tray".into(), r);
        }
    }

    /// Outline of the selected Module, and while dragging the valid drop slots, an insertion bar and a ghost.
    fn module_overlay(&self, k: &Kit, ctx: &Ctx, images: &mut Vec<String>) -> Option<Node> {
        if self.page != Page::Widgets {
            return None;
        }
        let mut o = Node::new("s/mo").abs_fill().overlay();
        let mut any = false;
        if let Some(r) = self.sel_module.as_ref().and_then(|s| self.preview_rects.get(&format!("mod:{s}"))) {
            o = o.child(Node::new("s/mo/sel").abs(Some(r[0] - 2.0), Some(r[1] - 2.0), None, None).wh(r[2] + 4.0, r[3] + 4.0).radius(8.0).border(2.0, k.c("accent")));
            any = true;
        }
        if let Some(d) = self.mdrag.as_ref().filter(|d| d.active) {
            let drop = self.drop_at(ctx, d.pos);
            let arr = self.preview_arr.borrow();
            // what follows the pointer: the Module itself, as the tray shows it
            let mut lifted = None;
            if let Some(a) = arr.as_ref() {
                let cfg = self.selected_cfg(ctx);
                let placed = a.slots.iter().flat_map(|s| s.modules.iter().chain(&s.cut)).chain(&a.hidden).find(|m| m.id == d.id);
                lifted = cfg.zip(placed).and_then(|(c, m)| self.module_thumb(ctx, c, a, m, "ghost", images)).map(|(n, _)| thumb_stage("s/mo/ghost/pv".into(), n));
                // and where it was, faded, so it reads as picked up rather than copied
                if let Some(r) = self.preview_rects.get(&format!("mod:{}", d.id)) {
                    o = o.child(Node::new("s/mo/from").abs(Some(r[0]), Some(r[1]), None, None).wh(r[2], r[3]).radius(8.0).fill(k.bg().with_alpha(0.65)).border(1.0, k.c("accent").with_alpha(0.35)));
                }
                let module = placed.map(|m| m.module.clone());
                let allowed = cfg.and_then(|c| self.def_of(ctx, &c.widget)).and_then(|m| m.modules.iter().find(|x| Some(&x.name) == module.as_ref())).map(|m| m.slots.clone()).unwrap_or_default();
                for s in &a.slots {
                    let Some(r) = self.preview_rects.get(&format!("slot:{}", s.name)) else { continue };
                    if !(allowed.is_empty() || allowed.iter().any(|x| *x == s.name)) {
                        continue;
                    }
                    let hot = drop.as_ref().is_some_and(|t| t.slot.as_deref() == Some(s.name.as_str()));
                    o = o.child(Node::new(format!("s/mo/slot/{}", s.name)).abs(Some(r[0]), Some(r[1]), None, None).wh(r[2], r[3]).radius(8.0).fill(k.c("accent").with_alpha(if hot { 0.14 } else { 0.05 })).border(1.0, k.c("accent").with_alpha(if hot { 0.9 } else { 0.4 })));
                }
                if let Some(Drop { slot: Some(slot), index }) = &drop {
                    if let Some(s) = a.slots.iter().find(|s| s.name == *slot) {
                        let rest: Vec<[f32; 4]> = s.modules.iter().filter(|m| m.id != d.id).filter_map(|m| self.preview_rects.get(&format!("mod:{}", m.id)).copied()).collect();
                        let column = rest.len() > 1 && (rest[1][1] - rest[0][1]).abs() > (rest[1][0] - rest[0][0]).abs();
                        let bar = if let Some(m) = rest.get(*index) {
                            if column { Some((m[0], m[1] - 3.0, m[2], 3.0)) } else { Some((m[0] - 4.0, m[1], 3.0, m[3])) }
                        } else if let Some(m) = rest.last() {
                            if column { Some((m[0], m[1] + m[3] + 1.0, m[2], 3.0)) } else { Some((m[0] + m[2] + 1.0, m[1], 3.0, m[3])) }
                        } else {
                            self.preview_rects.get(&format!("slot:{slot}")).map(|r| (r[0] + 4.0, r[1] + 4.0, 3.0, (r[3] - 8.0).max(8.0)))
                        };
                        if let Some((x, y, w, h)) = bar {
                            o = o.child(Node::new("s/mo/bar").abs(Some(x), Some(y), None, None).wh(w, h).radius(1.5).fill(k.c("accent")));
                        }
                    }
                }
                if drop.as_ref().is_some_and(|t| t.slot.is_none()) {
                    if let Some(r) = self.preview_rects.get("tray") {
                        o = o.child(Node::new("s/mo/tray").abs(Some(r[0]), Some(r[1]), None, None).wh(r[2], r[3]).radius(10.0).border(2.0, k.c("accent")));
                    }
                }
            }
            let body = lifted.unwrap_or_else(|| k.txt("s/mo/ghost/t".into(), &d.label, 12.0, k.c("text")));
            o = o.child(
                Node::new("s/mo/ghost")
                    .abs(Some(d.pos.0 + 14.0), Some(d.pos.1 + 12.0), None, None)
                    .pad(6.0)
                    .radius(10.0)
                    .fill(k.bg().lerp(k.c("text"), 0.05).with_alpha(0.96))
                    .border(1.5, k.c("accent"))
                    .shadow(16.0, 6.0, Color([0.0, 0.0, 0.0, 0.45]))
                    .opacity(0.94)
                    .child(body),
            );
            any = true;
        }
        any.then_some(o)
    }
}

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
    /// Rebuild on the next frame, e.g. after a change made outside the window.
    pub fn invalidate(&mut self) {
        self.redraw = true;
    }

    /// Shows a page by its id (`plugins`...).
    pub fn show_page(&mut self, id: &str) {
        self.ui.page = Page::parse(id);
        self.redraw = true;
    }

    pub fn open(el: &ActiveEventLoop, gpu: &mut Option<Gpu>, power: Power) -> Result<SettingsWin, String> {
        let mon = el.primary_monitor().or_else(|| el.available_monitors().next());
        // a small or zoomed screen gets a smaller window, never below the minimum
        let win = mon.as_ref().map_or((WIN.0 as f64, WIN.1 as f64), |m| {
            let (s, sc) = (m.size(), m.scale_factor());
            ((s.width as f64 / sc * 0.92).clamp(MIN_WIN.0 as f64, WIN.0 as f64), (s.height as f64 / sc * 0.88).clamp(MIN_WIN.1 as f64, WIN.1 as f64))
        });
        let pos = mon.map(|m| {
            let (p, s, sc) = (m.position(), m.size(), m.scale_factor());
            PhysicalPosition::new(p.x + ((s.width as f64 - win.0 * sc) / 2.0).max(0.0) as i32, p.y + ((s.height as f64 - win.1 * sc) / 2.0).max(0.0) as i32)
        });
        let mut attrs = WindowAttributes::default()
            .with_title("Wayfinder Settings")
            .with_resizable(true)
            .with_visible(false)
            .with_inner_size(LogicalSize::new(win.0, win.1))
            .with_min_inner_size(LogicalSize::new(MIN_WIN.0 as f64, MIN_WIN.1 as f64))
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
            "mod" => {
                self.ui.mod_press(action.strip_prefix("mod:").unwrap_or(""), self.mouse);
                vec![]
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
                    self.ui.mod_move(self.mouse);
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
                cmds.extend(self.ui.mod_release(self.mouse, ctx));
                self.redraw = true;
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
            WindowEvent::HoveredFile(_) => {
                self.ui.drop_hover = true;
                self.ui.page = Page::Plugins;
                self.redraw = true;
            }
            WindowEvent::HoveredFileCancelled => {
                self.ui.drop_hover = false;
                self.redraw = true;
            }
            WindowEvent::DroppedFile(p) => cmds.extend(self.ui.dropped(p)),
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
        self.ui.record_preview(&frame);
        // an open popup needs its anchor rect to position itself; one more frame settles it
        self.redraw = self.ui.has_popup() && !self.ui.popup_anchor_rects.contains_key(&self.ui.anchor_key());
        self.frame = Some(frame);
        self.last = now;
    }

    /// The images the last frame drew.
    pub fn drawn_images(&self) -> impl Iterator<Item = &str> {
        self.frame.iter().flat_map(|f| f.list.image_ids())
    }

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
        hidden: Vec<(String, Hidden)>,
        plugins: Vec<PluginRow>,
        sources: Vec<String>,
        data: crate::data::DataSources,
    }

    fn world() -> World {
        let lib = Library::load(Path::new("no-such-dir"));
        let theme = Theme::compose(&lib, &Selection::default(), &[]);
        let mut ws = Workspace::default();
        ws.onboarded = true;
        ws.instances.push(InstanceCfg { id: "clock-1".into(), widget: "clock".into(), monitor: MonitorRef { name: "\\\\.\\DISPLAY1".into(), width: 1920, height: 1080 }, ..Default::default() });
        let mut folder = InstanceCfg { id: "icon_folder-1".into(), widget: "icon_folder".into(), ..Default::default() };
        folder.set_items(&[Shortcut { name: "A".into(), target: "a.exe".into(), icon: String::new() }, Shortcut { name: "B".into(), target: "b.exe".into(), icon: String::new() }]);
        ws.instances.push(folder);
        let plugins = vec![
            PluginRow { id: "sunset".into(), name: "Sunset".into(), version: "1.2.0".into(), author: "Ada".into(), description: "Warm colours".into(), summary: "1 widget · 1 palette".into(), enabled: true, notes: vec!["Restyles Analog Clock".into()], problems: vec![], sole_widgets: vec!["weather".into()], contents: crate::content::Contents { widgets: vec!["weather".into()], palettes: vec!["Midnight".into()], ..Default::default() }, code: vec![crate::plugins::CodeRow { source: "weather".into(), net: vec!["api.open-meteo.com".into()], runs: true, status: "Running".into(), ..Default::default() }] },
            PluginRow { id: "broken".into(), name: "broken".into(), summary: "nothing yet".into(), enabled: false, problems: vec!["no plugin.toml".into()], ..Default::default() },
        ];
        World { ws, reg: Registry::load(Path::new("no-such-dir")), lib, theme, hidden: vec![], plugins, sources: crate::data::DataSources::builtin().names(), data: crate::data::DataSources::builtin() }
    }

    fn ctx(w: &World) -> Ctx<'_> {
        Ctx { ws: &w.ws, reg: &w.reg, lib: &w.lib, theme: &w.theme, log: &[], gpu_info: "test gpu", fonts: &[], edit: false, hidden: &w.hidden, plugins: &w.plugins, plugin_note: "", sources: &w.sources, plugin_files: &FileOwner::Me, data: &w.data }
    }

    #[test]
    fn general_says_which_app_opens_plugin_files() {
        fn has(n: &Node, key: &str) -> bool {
            n.key == key || n.children.iter().any(|c| has(c, key))
        }
        let w = world();
        let mut ui = UiState::default();
        ui.page = Page::General;
        let mine = ui.build(&ctx(&w), WIN).0;
        assert!(has(&mine, "gn/pf") && !has(&mine, "gn/pf/b"), "already this app: no button");
        let other = FileOwner::Other("C:\\Apps\\wayfinder.exe".into());
        let theirs = ui.build(&Ctx { plugin_files: &other, ..ctx(&w) }, WIN).0;
        assert!(has(&theirs, "gn/pf/b"), "another build has them: offer to switch");
        assert_eq!(ui.act("claimfiles", &ctx(&w), None), [Cmd::ClaimPluginFiles]);
    }

    #[test]
    fn a_widget_missing_a_data_source_says_so() {
        let dir = std::env::temp_dir().join(format!("wf-settings-needs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("agents.toml"), "name = 'Agents'\nneeds = ['agents', 'clock']\n[root]\ntype = 'box'").unwrap();
        let mut w = world();
        w.reg.load_dir(&dir);
        let s = UiState::default();
        assert_eq!(s.needs_note(&ctx(&w), "agents").as_deref(), Some("  ·  needs agents"));
        w.sources.push("agents".into());
        assert_eq!(s.needs_note(&ctx(&w), "agents"), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_long_description_wraps_and_keeps_remove_in_the_window() {
        let dir = std::env::temp_dir().join(format!("wf-settings-essay-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let long = "A widget whose author had a great deal to say about it, far more than one line of a narrow window can hold. ".repeat(3);
        std::fs::write(dir.join("essay.toml"), format!("name = 'Essay'\ndescription = '{long}'\n[root]\ntype = 'box'")).unwrap();
        let mut w = world();
        w.reg.load_dir(&dir);
        w.ws.instances.push(InstanceCfg { id: "essay-1".into(), widget: "essay".into(), ..Default::default() });
        w.ws.instances.push(InstanceCfg { id: "system_monitor-1".into(), widget: "system_monitor".into(), ..Default::default() });
        let laid = |ui: &UiState| {
            let (root, _) = ui.build(&ctx(&w), MIN_WIN);
            let (mut text, mut anim) = (TextEngine::new(), Anim::default());
            let mut env = Env { text: &mut text, anim: &mut anim, hover: None, now: Instant::now(), scale: 1.0 };
            ui::layout(&root, MIN_WIN, &mut env)
        };
        let mut ui = UiState::default();
        ui.selected = Some("essay-1".into());
        let f = laid(&ui);
        let [x, _, bw, _] = f.rect_of("ip/essay-1/del").unwrap();
        assert!(x + bw <= MIN_WIN.0, "Remove ends at {} in a {} px window", x + bw, MIN_WIN.0);
        let [_, _, _, dh] = f.rect_of("ip/essay-1/hs").unwrap();
        assert!(dh > 20.0, "the description wraps onto more lines ({dh} px tall)");
        ui.selected = Some("system_monitor-1".into());
        ui.advanced = true; // Placement and Style sit under "Advanced"
        let f = laid(&ui);
        for (k, [x, _, w, _]) in f.rects.iter().filter(|(k, _)| k.starts_with("sr/system_monitor-1/") || k.starts_with("ip/system_monitor-1/tp")) {
            assert!(x + w <= MIN_WIN.0, "`{k}` ends at {} in a {} px window", x + w, MIN_WIN.0);
        }
        assert!(f.rect_of("sr/system_monitor-1/anim-speed").is_some(), "the Style section is on the widget's panel");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The Settings frame of a system monitor Instance, laid out at `size`.
    fn monitor_frame(ui: &mut UiState, w: &World, size: (f32, f32)) -> Frame {
        let (root, _) = ui.build(&ctx(w), size);
        let (mut text, mut anim) = (TextEngine::new(), Anim::default());
        let mut env = Env { text: &mut text, anim: &mut anim, hover: None, now: Instant::now(), scale: 1.0 };
        let f = ui::layout(&root, size, &mut env);
        ui.record_preview(&f);
        f
    }

    #[test]
    fn dragging_a_module_in_the_preview_saves_a_layout_and_a_click_selects_it() {
        let mut w = world();
        w.ws.instances.push(InstanceCfg { id: "system_monitor-1".into(), widget: "system_monitor".into(), w: 340.0, h: 190.0, ..Default::default() });
        let mut ui = UiState::default();
        ui.selected = Some("system_monitor-1".into());
        let big = (1200.0, 900.0);
        monitor_frame(&mut ui, &w, big);
        let f = monitor_frame(&mut ui, &w, big); // the second build knows where the first drew
        let rect = |id: &str| ui.preview_rects.get(&format!("mod:{id}")).copied().unwrap_or_else(|| panic!("no rect for {id}: {:?}", ui.preview_rects.keys().collect::<Vec<_>>()));
        let (cpu, ram) = (rect("gauge:cpu"), rect("gauge:ram"));
        let centre = |r: [f32; 4]| (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);

        // a click selects
        ui.mod_press("gauge:cpu", centre(cpu));
        assert!(ui.mod_release(centre(cpu), &ctx(&w)).is_empty());
        assert_eq!(ui.sel_module.as_deref(), Some("gauge:cpu"));

        // dragging CPU past RAM puts it after RAM
        ui.mod_press("gauge:cpu", centre(cpu));
        ui.mod_move((ram[0] + ram[2] - 2.0, ram[1] + ram[3] / 2.0));
        let cmds = ui.mod_release((ram[0] + ram[2] - 2.0, ram[1] + ram[3] / 2.0), &ctx(&w));
        let [Cmd::Layout(id, tier, Some(layout))] = cmds.as_slice() else { panic!("{cmds:?}") };
        assert_eq!((id.as_str(), tier.as_str()), ("system_monitor-1", "normal"));
        assert_eq!(&layout["gauges"][..2], ["gauge:ram", "gauge:cpu"]);
        let _ = f;

        // dropping on the tray hides it
        ui.mod_press("gauge:cpu", centre(cpu));
        let tray = ui.preview_rects["tray"];
        let cmds = ui.mod_release((tray[0] + 20.0, tray[1] + tray[3] / 2.0), &ctx(&w)); // not moved yet: only a click
        assert!(cmds.is_empty());
        ui.mod_press("gauge:cpu", centre(cpu));
        ui.mod_move((tray[0] + 20.0, tray[1] + tray[3] / 2.0));
        let cmds = ui.mod_release((tray[0] + 20.0, tray[1] + tray[3] / 2.0), &ctx(&w));
        let [Cmd::Layout(_, _, Some(layout))] = cmds.as_slice() else { panic!("{cmds:?}") };
        assert!(layout.values().all(|v| !v.iter().any(|m| m == "gauge:cpu")), "{layout:?}");
    }

    #[test]
    fn a_hidden_module_waits_in_the_tray_as_a_preview_named_with_its_kind() {
        let mut w = world();
        let mut cfg = InstanceCfg { id: "system_monitor-1".into(), widget: "system_monitor".into(), w: 340.0, h: 190.0, ..Default::default() };
        // the normal size without the CPU gauge
        cfg.layout.insert("normal".into(), [("gauges".to_string(), vec!["gauge:ram".to_string()]), ("footer".to_string(), vec!["footer".to_string()])].into());
        w.ws.instances.push(cfg);
        let mut ui = UiState::default();
        ui.selected = Some("system_monitor-1".into());
        ui.act("tier:normal", &ctx(&w), None);
        let (root, _) = ui.build(&ctx(&w), WIN);
        let tile = find_node(&root, "ip/system_monitor-1/tray/gauge:cpu").expect("the CPU gauge has a tile");
        assert_eq!(tile.action.as_deref(), Some("mod:gauge:cpu"), "the tile drags it back");
        let thumb = find_node(tile, "ip/system_monitor-1/tray/gauge:cpu/pv").unwrap();
        assert!(thumb.children.first().is_some_and(|n| n.key.starts_with("tray/system_monitor-1/gauge:cpu") && n.action.is_none()), "the gauge itself, inert");
        let mut shown = Vec::new();
        texts(tile, &mut shown);
        assert!(shown.iter().any(|t| t == "Gauge") && shown.iter().filter(|t| *t == "CPU").count() == 2, "its ring's label and the caption, and its kind: {shown:?}");
    }

    #[test]
    fn an_unsaved_choice_shows_its_default_not_theme_font() {
        let mut w = world();
        w.ws.instances.push(InstanceCfg { id: "drawer-1".into(), widget: "drawer".into(), ..Default::default() });
        let c = ctx(&w);
        let ui = UiState::default();
        assert_eq!(ui.dropdown_label(&c, "p:drawer-1:toggle"), "The arrow button", "the enum's default");
        assert_eq!(ui.dropdown_label(&c, "p:drawer-1:font_header"), "Theme font", "an empty font is the theme's");
        assert_eq!(ui.dropdown_label(&c, "sy:*:font-body"), "Theme font", "no Style font set");
    }

    #[test]
    fn a_long_list_opens_with_a_search_and_enter_picks_the_first_match() {
        let mut w = world();
        w.ws.instances.push(InstanceCfg { id: "drawer-1".into(), widget: "drawer".into(), ..Default::default() });
        let fonts: Vec<String> = ["Arial", "Bahnschrift", "Calibri", "Cambria", "Candara", "Consolas", "Constantia", "Corbel", "Courier New", "Ebrima", "Gabriola", "Georgia", "Impact", "Segoe UI", "Tahoma", "Verdana"].map(String::from).to_vec();
        let c = Ctx { fonts: &fonts, ..ctx(&w) };
        let mut ui = UiState::default();
        ui.scroll.insert("s/ov/list".into(), 5000.0); // left by another, longer list
        ui.act("dd:p:drawer-1:font_header", &c, None);
        assert!(ui.scroll_of("s/ov/list") < 600.0, "opens on its own items, not scrolled past them");
        assert!(ui.wants_caret(), "the search takes the typing");
        for ch in ["c", "a", "m"] {
            assert!(ui.on_key(&Key::Character(ch.into()), Some(ch), &c).is_empty());
        }
        assert_eq!(ui.dropdown_shown(&c, "p:drawer-1:font_header").iter().map(|(v, _)| v.as_str()).collect::<Vec<_>>(), ["Cambria"]);
        let mut shown = Vec::new();
        texts(&ui.build(&c, WIN).0, &mut shown);
        assert!(shown.iter().any(|t| t == "Cambria") && !shown.iter().any(|t| t == "Arial"), "{shown:?}");
        assert_eq!(ui.on_key(&Key::Named(NamedKey::Enter), None, &c), [Cmd::Param("drawer-1".into(), "font_header".into(), Value::Str("Cambria".into()))]);
        assert!(!ui.has_popup() && !ui.wants_caret());
        ui.act("dd:p:drawer-1:toggle", &c, None);
        assert!(!ui.wants_caret(), "two choices need no search");
    }

    #[test]
    fn a_dragged_module_follows_the_pointer_as_itself() {
        let mut w = world();
        w.ws.instances.push(InstanceCfg { id: "system_monitor-1".into(), widget: "system_monitor".into(), w: 340.0, h: 190.0, ..Default::default() });
        let mut ui = UiState::default();
        ui.selected = Some("system_monitor-1".into());
        monitor_frame(&mut ui, &w, WIN);
        monitor_frame(&mut ui, &w, WIN); // the second build knows where the first drew
        let r = ui.preview_rects["mod:gauge:cpu"];
        ui.mod_press("gauge:cpu", (r[0] + 5.0, r[1] + 5.0));
        ui.mod_move((r[0] + 60.0, r[1] + 60.0));
        let (root, _) = ui.build(&ctx(&w), WIN);
        let ghost = find_node(&root, "s/mo/ghost/pv").expect("a preview under the pointer, not a label");
        assert!(ghost.children.first().is_some_and(|n| n.key.starts_with("ghost/system_monitor-1/gauge:cpu")), "{:?}", ghost.children.first().map(|n| &n.key));
        assert!(find_node(&root, "s/mo/from").is_some(), "its place in the preview fades");
    }

    #[test]
    fn advanced_settings_start_collapsed_and_tier_tabs_pick_the_preview() {
        let mut w = world();
        w.ws.instances.push(InstanceCfg { id: "system_monitor-1".into(), widget: "system_monitor".into(), ..Default::default() });
        let c = ctx(&w);
        let mut ui = UiState::default();
        ui.selected = Some("system_monitor-1".into());
        let f = monitor_frame(&mut ui, &w, WIN);
        assert!(f.rect_of("z:system_monitor-1").is_none(), "Layer is under Advanced");
        assert!(ui.act("adv:toggle", &c, None).is_empty() && ui.advanced);
        assert!(ui.act("tier:large", &c, None).is_empty());
        monitor_frame(&mut ui, &w, WIN);
        assert_eq!(ui.preview_arr.borrow().as_ref().map(|a| a.tier.clone()).as_deref(), Some("large"));
        assert_eq!(ui.act("layreset:system_monitor-1|large", &c, None), vec![Cmd::Layout("system_monitor-1".into(), "large".into(), None)]);
    }

    #[test]
    fn general_offers_a_restart_for_the_adapter_choice() {
        let w = world();
        assert_eq!(UiState::default().act("restart", &ctx(&w), None), vec![Cmd::Restart]);
    }

    #[test]
    fn size_limit_toggles_per_widget() {
        let mut w = world();
        let c = ctx(&w);
        assert_eq!(UiState::default().act("lim:clock-1", &c, None), vec![Cmd::SizeLimit("clock-1".into(), false)]);
        w.ws.instances[0].size_limit = false;
        let c = ctx(&w);
        assert_eq!(UiState::default().act("lim:clock-1", &c, None), vec![Cmd::SizeLimit("clock-1".into(), true)]);
    }

    #[test]
    fn a_widget_can_pick_its_own_palette_or_go_back_to_global() {
        let w = world();
        let c = ctx(&w);
        let mut ui = UiState::default();
        let items = ui.dropdown_items(&c, "tp:clock-1:palette");
        assert_eq!(items[0], (String::new(), "Global (Midnight)".to_string()));
        assert_eq!(ui.dropdown_label(&c, "tp:clock-1:palette"), "Global (Midnight)");
        assert_eq!(ui.act("pick:tp:clock-1:palette|Daylight", &c, None), vec![Cmd::ThemePick("clock-1".into(), "palette".into(), Some("Daylight".into()))]);
        assert_eq!(ui.act("pick:tp:clock-1:palette|", &c, None), vec![Cmd::ThemePick("clock-1".into(), "palette".into(), None)]);
    }

    #[test]
    fn reset_style_on_a_widget_clears_its_overrides_and_its_own_theme() {
        let mut w = world();
        w.ws.instances[0].style.insert("blur".into(), serde_json::json!(true));
        w.ws.instances[0].theme.palette = Some("Daylight".into());
        let c = ctx(&w);
        let cmds = UiState::default().act("syreset:clock-1|*", &c, None);
        assert_eq!(cmds, vec![Cmd::Style(Scope::Instance("clock-1".into()), "blur".into(), None), Cmd::ThemePick("clock-1".into(), "palette".into(), None)]);
    }

    #[test]
    fn style_toggles_and_resets_at_both_scopes() {
        let mut w = world();
        w.ws.style.insert("blur".into(), serde_json::json!(true));
        let c = ctx(&w);
        let mut ui = UiState::default();
        assert_eq!(ui.act("sy:*|outlines", &c, None), vec![Cmd::Style(Scope::Global, "outlines".into(), Some(Value::Bool(false)))]);
        assert_eq!(ui.act("sy:clock-1|blur", &c, None), vec![Cmd::Style(Scope::Instance("clock-1".into()), "blur".into(), Some(Value::Bool(false)))], "the instance starts from the inherited global value");
        assert_eq!(ui.act("syreset:*|blur", &c, None), vec![Cmd::Style(Scope::Global, "blur".into(), None)]);
        assert_eq!(ui.act("syreset:*|*", &c, None), vec![Cmd::Style(Scope::Global, "blur".into(), None)], "reset all resets what is set");
        assert_eq!(ui.dropdown_items(&c, "sy:*:anim-speed")[0], ("off".to_string(), "Off".to_string()));
        assert_eq!(ui.act("pick:sy:*:anim-speed|off", &c, None), vec![Cmd::Style(Scope::Global, "anim-speed".into(), Some(Value::Str("off".into())))]);
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
        ui.focus_input(&c, "hx:sy:*:accent", Some(0));
        // typing into a field that already holds a colour stays invalid until it parses
        ui.focus.as_mut().unwrap().text = String::new();
        assert_eq!(ui.on_key(&Key::Character("z".into()), Some("z"), &c), vec![], "'z' is not a hex digit");
        ui.focus.as_mut().unwrap().text = "ff8800".into();
        ui.focus.as_mut().unwrap().caret = 6;
        let cmds = ui.on_key(&Key::Named(NamedKey::Backspace), None, &c);
        assert_eq!(cmds, vec![], "ff880 is five digits: invalid");
        let cmds = ui.on_key(&Key::Character("0".into()), Some("0"), &c);
        assert_eq!(cmds, vec![Cmd::Style(Scope::Global, "accent".into(), Some(Value::Str("#ff8800".into())))]);
    }

    #[test]
    fn colour_picker_emits_a_command_per_pick() {
        let w = world();
        let c = ctx(&w);
        let mut ui = UiState::default();
        ui.act("cp:sy:clock-1:accent", &c, None);
        let cmds = ui.act(&format!("cpsv:{}:0", SV_N - 1), &c, None); // full saturation, full value
        let Cmd::Style(Scope::Instance(id), name, Some(Value::Str(hex))) = &cmds[0] else { panic!("{cmds:?}") };
        assert_eq!((id.as_str(), name.as_str()), ("clock-1", "accent"));
        assert!(Color::parse(hex).is_some());
        assert_eq!(ui.act("cpset:#00ff00", &c, None), vec![Cmd::Style(Scope::Instance("clock-1".into()), "accent".into(), Some(Value::Str("#00ff00".into())))]);
    }

    fn texts(n: &Node, out: &mut Vec<String>) {
        if let Kind::Text(t) = &n.kind {
            out.push(t.text.clone());
        }
        n.children.iter().for_each(|c| texts(c, out));
    }

    #[test]
    fn plugin_remove_needs_two_clicks_and_names_its_instances() {
        let mut w = world();
        w.ws.instances.push(InstanceCfg { id: "weather-1".into(), widget: "weather".into(), ..Default::default() });
        let c = ctx(&w);
        let mut ui = UiState::default();
        ui.act("nav:plugins", &c, None);
        assert_eq!(ui.act("prm:sunset", &c, None), vec![], "the first click only asks");
        let mut shown = Vec::new();
        texts(&ui.build(&c, WIN).0, &mut shown);
        assert!(shown.iter().any(|t| t.contains("weather-1")), "{shown:?}");
        assert!(!shown.iter().any(|t| t.contains("clock-1")), "a built-in widget stays");
        assert_eq!(ui.act("prm:sunset", &c, None), vec![Cmd::RemovePlugin("sunset".into())]);
        ui.act("prm:sunset", &c, None);
        assert_eq!(ui.act("pon:sunset", &c, None).len(), 1, "any other action disarms it");
        assert_eq!(ui.act("prm:sunset", &c, None), vec![]);
        ui.act("del:clock-1", &c, None);
        assert_eq!(ui.act("prm:sunset", &c, None), vec![], "a widget's remove never confirms a plugin's");
    }

    #[test]
    fn plugin_toggle_flips_enabled() {
        let w = world();
        let c = ctx(&w);
        let mut ui = UiState::default();
        assert_eq!(ui.act("pon:sunset", &c, None), vec![Cmd::PluginEnabled("sunset".into(), false)]);
        assert_eq!(ui.act("pon:broken", &c, None), vec![Cmd::PluginEnabled("broken".into(), true)]);
        assert_eq!(ui.act("pon:nope", &c, None), vec![]);
        assert_eq!(ui.act("pfolder", &c, None), vec![Cmd::OpenPluginsFolder]);
    }

    #[test]
    fn dropping_a_plugin_file_installs_it() {
        let mut ui = UiState::default();
        ui.drop_hover = true;
        assert_eq!(ui.dropped(Path::new("C:\\Downloads\\sunset.wfplugin")), vec![Cmd::InstallPlugin("C:\\Downloads\\sunset.wfplugin".into())]);
        assert_eq!((ui.page, ui.drop_hover), (Page::Plugins, false));
        let w = world();
        let c = ctx(&w);
        ui.drop_hover = true;
        let mut shown = Vec::new();
        texts(&ui.build(&c, WIN).0, &mut shown);
        assert!(shown.iter().any(|t| t == "Drop to install"));
    }

    #[test]
    fn a_hidden_instance_says_why() {
        let mut w = world();
        w.hidden = vec![("clock-1".into(), Hidden::PluginOff("Sunset".into())), ("icon_folder-1".into(), Hidden::Parked)];
        let c = ctx(&w);
        let mut shown = Vec::new();
        texts(&UiState::default().build(&c, WIN).0, &mut shown);
        assert!(shown.iter().any(|t| t.contains("plugin Sunset is off")), "{shown:?}");
        assert!(shown.iter().any(|t| t.contains("monitor missing")));
    }

    #[test]
    fn every_page_builds_with_unique_keys() {
        fn walk(n: &Node, seen: &mut std::collections::HashSet<String>, what: &str) {
            assert!(seen.insert(n.key.clone()), "duplicate node key `{}` on {what}: hover, animation and text state would collide", n.key);
            n.children.iter().for_each(|c| walk(c, seen, what));
        }
        let mut w = world();
        // a Widget only the Sunset plugin provides, so Add a widget shows its plugin's badge
        let dir = std::env::temp_dir().join(format!("wf-settings-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("weather.toml"), "name = 'Weather'\ndescription = 'Rain or shine.'\n[root]\ntype = 'box'").unwrap();
        w.reg.load_dir(&dir);
        let mut screens: Vec<(String, UiState)> = Vec::new();
        for page in Page::ALL {
            let mut ui = UiState::default();
            ui.page = page;
            ui.selected = Some("icon_folder-1".into());
            screens.push((format!("{page:?}"), ui));
        }
        let mut gallery = UiState::default();
        gallery.adding = true;
        screens.push(("Add a widget".into(), gallery));
        for (what, ui) in &screens {
            walk(&ui.build(&ctx(&w), WIN).0, &mut Default::default(), what);
        }
        w.ws.onboarded = false;
        for step in 0..SETUP_STEPS {
            let mut ui = UiState::default();
            ui.step = step;
            walk(&ui.build(&ctx(&w), WIN).0, &mut Default::default(), &format!("setup step {step}"));
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn add_a_widget_shows_each_widget_live_or_its_icon_when_it_cannot_run() {
        let dir = std::env::temp_dir().join(format!("wf-settings-gallery-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("agents.toml"), "name = 'Agents'\nneeds = ['agents']\n[root]\ntype = 'box'").unwrap();
        let mut w = world();
        w.reg.load_dir(&dir);
        let mut ui = UiState::default();
        ui.adding = true;
        let (root, _) = ui.build(&ctx(&w), WIN);
        let top = find_node(&root, "w/g/grid/c/clock/top").unwrap();
        let clock = find_node(top, "w/g/grid/c/clock/pv").expect("the clock itself, not an icon");
        fn clickable(n: &Node) -> bool {
            n.action.is_some() || n.children.iter().any(clickable)
        }
        assert!(!clickable(clock), "a preview takes no clicks");
        let agents = find_node(&root, "w/g/grid/c/agents/top").unwrap();
        assert!(find_node(agents, "w/g/grid/c/agents/pv").is_none() && find_node(agents, "w/g/grid/c/agents/ic").is_some(), "no `agents` source: its icon");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_fresh_install_walks_through_setup_and_can_skip_it() {
        let mut w = world();
        w.ws.onboarded = false;
        let mut ui = UiState::default();
        let shown = |ui: &UiState, w: &World| {
            let mut t = Vec::new();
            texts(&ui.build(&ctx(w), WIN).0, &mut t);
            t
        };
        assert!(shown(&ui, &w).iter().any(|t| t == "Step 1 of 4"), "setup replaces the pages");
        for _ in 0..5 {
            assert!(ui.act("ob:next", &ctx(&w), None).is_empty());
        }
        assert!(shown(&ui, &w).iter().any(|t| t == "Step 4 of 4"), "Continue stops at the last step");
        ui.act("ob:back", &ctx(&w), None);
        assert!(shown(&ui, &w).iter().any(|t| t == "Put a few widgets out"));
        ui.page = Page::Log;
        assert_eq!(ui.act("ob:done", &ctx(&w), None), [Cmd::Onboarded], "Skip setup and Finish both end it");
        w.ws.onboarded = true;
        assert!(ui.page == Page::Widgets && shown(&ui, &w).iter().any(|t| t == "Edit layout"), "then the Widgets page");
    }

    #[test]
    fn add_a_widget_filters_by_category_and_search() {
        let w = world();
        let c = ctx(&w);
        let cards = |ui: &UiState| {
            let mut ids = Vec::new();
            fn find(n: &Node, out: &mut Vec<String>) {
                if let Some(id) = n.action.as_deref().and_then(|a| a.strip_prefix("add:")) {
                    out.push(id.to_string());
                }
                n.children.iter().for_each(|c| find(c, out));
            }
            find(&ui.build(&c, WIN).0, &mut ids);
            ids
        };
        let mut ui = UiState::default();
        assert!(cards(&ui).is_empty(), "a selected widget's panel, not the gallery");
        ui.act("gallery:open", &c, None);
        assert_eq!(cards(&ui).len(), w.reg.ids().len());
        assert_eq!(&cards(&ui)[..2], ["clock", "digital_clock"], "Time comes first");
        ui.act("cat:Launchers", &c, None);
        assert_eq!(cards(&ui), ["drawer", "icon_folder", "icon_list"]);
        ui.act("cat:", &c, None);
        ui.focus_input(&c, "q:widgets", None);
        for ch in ["t", "r", "a", "y"] {
            assert!(ui.on_key(&Key::Character(ch.into()), Some(ch), &c).is_empty(), "typing a search changes nothing on the desktop");
        }
        assert_eq!(cards(&ui), ["drawer"], "matches the description: a pull-out tray");
        assert_eq!(ui.act("add:drawer", &c, None), [Cmd::Add("drawer".into())]);
        assert!(!ui.adding && ui.selected.as_deref() == Some("drawer-1"), "adding one shows it");
    }

    #[test]
    fn the_log_filters_by_level_and_links_a_gpu_warning() {
        let mut w = world();
        let line = |level, source: &str, text: &str| LogLine { time: "01:29:58".into(), level, source: source.into(), text: text.into() };
        let log = vec![line(Level::Info, "core", "ready"), line(Level::Warning, "gpu", "Using the software adapter"), line(Level::Error, "plugins", "could not install x")];
        w.ws.instances.clear();
        let c = Ctx { log: &log, ..ctx(&w) };
        let mut ui = UiState::default();
        ui.act("nav:log", &c, None);
        assert_eq!(ui.log_rows(&c).len(), 3);
        ui.act("lvl:warning", &c, None);
        assert_eq!(ui.log_rows(&c).iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Using the software adapter"]);
        let mut shown = Vec::new();
        texts(&ui.build(&c, WIN).0, &mut shown);
        assert!(shown.iter().any(|t| t.starts_with("Change adapter in General")), "{shown:?}");
        ui.act("lvl:all", &c, None);
        ui.queries.insert("q:log".into(), "PLUGINS".into());
        assert_eq!(ui.log_rows(&c).len(), 1, "search matches the source too, ignoring case");
        assert_eq!(ui.act("openlog", &c, None), [Cmd::OpenData("wayfinder.log")]);
    }

    #[test]
    fn accent_presets_and_the_cpu_fix_are_one_click() {
        let w = world();
        let c = ctx(&w);
        let mut ui = UiState::default();
        assert_eq!(ui.act("setc:sy:*:accent|#6b8cff", &c, None), [Cmd::Style(Scope::Global, "accent".into(), Some(Value::Str("#6b8cff".into())))]);
        assert_eq!(ui.act("gpufix", &c, None), [Cmd::Gpu("low".into()), Cmd::Restart]);
        assert_eq!(split_unit("Corner roundness (px)"), ("Corner roundness", "px"));
        assert_eq!((with_unit(15.0, "px"), with_unit(60.0, "%"), with_unit(8.0, "")), ("15 px".to_string(), "60%".to_string(), "8".to_string()));
        assert_eq!(gpu_parts("Microsoft Basic Render Driver / Dx12 / Cpu / alpha PreMultiplied / present Mailbox"), ["Microsoft Basic Render Driver", "DX12", "CPU", "Mailbox"]);
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
        assert_eq!(ui.slider_spec(&c, "sy:*:radius-lg"), Some((0.0, 40.0, 1.0, 22.0)));
        assert_eq!(UiState::slider_cmd("sy:*:radius-lg", 30.0), Some(Cmd::Style(Scope::Global, "radius-lg".into(), Some(Value::Num(30.0)))));
    }
}
