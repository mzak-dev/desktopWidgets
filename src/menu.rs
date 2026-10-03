//! The tray menu, drawn by the engine like Settings: one popup window, made on the first
//! right-click and reused, sized for the tallest menu so expanding never resizes it (ADR-006).

use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::{Window, WindowAttributes};

use crate::anim::{self, Anim, Ease};
use crate::color::Color;
use crate::gfx::{Gpu, Power, RenderError, Target};
use crate::platform::win32::{self, ZMode};
use crate::settings::{EDIT_KEYS, FG, Kit, LINE, MUTED, SURFACE};
use crate::text::TextEngine;
use crate::theme::Theme;
use crate::ui::{self, Env, Frame, Node};
use crate::workspace::MonitorInfo;

const CARD_W: f32 = 252.0;
const ROW: f32 = 32.0;
const SEP: f32 = 9.0;
const PAD: f32 = 6.0;
/// Transparent room around the card for its shadow.
const GUTTER: f32 = 24.0;
/// Logical px kept between the card and the screen's work area.
const EDGE: f32 = 8.0;
/// The Workspace row: the one that expands.
const WS_ROW: usize = 2;
const WS_ID: &str = "wsexp";

/// What the menu shows of the app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Data {
    pub workspaces: Vec<String>,
    pub active: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum RowKind {
    Plain,
    /// Expands the Workspace list.
    Header,
    /// Under the header, indented; ticked when it is the one on screen.
    Sub(bool),
}

#[derive(Clone, Debug, PartialEq)]
struct Row {
    /// The action id `App` already knows (`edit`, `ws:1`...).
    id: String,
    glyph: &'static str,
    label: String,
    hint: String,
    /// A separator above it.
    sep: bool,
    kind: RowKind,
}

fn row(id: &str, glyph: &'static str, label: &str, sep: bool) -> Row {
    Row { id: id.into(), glyph, label: label.into(), hint: String::new(), sep, kind: RowKind::Plain }
}

fn rows(d: &Data, expanded: bool) -> Vec<Row> {
    let mut v = vec![
        Row { hint: EDIT_KEYS.join("+"), ..row("edit", "edit", "Edit layout", false) },
        row("settings", "gear", "Settings…", false),
        Row { kind: RowKind::Header, ..row(WS_ID, "grid", &format!("Workspace: {}", d.active), false) },
    ];
    if expanded {
        for (i, w) in d.workspaces.iter().enumerate() {
            v.push(Row { kind: RowKind::Sub(*w == d.active), ..row(&format!("ws:{i}"), "", w, false) });
        }
        v.push(Row { kind: RowKind::Sub(false), ..row("wsmanage", "", "Manage workspaces…", false) });
    }
    v.push(row("reload", "refresh", "Reload widgets and themes", true));
    v.push(row("folder", "folder", "Open widgets folder", false));
    v.push(row("quit", "power", "Quit Wayfinder", true));
    v
}

fn row_h(r: &Row) -> f32 {
    ROW + if r.sep { SEP } else { 0.0 }
}

/// Where a row's top is inside the card's content box.
fn row_y(rows: &[Row], i: usize) -> f32 {
    rows[..i].iter().map(row_h).sum::<f32>() + if rows[i].sep { SEP } else { 0.0 }
}

fn card_h(rows: &[Row]) -> f32 {
    rows.iter().map(row_h).sum::<f32>() + 2.0 * PAD
}

/// The window's logical size: the card at its tallest, plus the gutter.
fn window_size(d: &Data) -> (f32, f32) {
    (CARD_W + 2.0 * GUTTER, card_h(&rows(d, true)) + 2.0 * GUTTER)
}

#[derive(Debug, PartialEq)]
enum Out {
    Pick(String),
    Close,
}

/// What the user has done to the menu; no window in it, so it can be tested.
#[derive(Debug, Default)]
struct State {
    expanded: bool,
    /// The row under the pointer or the keyboard: they share one.
    cursor: Option<usize>,
    /// Where the highlight last was, so it fades out in place and glides from there.
    pill: f32,
}

impl State {
    fn step(&mut self, n: usize, dir: i32) {
        self.cursor = Some(match self.cursor {
            None if dir > 0 => 0,
            None => n - 1,
            Some(c) => (c as i32 + dir).rem_euclid(n as i32) as usize,
        });
    }

    fn activate(&mut self, d: &Data, i: usize) -> Option<Out> {
        let r = rows(d, self.expanded);
        let r = r.get(i)?;
        if r.id == WS_ID {
            self.expanded = !self.expanded;
            self.cursor = Some(i);
            return None;
        }
        Some(Out::Pick(r.id.clone()))
    }

    fn key(&mut self, d: &Data, key: &Key) -> Option<Out> {
        let n = rows(d, self.expanded).len();
        match key {
            Key::Named(NamedKey::Escape) => return Some(Out::Close),
            Key::Named(NamedKey::ArrowDown) => self.step(n, 1),
            Key::Named(NamedKey::ArrowUp) => self.step(n, -1),
            Key::Named(NamedKey::Enter | NamedKey::Space) => return self.cursor.and_then(|i| self.activate(d, i)),
            Key::Named(NamedKey::ArrowRight) if self.cursor == Some(WS_ROW) => self.expanded = true,
            Key::Named(NamedKey::ArrowLeft) if self.expanded && self.cursor.is_some_and(|c| c >= WS_ROW) => {
                self.expanded = false;
                self.cursor = Some(WS_ROW);
            }
            _ => {}
        }
        None
    }

    /// Remembers the highlighted row's place.
    fn track(&mut self, rows: &[Row]) {
        if let Some(c) = self.cursor.filter(|c| *c < rows.len()) {
            self.pill = row_y(rows, c);
        }
    }
}

fn build(theme: &Theme, rows: &[Row], st: &State, bottom: bool, closing: bool, size: (f32, f32)) -> Node {
    let k = &Kit::new(theme);
    let (accent, dim, text) = (k.accent(), MUTED, FG);
    let dy = if bottom { 1.0 } else { -1.0 };
    let radius = (theme.num("radius-lg") * 0.55).clamp(4.0, 14.0);
    let pill_a = if closing { 0.34 } else if st.cursor.is_some() { 0.14 } else { 0.0 };
    // first, so the rows draw over it; it glides to the cursor's row, then fades where it stopped
    let mut card = Node::new("m/c")
        .col()
        .w(CARD_W)
        .no_shrink()
        .pad(PAD)
        .radius(radius)
        .fill(SURFACE.with_alpha(0.98))
        .border(1.0, LINE)
        .shadow(18.0, 6.0, Color([0.0, 0.0, 0.0, 0.45]))
        .clip()
        .enter(180, 8.0 * dy, 0)
        // the offset must exist every frame, or closing would jump instead of drift
        .offset(0.0, if closing { 6.0 * dy } else { 0.0 })
        .opacity(if closing { 0.0 } else { 1.0 })
        .transition(if closing { 120 } else { 140 }, if closing { Ease::In } else { Ease::Out })
        .child(Node::new("m/pill").abs(Some(PAD), Some(PAD), None, None).wh(CARD_W - 2.0 * PAD, ROW).radius(radius.min(8.0)).fill(accent.with_alpha(pill_a)).offset(0.0, st.pill).transition(140, Ease::Out));
    let mut sub = 0u32;
    for (i, r) in rows.iter().enumerate() {
        if r.sep {
            card = card.child(Node::new(format!("m/sep/{}", r.id)).h(SEP).no_shrink().pad_xy(8.0, 4.0).child(Node::new(format!("m/sep/{}/l", r.id)).h(1.0).fill(LINE)));
        }
        let indent = matches!(r.kind, RowKind::Sub(_));
        // the top rows build outward from the taskbar; the Workspace rows follow the one that opened them
        let delay = if indent {
            sub += 1;
            30 + 24 * sub
        } else {
            let from_edge = if bottom { rows.len() - 1 - i } else { i };
            14 * from_edge as u32
        };
        let key = format!("m/r/{}", r.id);
        let lead = if indent { Node::new(format!("{key}/g")).w(16.0).no_shrink() } else { k.glyph(format!("{key}/g"), r.glyph, 14.0, dim).no_shrink() };
        let mut row = Node::new(&key)
            .row()
            .h(ROW)
            .no_shrink()
            .align(taffy::AlignItems::CENTER)
            .pad_xy(10.0, 0.0)
            .gap(10.0)
            .on(r.id.clone())
            .enter(150, 4.0 * dy, delay)
            .child(lead)
            .child(k.txt(format!("{key}/t"), &r.label, 13.0, text).grow(1.0));
        match r.kind {
            RowKind::Plain if !r.hint.is_empty() => row = row.child(k.txt(format!("{key}/h"), &r.hint, 11.0, dim)),
            // a new key per state replays its enter: the chevron turns over by fading in
            RowKind::Header => row = row.child(k.glyph(format!("{key}/c{}", st.expanded), if st.expanded { "chevron-down" } else { "chevron-right" }, 11.0, dim).enter(140, 0.0, 0)),
            RowKind::Sub(true) => row = row.child(k.glyph(format!("{key}/ok"), "check", 11.0, accent)),
            _ => {}
        }
        card = card.child(row);
    }
    let stage = Node::new("m/s").abs_fill().col().pad(GUTTER).justify(if bottom { taffy::JustifyContent::FLEX_END } else { taffy::JustifyContent::FLEX_START }).child(card);
    Node::new("m").wh(size.0, size.1).child(stage)
}

/// The tree and window size for a state, for `examples/render_settings.rs` to draw to a PNG.
pub fn preview(theme: &Theme, d: &Data, expanded: bool, cursor: Option<usize>) -> (Node, (f32, f32)) {
    let r = rows(d, expanded);
    let mut st = State { expanded, cursor, pill: 0.0 };
    st.track(&r);
    let size = window_size(d);
    (build(theme, &r, &st, true, false, size), size)
}

pub struct MenuWin {
    pub window: Arc<Window>,
    target: Target,
    anim: Anim,
    st: State,
    data: Data,
    frame: Option<Frame>,
    mouse: (f32, f32),
    /// The card grows upward from the taskbar below it, or downward from one above.
    bottom: bool,
    open: bool,
    closing: bool,
    redraw: bool,
    animating: bool,
    last: Instant,
}

impl MenuWin {
    pub fn new(el: &ActiveEventLoop, gpu: &mut Option<Gpu>, power: Power) -> Result<MenuWin, String> {
        let (w, h) = window_size(&Data::default());
        let attrs = WindowAttributes::default()
            .with_title("Wayfinder menu")
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_visible(false)
            .with_skip_taskbar(true)
            .with_inner_size(LogicalSize::new(w as f64, h as f64))
            // ADR-001: DirectComposition presents beneath the GDI redirection bitmap.
            .with_no_redirection_bitmap(true);
        let window = Arc::new(el.create_window(attrs).map_err(|e| e.to_string())?);
        let target = match gpu.as_mut() {
            Some(g) => g.target_for(&window)?,
            None => {
                let (g, t) = Gpu::new(&window, power)?;
                *gpu = Some(g);
                t
            }
        };
        Ok(MenuWin { window, target, anim: Anim::default(), st: State::default(), data: Data::default(), frame: None, mouse: (-1.0, -1.0), bottom: true, open: false, closing: false, redraw: false, animating: false, last: Instant::now() })
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The Workspaces changed while it may be open.
    pub fn set_data(&mut self, d: Data) {
        if self.data != d {
            self.data = d;
            self.redraw = true;
        }
    }

    /// Shows it with the card's corner nearest `at` (the tray click, physical px) on `mon`'s work area.
    pub fn show(&mut self, at: PhysicalPosition<f64>, mon: Option<&MonitorInfo>, gpu: &mut Gpu, text: &mut TextEngine, theme: &Theme) {
        let s = mon.map_or(self.window.scale_factor(), |m| m.scale);
        let (lw, lh) = window_size(&self.data);
        let (pw, ph) = ((lw as f64 * s).ceil() as i32, (lh as f64 * s).ceil() as i32);
        let (wx, wy, ww, wh) = mon.map_or((i32::MIN / 2, i32::MIN / 2, i32::MAX as u32, i32::MAX as u32), |m| m.work);
        let (wx, wy, ww, wh) = (wx as f64, wy as f64, ww as f64, wh as f64);
        let (gut, edge) = (GUTTER as f64 * s, EDGE as f64 * s);
        self.bottom = at.y > wy + wh / 2.0;
        // centred on the click, then pulled inside the work area
        let x = (at.x - pw as f64 / 2.0).min(wx + ww - pw as f64 + gut - edge).max(wx - gut + edge);
        let y = if self.bottom { at.y.min(wy + wh) - edge + gut - ph as f64 } else { at.y.max(wy) + edge - gut };
        if let Some(h) = win32::hwnd_of(&self.window) {
            win32::set_rect(h, x.round() as i32, y.round() as i32, pw, ph);
        }
        // the scale may have changed with the monitor
        let _ = self.window.request_inner_size(LogicalSize::new(lw as f64, lh as f64));
        self.st = State::default();
        self.st.pill = 0.0;
        self.anim = Anim::default(); // every row replays its entrance
        self.closing = false;
        self.open = true;
        self.render(gpu, text, theme);
        self.window.set_visible(true);
        if let Some(h) = win32::hwnd_of(&self.window) {
            win32::set_zmode(h, ZMode::Topmost);
        }
        // winit rebuilds WS_EX_* on a state change: a tool window we can still focus
        win32::set_no_activate(&self.window, false);
        self.window.focus_window();
    }

    fn begin_close(&mut self) {
        if self.open && !self.closing {
            self.closing = true;
            self.redraw = true;
        }
    }

    fn logical(&self, p: PhysicalPosition<f64>) -> (f32, f32) {
        let s = self.window.scale_factor();
        ((p.x / s) as f32, (p.y / s) as f32)
    }

    /// The action id picked, for `App` to run.
    pub fn event(&mut self, ev: &WindowEvent) -> Option<String> {
        if !self.open {
            return None;
        }
        let mut out = None;
        match ev {
            WindowEvent::Focused(false) | WindowEvent::CloseRequested => self.begin_close(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => self.redraw = true,
            _ if self.closing => {}
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse = self.logical(*position);
                let id = self.frame.as_ref().and_then(|f| f.hit_at(self.mouse.0, self.mouse.1)).and_then(|h| h.action.clone());
                let at = id.and_then(|id| rows(&self.data, self.st.expanded).iter().position(|r| r.id == id));
                if at != self.st.cursor {
                    self.st.cursor = at;
                    self.redraw = true;
                }
            }
            WindowEvent::CursorLeft { .. } => {
                self.st.cursor = None;
                self.redraw = true;
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                let at = self.st.cursor.filter(|_| self.frame.as_ref().is_some_and(|f| f.hit_at(self.mouse.0, self.mouse.1).is_some_and(|h| h.action.is_some())));
                out = at.and_then(|i| self.st.activate(&self.data, i));
                self.redraw = true;
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                out = self.st.key(&self.data, &event.logical_key);
                self.redraw = true;
            }
            _ => {}
        }
        match out {
            Some(Out::Pick(id)) => {
                self.begin_close();
                Some(id)
            }
            Some(Out::Close) => {
                self.begin_close();
                None
            }
            None => None,
        }
    }

    pub fn render(&mut self, gpu: &mut Gpu, text: &mut TextEngine, theme: &Theme) {
        if !self.open {
            return;
        }
        let now = Instant::now();
        self.anim.duration_factor = anim::duration_factor(&theme.str("anim-speed"));
        let s = self.window.scale_factor() as f32;
        let phys = self.window.inner_size();
        gpu.fit(&mut self.target, phys.width, phys.height);
        let size = (phys.width as f32 / s, phys.height as f32 / s);
        let rows = rows(&self.data, self.st.expanded);
        self.st.track(&rows);
        let root = build(theme, &rows, &self.st, self.bottom, self.closing, size);
        let mut env = Env { text, anim: &mut self.anim, hover: None, now, scale: s };
        let frame = ui::layout(&root, size, &mut env);
        match gpu.render(&mut self.target, &frame.list, text) {
            Ok(()) => {}
            Err(RenderError::Skip(e)) | Err(RenderError::Lost(e)) => eprintln!("wayfinder: menu render: {e}"),
        }
        self.animating = frame.animating;
        self.frame = Some(frame);
        self.redraw = false;
        self.last = now;
        if self.closing && !self.animating {
            self.open = false;
            self.closing = false;
            self.window.set_visible(false);
        }
    }

    pub fn next_frame(&self, now: Instant) -> Option<Instant> {
        if !self.open {
            return None;
        }
        if self.redraw {
            return Some(now);
        }
        self.animating.then(|| self.last + Duration::from_millis(16))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> Data {
        Data { workspaces: vec!["Main".into(), "Work".into()], active: "Work".into() }
    }

    fn ids(d: &Data, expanded: bool) -> Vec<String> {
        rows(d, expanded).into_iter().map(|r| r.id).collect()
    }

    #[test]
    fn collapsed_hides_the_workspaces_and_expanded_ticks_the_active_one() {
        let d = data();
        assert_eq!(ids(&d, false), ["edit", "settings", "wsexp", "reload", "folder", "quit"]);
        let open = rows(&d, true);
        assert_eq!(open.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["edit", "settings", "wsexp", "ws:0", "ws:1", "wsmanage", "reload", "folder", "quit"]);
        assert_eq!((open[3].kind, open[4].kind), (RowKind::Sub(false), RowKind::Sub(true)));
        assert_eq!(open[WS_ROW].id, WS_ID);
    }

    #[test]
    fn the_window_fits_the_expanded_card() {
        let d = data();
        assert_eq!(window_size(&d).1, card_h(&rows(&d, true)) + 2.0 * GUTTER);
        assert!(card_h(&rows(&d, true)) > card_h(&rows(&d, false)));
        let r = rows(&d, false);
        assert_eq!(row_y(&r, 3), 3.0 * ROW + SEP, "the separator above Reload counts in its place");
    }

    #[test]
    fn arrows_wrap_enter_picks_and_the_header_expands_without_closing() {
        let d = data();
        let mut st = State::default();
        let down = Key::Named(NamedKey::ArrowDown);
        assert_eq!(st.key(&d, &Key::Named(NamedKey::ArrowUp)), None);
        assert_eq!(st.cursor, Some(5), "up from nothing starts at the last row");
        st.key(&d, &down);
        assert_eq!(st.cursor, Some(0), "and down from the last wraps");
        assert_eq!(st.key(&d, &Key::Named(NamedKey::Enter)), Some(Out::Pick("edit".into())));
        st.key(&d, &down);
        st.key(&d, &down);
        assert_eq!(st.cursor, Some(WS_ROW));
        assert_eq!(st.key(&d, &Key::Named(NamedKey::Enter)), None);
        assert!(st.expanded, "Enter on the header opens it and the menu stays");
        st.key(&d, &down);
        st.key(&d, &down);
        assert_eq!(st.key(&d, &Key::Named(NamedKey::Space)), Some(Out::Pick("ws:1".into())));
    }

    #[test]
    fn right_opens_left_closes_and_escape_leaves() {
        let d = data();
        let mut st = State { cursor: Some(WS_ROW), ..Default::default() };
        st.key(&d, &Key::Named(NamedKey::ArrowRight));
        assert!(st.expanded);
        st.cursor = Some(4);
        st.key(&d, &Key::Named(NamedKey::ArrowLeft));
        assert_eq!((st.expanded, st.cursor), (false, Some(WS_ROW)), "back on the header, not on a row that is gone");
        assert_eq!(st.key(&d, &Key::Named(NamedKey::Escape)), Some(Out::Close));
    }

    #[test]
    fn the_highlight_stays_where_it_was_when_the_pointer_leaves() {
        let d = data();
        let r = rows(&d, false);
        let mut st = State { cursor: Some(3), ..Default::default() };
        st.track(&r);
        let at = st.pill;
        st.cursor = None;
        st.track(&r);
        assert_eq!(st.pill, at);
    }

    #[test]
    fn every_row_is_in_the_tree_with_its_action() {
        fn find<'a>(n: &'a Node, key: &str) -> Option<&'a Node> {
            if n.key == key { Some(n) } else { n.children.iter().find_map(|c| find(c, key)) }
        }
        let theme = Theme::compose(&crate::theme::Library::load(std::path::Path::new("no-such-dir")), &crate::theme::Selection::default(), &[]);
        let d = data();
        for expanded in [false, true] {
            let st = State { expanded, ..Default::default() };
            let r = rows(&d, expanded);
            let root = build(&theme, &r, &st, true, false, window_size(&d));
            for row in &r {
                let n = find(&root, &format!("m/r/{}", row.id)).unwrap_or_else(|| panic!("{} missing", row.id));
                assert_eq!(n.action.as_deref(), Some(row.id.as_str()));
            }
        }
    }
}
