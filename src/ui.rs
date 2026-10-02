//! Node tree -> taffy layout -> draw list + hit regions, shared by widgets and
//! the settings window (decision 9).

use std::time::Instant;

use taffy::prelude::*;

use crate::anim::{Anim, Ease, Pic};
use crate::color::Color;
use crate::draw::*;
use crate::elements::{ShapeCx, rgba_with_opacity};
use crate::text::{RunInfo, TextEngine};
use crate::textspec::TextSpec;

pub use crate::elements::{ArcSpec, Fit, HandSpec, ImageSpec, Kind, TicksSpec};

/// In card units from a Widget, window units after `Card::expand_in_window_units`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExpandInfo {
    pub active: bool,
    pub width: Option<f32>,
    pub height: Option<f32>,
}

#[derive(Clone, Copy, Debug)]
pub struct Shadow {
    pub blur: f32,
    pub dy: f32,
    pub color: Color,
}

#[derive(Clone, Copy, Debug)]
pub struct Look {
    pub fill: Color,
    pub gradient_bottom: Option<Color>,
    pub border: f32,
    pub border_color: Color,
    pub radius: f32,
    pub opacity: f32,
    pub shadow: Option<Shadow>,
}

impl Default for Look {
    fn default() -> Self {
        Self { fill: Color::default(), gradient_bottom: None, border: 0.0, border_color: Color::default(), radius: 0.0, opacity: 1.0, shadow: None }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Hover {
    pub fill: Option<Color>,
    pub border_color: Option<Color>,
    pub opacity: Option<f32>,
    pub text_color: Option<Color>,
}

impl Hover {
    pub fn any(&self) -> bool {
        self.fill.is_some() || self.border_color.is_some() || self.opacity.is_some() || self.text_color.is_some()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Transition {
    pub ms: u32,
    pub ease: Ease,
}

#[derive(Clone, Copy, Debug)]
pub struct Enter {
    pub ms: u32,
    pub dy: f32,
    pub delay: u32,
}

#[derive(Clone, Debug)]
pub struct Node {
    /// Hierarchical ("0/2/1"); keys hover, animation and text state.
    pub key: String,
    pub style: Style,
    pub kind: Kind,
    pub look: Look,
    pub hover: Hover,
    pub action: Option<String>,
    pub transition: Transition,
    pub enter: Option<Enter>,
    /// Makes it a clipping, vertically scrolling container.
    pub scroll_offset: Option<f32>,
    /// Makes it a clipping, sideways scrolling container.
    pub scroll_offset_x: Option<f32>,
    /// A file dropped on it runs this action with the file's path after it.
    pub on_drop: Option<String>,
    /// Dragging across it sets `state.slide` (0-1) and `state.sliding`; letting go runs this
    /// action with the fraction after it.
    pub on_slide: Option<String>,
    pub clip: bool,
    pub overlay: bool,
    pub hit_testable: bool,
    pub offset: Option<(f32, f32)>,
    pub children: Vec<Node>,
}

impl Node {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            style: Style { display: Display::Flex, ..Default::default() },
            kind: Kind::Box,
            look: Look::default(),
            hover: Hover::default(),
            action: None,
            transition: Transition::default(),
            enter: None,
            scroll_offset: None,
            scroll_offset_x: None,
            on_drop: None,
            on_slide: None,
            clip: false,
            overlay: false,
            hit_testable: false,
            offset: None,
            children: Vec::new(),
        }
    }

    pub fn row(mut self) -> Self {
        self.style.flex_direction = FlexDirection::Row;
        self
    }
    pub fn col(mut self) -> Self {
        self.style.flex_direction = FlexDirection::Column;
        self
    }
    pub fn w(mut self, v: f32) -> Self {
        self.style.size.width = length(v);
        self
    }
    pub fn h(mut self, v: f32) -> Self {
        self.style.size.height = length(v);
        self
    }
    pub fn wh(self, w: f32, h: f32) -> Self {
        self.w(w).h(h)
    }
    pub fn w_pct(mut self, p: f32) -> Self {
        self.style.size.width = percent(p);
        self
    }
    pub fn h_pct(mut self, p: f32) -> Self {
        self.style.size.height = percent(p);
        self
    }
    pub fn min_w(mut self, v: f32) -> Self {
        self.style.min_size.width = length(v);
        self
    }
    pub fn min_h(mut self, v: f32) -> Self {
        self.style.min_size.height = length(v);
        self
    }
    pub fn grow(mut self, g: f32) -> Self {
        self.style.flex_grow = g;
        self.style.flex_basis = length(0.0);
        self
    }
    pub fn no_shrink(mut self) -> Self {
        self.style.flex_shrink = 0.0;
        self
    }
    pub fn pad(mut self, v: f32) -> Self {
        self.style.padding = Rect { left: length(v), right: length(v), top: length(v), bottom: length(v) };
        self
    }
    pub fn pad_xy(mut self, x: f32, y: f32) -> Self {
        self.style.padding = Rect { left: length(x), right: length(x), top: length(y), bottom: length(y) };
        self
    }
    /// Top, right, bottom, left, as CSS has it.
    pub fn pad_each(mut self, t: f32, r: f32, b: f32, l: f32) -> Self {
        self.style.padding = Rect { left: length(l), right: length(r), top: length(t), bottom: length(b) };
        self
    }
    pub fn gap(mut self, v: f32) -> Self {
        self.style.gap = Size { width: length(v), height: length(v) };
        self
    }
    pub fn align(mut self, a: AlignItems) -> Self {
        self.style.align_items = Some(a);
        self
    }
    pub fn justify(mut self, j: JustifyContent) -> Self {
        self.style.justify_content = Some(j);
        self
    }
    pub fn center(self) -> Self {
        self.align(AlignItems::CENTER).justify(JustifyContent::CENTER)
    }
    pub fn wrap(mut self) -> Self {
        self.style.flex_wrap = FlexWrap::Wrap;
        self
    }
    /// Absolutely positioned at the given insets (`None` = auto).
    pub fn abs(mut self, l: Option<f32>, t: Option<f32>, r: Option<f32>, b: Option<f32>) -> Self {
        let f = |v: Option<f32>| v.map_or(auto(), length);
        self.style.position = Position::Absolute;
        self.style.inset = Rect { left: f(l), right: f(r), top: f(t), bottom: f(b) };
        self
    }
    pub fn abs_fill(self) -> Self {
        self.abs(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
    }
    pub fn fill(mut self, c: Color) -> Self {
        self.look.fill = c;
        self
    }
    pub fn radius(mut self, r: f32) -> Self {
        self.look.radius = r;
        self
    }
    pub fn border(mut self, w: f32, c: Color) -> Self {
        self.look.border = w;
        self.look.border_color = c;
        self
    }
    pub fn shadow(mut self, blur: f32, dy: f32, c: Color) -> Self {
        self.look.shadow = Some(Shadow { blur, dy, color: c });
        self
    }
    pub fn opacity(mut self, o: f32) -> Self {
        self.look.opacity = o;
        self
    }
    pub fn on(mut self, action: impl Into<String>) -> Self {
        self.action = Some(action.into());
        self
    }
    pub fn hover_fill(mut self, c: Color) -> Self {
        self.hover.fill = Some(c);
        self
    }
    pub fn ease(mut self, ms: u32) -> Self {
        self.transition = Transition { ms, ease: Ease::Out };
        self
    }
    pub fn transition(mut self, ms: u32, ease: Ease) -> Self {
        self.transition = Transition { ms, ease };
        self
    }
    pub fn offset(mut self, dx: f32, dy: f32) -> Self {
        self.offset = Some((dx, dy));
        self
    }
    pub fn hit(mut self) -> Self {
        self.hit_testable = true;
        self
    }
    pub fn scroll(mut self, off: f32) -> Self {
        self.scroll_offset = Some(off);
        self
    }
    pub fn clip(mut self) -> Self {
        self.clip = true;
        self
    }
    pub fn overlay(mut self) -> Self {
        self.overlay = true;
        self
    }
    pub fn max_w(mut self, v: f32) -> Self {
        self.style.max_size.width = length(v);
        self
    }
    pub fn max_h(mut self, v: f32) -> Self {
        self.style.max_size.height = length(v);
        self
    }
    pub fn enter(mut self, ms: u32, dy: f32, delay: u32) -> Self {
        self.enter = Some(Enter { ms, dy, delay });
        self
    }
    pub fn child(mut self, n: Node) -> Self {
        self.children.push(n);
        self
    }
    pub fn kids(mut self, ns: impl IntoIterator<Item = Node>) -> Self {
        self.children.extend(ns);
        self
    }
    pub fn text(key: impl Into<String>, s: impl Into<String>, size: f32, color: Color) -> Self {
        let mut n = Node::new(key);
        n.kind = Kind::Text(TextSpec { text: s.into(), size, color, ..Default::default() });
        n
    }
    pub fn with_text(mut self, f: impl FnOnce(&mut TextSpec)) -> Self {
        if let Kind::Text(t) = &mut self.kind {
            f(t);
        }
        self
    }
}

/// Logical px.
#[derive(Clone, Debug)]
pub struct Hit {
    pub rect: [f32; 4],
    pub clip: [f32; 4],
    pub key: String,
    pub action: Option<String>,
    pub on_drop: Option<String>,
    pub on_slide: Option<String>,
}

impl Hit {
    fn contains(&self, x: f32, y: f32) -> bool {
        let [rx, ry, rw, rh] = self.rect;
        let c = self.clip;
        x >= rx && x < rx + rw && y >= ry && y < ry + rh && x >= c[0] && x < c[2] && y >= c[1] && y < c[3]
    }
}

#[derive(Clone, Debug)]
pub struct ScrollInfo {
    pub key: String,
    /// Along its axis.
    pub view: f32,
    pub content: f32,
    pub horizontal: bool,
}

/// One scroll axis of a container, in logical px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollAxis {
    pub offset: f32,
    pub view: f32,
    pub content: f32,
}

/// The kind-specific part of a `Placed`.
#[derive(Clone, Debug)]
pub enum PlacedKind {
    Box,
    Text {
        spec: TextSpec,
        /// What the text is drawn in: `spec.color` after hover and its transition, before opacity.
        color: Color,
        /// Shaping facts; `None` when the node never reached the text engine.
        run: Option<RunInfo>,
    },
    Image {
        spec: ImageSpec,
        /// The picture ids drawn this frame: the old one then the new one during a fade.
        shown: Vec<String>,
    },
    Shape {
        name: &'static str,
        attrs: Vec<(&'static str, String)>,
    },
}

/// What `emit` knew about one node, recorded when `Env.trace` is on (pre-order, beside
/// `Frame.rects`): the facts a scene dump prints.
#[derive(Clone, Debug)]
pub struct Placed {
    pub key: String,
    /// 0 = the window node, 1 = the card.
    pub depth: u32,
    pub kind: PlacedKind,
    /// Logical px in the window.
    pub rect: [f32; 4],
    /// The clip this node is drawn under, `[x0, y0, x1, y1]` logical px (`Hit.clip`'s form;
    /// effectively unbounded when nothing above clips).
    pub clip: [f32; 4],
    /// The clip this node imposes on its children, when it clips or scrolls.
    pub imposes: Option<[f32; 4]>,
    /// Effective opacity: ancestors, the node's own (hover, transition) and its enter fade.
    pub opacity: f32,
    pub layer: usize,
    /// As drawn: the animated fill and border colour, `opacity` being the node's own.
    pub look: Look,
    pub hover: Hover,
    pub enter: Option<Enter>,
    pub action: Option<String>,
    pub on_drop: Option<String>,
    pub on_slide: Option<String>,
    /// It is in `Frame.hits`.
    pub hit: bool,
    pub scroll_y: Option<ScrollAxis>,
    pub scroll_x: Option<ScrollAxis>,
    /// `emit` drew it: it has area and is not transparent.
    pub drawn: bool,
}

#[derive(Default)]
pub struct Frame {
    pub list: DrawList,
    pub hits: Vec<Hit>,
    /// Appended after `hits` so popups are on top.
    overlay_hits: Vec<Hit>,
    pub scrolls: Vec<ScrollInfo>,
    /// (x, y, w, h) in logical px, per key.
    pub rects: Vec<(String, [f32; 4])>,
    /// Every node in pre-order; empty unless `Env.trace` was on.
    pub nodes: Vec<Placed>,
    pub animating: bool,
    pub content_size: (f32, f32),
}

impl Frame {
    pub fn hit_at(&self, x: f32, y: f32) -> Option<&Hit> {
        self.hits.iter().rev().find(|h| h.contains(x, y))
    }

    /// The `on_drop` action of the topmost element under `(x, y)` that takes files.
    pub fn drop_at(&self, x: f32, y: f32) -> Option<&str> {
        self.hits.iter().rev().find(|h| h.on_drop.is_some() && h.contains(x, y)).and_then(|h| h.on_drop.as_deref())
    }

    pub fn rect_of(&self, key: &str) -> Option<[f32; 4]> {
        self.rects.iter().find(|(k, _)| k == key).map(|(_, r)| *r)
    }
}

pub struct Env<'a> {
    pub text: &'a mut TextEngine,
    pub anim: &'a mut Anim,
    pub hover: Option<&'a str>,
    pub now: Instant,
    pub scale: f32,
    /// Also record a `Placed` per node in `Frame.nodes`. Off for the app; the draw list is the same either way.
    pub trace: bool,
}

struct Ctx {
    clip: [f32; 4],
    opacity: f32,
    layer: usize,
    /// 0 = the window node, 1 = the card.
    depth: u32,
}

/// A reflow (a tier switch, a gauge wrapping) that moves or resizes a node by more
/// than this in one frame glides for `REFLOW_MS`; smaller steps, like a live resize,
/// are followed directly.
const REFLOW_JUMP: f32 = 6.0;
const REFLOW_MS: u32 = 220;

fn is_hovered(hover: Option<&str>, key: &str) -> bool {
    hover.is_some_and(|h| h == key || (h.starts_with(key) && h.as_bytes().get(key.len()) == Some(&b'/')))
}

fn build<'a>(tree: &mut TaffyTree<usize>, n: &'a Node, nodes: &mut Vec<&'a Node>, ids: &mut Vec<NodeId>, in_scroll: bool) -> NodeId {
    let idx = nodes.len();
    nodes.push(n);
    ids.push(NodeId::new(0)); // placeholder, patched below
    let mut style = n.style.clone();
    if in_scroll {
        style.flex_shrink = 0.0;
    }
    let scroll = |on: bool| if on { taffy::Overflow::Scroll } else { taffy::Overflow::Visible };
    if n.scroll_offset.is_some() || n.scroll_offset_x.is_some() {
        style.overflow = taffy::Point { x: scroll(n.scroll_offset_x.is_some()), y: scroll(n.scroll_offset.is_some()) };
    }
    let id = if n.children.is_empty() {
        match n.kind {
            Kind::Text(_) | Kind::Image(_) => tree.new_leaf_with_context(style, idx),
            _ => tree.new_leaf(style),
        }
        .expect("leaf")
    } else {
        let kids: Vec<NodeId> = n.children.iter().map(|c| build(tree, c, nodes, ids, n.scroll_offset.is_some() || n.scroll_offset_x.is_some())).collect();
        tree.new_with_children(style, &kids).expect("node")
    };
    ids[idx] = id;
    id
}

pub fn layout(root: &Node, size: (f32, f32), env: &mut Env) -> Frame {
    let mut tree: TaffyTree<usize> = TaffyTree::new();
    let (mut nodes, mut ids) = (Vec::new(), Vec::new());
    let rid = build(&mut tree, root, &mut nodes, &mut ids, false);

    env.text.begin_frame();
    env.anim.begin_frame();
    {
        let text = &mut *env.text;
        let nodes = &nodes;
        tree.compute_layout_with_measure(
            rid,
            Size { width: AvailableSpace::Definite(size.0), height: AvailableSpace::Definite(size.1) },
            |inputs, _id, ctx, style| {
                taffy::compute_leaf_layout(inputs, style, |_, _| 0.0, |known, avail| {
                    let Some(&mut i) = ctx else { return Size::ZERO };
                    match &nodes[i].kind {
                        Kind::Text(spec) => {
                            let limit = known.width.or(match avail.width {
                                AvailableSpace::Definite(w) => Some(w),
                                _ => None,
                            });
                            let (w, h) = text.measure(&nodes[i].key, spec, limit);
                            Size { width: known.width.unwrap_or(w), height: known.height.unwrap_or(h) }
                        }
                        Kind::Image(im) => {
                            let (iw, ih) = (im.w.max(1.0), im.h.max(1.0));
                            match (known.width, known.height) {
                                (Some(w), Some(h)) => Size { width: w, height: h },
                                (Some(w), None) => Size { width: w, height: w * ih / iw },
                                (None, Some(h)) => Size { width: h * iw / ih, height: h },
                                _ => Size { width: iw, height: ih },
                            }
                        }
                        _ => Size::ZERO,
                    }
                })
            },
        )
        .expect("layout");
    }

    let mut frame = Frame::default();
    let mut next = 0usize;
    let ctx = Ctx { clip: NO_CLIP, opacity: 1.0, layer: 0, depth: 0 };
    emit(root, &ids, &mut next, &tree, (0.0, 0.0), &ctx, env, &mut frame);
    let l = tree.layout(rid).expect("root");
    frame.content_size = (l.size.width, l.size.height);
    let ov = std::mem::take(&mut frame.overlay_hits);
    frame.hits.extend(ov);
    env.text.end_frame();
    env.anim.end_frame();
    frame.animating = env.anim.animating(env.now);
    frame
}

#[allow(clippy::too_many_arguments)]
fn emit(n: &Node, ids: &[NodeId], next: &mut usize, tree: &TaffyTree<usize>, origin: (f32, f32), ctx: &Ctx, env: &mut Env, out: &mut Frame) {
    let id = ids[*next];
    *next += 1;
    let l = tree.layout(id).expect("layout");
    let now = env.now;
    let key = n.key.as_str();

    // enter animation: fade + slide, replayed whenever the key (re)appears
    let (mut enter_a, mut enter_dy) = (1.0, 0.0);
    if let Some(e) = n.enter {
        let v = env.anim.value(key, "enter", [1.0, 0.0, 0.0, 0.0], e.ms, Ease::Out, e.delay, Some([0.0, e.dy, 0.0, 0.0]), now);
        (enter_a, enter_dy) = (v[0], v[1]);
    }

    let mut off = (0.0, 0.0);
    if let Some((ox, oy)) = n.offset {
        let v = env.anim.value(key, "off", [ox, oy, 0.0, 0.0], n.transition.ms, n.transition.ease, 0, None, now);
        off = (v[0], v[1]);
    }
    // the window and its card always fill the window; absolute nodes follow their parent
    let laid = [l.location.x, l.location.y, l.size.width, l.size.height];
    let flows = ctx.depth >= 2 && n.style.position == Position::Relative;
    let [lx, ly, lw, lh] = if flows { env.anim.follow(key, "rect", laid, REFLOW_MS, REFLOW_JUMP, now) } else { laid };
    let (x, y) = (origin.0 + lx + off.0, origin.1 + ly + enter_dy + off.1);
    // text keeps its laid-out box: a gliding width would re-wrap it every frame
    let (w, h) = if matches!(n.kind, Kind::Text(_)) { (l.size.width, l.size.height) } else { (lw, lh) };
    let hov = is_hovered(env.hover, key);
    let tr = n.transition;

    let mut animate = |prop: &'static str, target: [f32; 4]| -> [f32; 4] {
        env.anim.value(key, prop, target, tr.ms, tr.ease, 0, None, now)
    };
    let fill_t = if hov { n.hover.fill.unwrap_or(n.look.fill) } else { n.look.fill };
    let bc_t = if hov { n.hover.border_color.unwrap_or(n.look.border_color) } else { n.look.border_color };
    let op_t = if hov { n.hover.opacity.unwrap_or(n.look.opacity) } else { n.look.opacity };
    let fill = Color(animate("fill", fill_t.0));
    let fill2 = n.look.gradient_bottom.map(|f2| {
        // keep a gradient's bottom stop in step with a hover-shifted top stop
        let d = [fill.0[0] - n.look.fill.0[0], fill.0[1] - n.look.fill.0[1], fill.0[2] - n.look.fill.0[2]];
        Color([(f2.0[0] + d[0]).clamp(0.0, 1.0), (f2.0[1] + d[1]).clamp(0.0, 1.0), (f2.0[2] + d[2]).clamp(0.0, 1.0), f2.0[3] * fill.0[3] / n.look.fill.0[3].max(1e-4)])
    });
    let border_color = Color(animate("bc", bc_t.0));
    let own_op = animate("op", [op_t, 0.0, 0.0, 0.0])[0];
    let op = ctx.opacity * own_op * enter_a;

    let s = env.scale;
    let layer = if n.overlay { 1 } else { ctx.layer };
    let clip = ctx.clip;
    let list = &mut out.list.layers[layer];
    let rect = [x, y, w, h];
    let drawn = w > 0.0 && h > 0.0 && op > 0.001;
    let mut traced_color = None;
    let mut shown = Vec::new();

    if drawn {
        let (cx, cy, hw, hh) = ((x + w / 2.0) * s, (y + h / 2.0) * s, w / 2.0 * s, h / 2.0 * s);
        let r = (n.look.radius * s).min(hw).min(hh);
        if let Some(sh) = n.look.shadow {
            list.shapes.push(Inst {
                a: [cx, cy + sh.dy * s],
                b: [hw, hh],
                radius: r,
                kind: KIND_SHADOW,
                soft: sh.blur * s,
                fill_top: rgba_with_opacity(sh.color, op),
                clip,
                ..Default::default()
            });
        }
        if fill.0[3] > 0.0 || (n.look.border > 0.0 && border_color.0[3] > 0.0) {
            list.shapes.push(Inst {
                a: [cx, cy],
                b: [hw, hh],
                radius: r,
                border: n.look.border * s,
                kind: KIND_RECT,
                fill_top: rgba_with_opacity(fill, op),
                fill_bot: rgba_with_opacity(fill2.unwrap_or(fill), op),
                border_color: rgba_with_opacity(border_color, op),
                clip,
                ..Default::default()
            });
        }
        match &n.kind {
            Kind::Box => {}
            Kind::Text(spec) => {
                env.text.prepare(key, spec, w);
                let mut color = if hov { n.hover.text_color.unwrap_or(spec.color) } else { spec.color };
                color = Color(env.anim.value(key, "tc", color.0, tr.ms, tr.ease, 0, None, now));
                traced_color = Some(color);
                list.texts.push(TextItem {
                    key: key.to_string(),
                    x: x * s,
                    y: y * s,
                    scale: s,
                    color: color.mul_alpha(op).to_u8(),
                    clip: intersect(clip, [x * s, y * s, (x + w) * s, (y + h) * s]),
                });
                if let Some(c) = spec.caret {
                    let cxp = (x + env.text.caret_x(key, c)) * s;
                    list.shapes.push(Inst {
                        a: [cxp, y * s + 1.0 * s],
                        b: [cxp, (y + h) * s - 1.0 * s],
                        radius: 0.75 * s,
                        kind: KIND_CAPSULE,
                        fill_top: rgba_with_opacity(color, op),
                        fill_bot: rgba_with_opacity(color, op),
                        clip,
                        ..Default::default()
                    });
                }
            }
            Kind::Image(im) => {
                let target = Pic { id: im.id.clone(), size: (im.w, im.h) };
                let (under, over) = if im.fade > 0 { env.anim.crossfade(key, target, im.ready, im.fade, now) } else { (target, None) };
                // the old picture underneath at full opacity, the new one over it; each fits by its own size
                for (pic, alpha) in std::iter::once((under, op)).chain(over.map(|(p, t)| (p, op * t))) {
                    if env.trace {
                        shown.push(pic.id.clone());
                    }
                    let ((fw, fh), uv) = im.fit.place(pic.size, w, h);
                    let (dw, dh) = (fw / 2.0 * s, fh / 2.0 * s);
                    list.images.push(ImgDraw {
                        tex: pic.id,
                        inst: ImgInst {
                            center: [cx, cy],
                            half: [dw, dh],
                            radius: r.min(dw).min(dh),
                            feather: im.feather * s,
                            alpha,
                            tint: im.tint.map_or([1.0; 4], |t| t.0),
                            clip,
                            uv,
                            ..Default::default()
                        },
                        play: im.play,
                        frame: im.frame,
                    });
                }
            }
            Kind::Shape(shape) => shape.emit(&ShapeCx { center_px: [cx, cy], logical_size: (w, h), scale: s, inherited_opacity: op, clip_px: clip }, &mut list.shapes),
        }
    }

    let has_hit = n.hit_testable || n.action.is_some() || n.on_drop.is_some() || n.on_slide.is_some() || n.hover.any();
    if has_hit {
        let h = Hit { rect, clip: [clip[0] / s, clip[1] / s, clip[2] / s, clip[3] / s], key: key.to_string(), action: n.action.clone(), on_drop: n.on_drop.clone(), on_slide: n.on_slide.clone() };
        if layer == 1 { out.overlay_hits.push(h) } else { out.hits.push(h) }
    }
    out.rects.push((key.to_string(), rect));

    let mut child_ctx = Ctx { clip, opacity: op, layer, depth: ctx.depth + 1 };
    let mut child_origin = (x, y);
    if n.clip || n.scroll_offset.is_some() || n.scroll_offset_x.is_some() {
        child_ctx.clip = intersect(clip, [x * s, y * s, (x + w) * s, (y + h) * s]);
    }
    if let Some(off) = n.scroll_offset {
        child_origin.1 -= off;
        out.scrolls.push(ScrollInfo { key: key.to_string(), view: h, content: l.scrollable_overflow_rect.bottom.max(h), horizontal: false });
    }
    if let Some(off) = n.scroll_offset_x {
        child_origin.0 -= off;
        out.scrolls.push(ScrollInfo { key: key.to_string(), view: w, content: l.scrollable_overflow_rect.right.max(w), horizontal: true });
    }
    if env.trace {
        let axis = |horizontal: bool| out.scrolls.iter().rev().find(|s| s.key == key && s.horizontal == horizontal);
        let (scroll_y, scroll_x) = (
            n.scroll_offset.and_then(|offset| axis(false).map(|s| ScrollAxis { offset, view: s.view, content: s.content })),
            n.scroll_offset_x.and_then(|offset| axis(true).map(|s| ScrollAxis { offset, view: s.view, content: s.content })),
        );
        let kind = match &n.kind {
            Kind::Box => PlacedKind::Box,
            Kind::Text(spec) => PlacedKind::Text { spec: spec.clone(), color: traced_color.unwrap_or(spec.color), run: env.text.describe(key, spec) },
            Kind::Image(im) => PlacedKind::Image { spec: im.clone(), shown },
            Kind::Shape(shape) => {
                let (name, attrs) = shape.describe();
                PlacedKind::Shape { name, attrs }
            }
        };
        let logical = |c: [f32; 4]| [c[0] / s, c[1] / s, c[2] / s, c[3] / s];
        out.nodes.push(Placed {
            key: key.to_string(),
            depth: ctx.depth,
            kind,
            rect,
            clip: logical(clip),
            imposes: (n.clip || n.scroll_offset.is_some() || n.scroll_offset_x.is_some()).then(|| logical(child_ctx.clip)),
            opacity: op,
            layer,
            look: Look { fill, gradient_bottom: fill2, border: n.look.border, border_color, radius: n.look.radius, opacity: own_op, shadow: n.look.shadow },
            hover: n.hover,
            enter: n.enter,
            action: n.action.clone(),
            on_drop: n.on_drop.clone(),
            on_slide: n.on_slide.clone(),
            hit: has_hit,
            scroll_y,
            scroll_x,
            drawn,
        });
    }
    for c in &n.children {
        emit(c, ids, next, tree, child_origin, &child_ctx, env, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A window node, a card and a wrapping row of three 80 px "gauges".
    fn gauges(w: f32) -> Node {
        let row = Node::new("w/c/row").row().wrap().gap(10.0).kids((0..3).map(|i| Node::new(format!("w/c/row/{i}")).wh(80.0, 80.0)));
        Node::new("w").wh(w, 300.0).child(Node::new("w/c").grow(1.0).child(row))
    }

    #[test]
    fn cover_crops_the_middle_and_contain_letterboxes() {
        // a 200 x 100 picture in a 100 x 100 box
        assert_eq!(Fit::Contain.place((200.0, 100.0), 100.0, 100.0), ((100.0, 50.0), [0.0, 0.0, 1.0, 1.0]));
        assert_eq!(Fit::Cover.place((200.0, 100.0), 100.0, 100.0), ((100.0, 100.0), [0.25, 0.0, 0.75, 1.0]));
        assert_eq!(Fit::Cover.place((100.0, 400.0), 50.0, 100.0), ((50.0, 100.0), [0.0, 0.25, 1.0, 0.75]));
        assert_eq!(Fit::Cover.place((64.0, 64.0), 32.0, 32.0).1, [0.0, 0.0, 1.0, 1.0], "same shape: nothing cropped");
    }

    #[test]
    fn a_sideways_strip_scrolls_and_takes_drops() {
        let strip = Node::new("w/s").row().wh(100.0, 40.0).kids((0..5).map(|i| Node::new(format!("w/s/{i}")).wh(60.0, 40.0)));
        let mut strip = strip;
        strip.scroll_offset_x = Some(70.0);
        strip.children[1].on_drop = Some("gallery.add".into());
        let root = Node::new("w").wh(100.0, 40.0).child(strip);
        let mut env = Env { text: &mut TextEngine::new(), anim: &mut Anim::default(), hover: None, now: Instant::now(), scale: 1.0, trace: false };
        let f = layout(&root, (100.0, 40.0), &mut env);
        let s = f.scrolls.iter().find(|s| s.key == "w/s").unwrap();
        assert_eq!((s.horizontal, s.view, s.content), (true, 100.0, 300.0), "five 60 px cards in 100 px");
        assert_eq!(f.rect_of("w/s/1").unwrap()[0], -10.0, "moved left by the offset");
        assert_eq!(f.drop_at(5.0, 20.0), Some("gallery.add"));
        assert_eq!(f.drop_at(60.0, 20.0), None, "the next card takes no files");
    }

    /// A window, a clipping card and, inside it, one of each thing the trace has to say something about.
    fn fixture() -> Node {
        let ticks = Node { kind: Kind::shape(TicksSpec { count: 12, major_every: 3, len: 4.0, major_len: 8.0, width: 1.0, major_width: 2.0, color: Color([1.0, 0.0, 0.0, 1.0]), major_color: Color([0.0, 1.0, 0.0, 0.5]), inset: 6.5 }), ..Node::new("w/c/ticks").wh(40.0, 40.0) };
        let pic = Node { kind: Kind::Image(ImageSpec { id: "pic.png".into(), w: 48.0, h: 24.0, ready: true, ..Default::default() }), ..Node::new("w/c/pic").w(40.0) };
        let list = Node::new("w/c/list").col().wh(100.0, 50.0).scroll(10.0).kids((0..4).map(|i| Node::new(format!("w/c/list/{i}")).h(30.0)));
        let mut strip = Node::new("w/c/strip").row().wh(100.0, 20.0).kids((0..4).map(|i| Node::new(format!("w/c/strip/{i}")).w(60.0)));
        strip.scroll_offset_x = Some(5.0);
        let mut drop = Node::new("w/c/drop").wh(20.0, 10.0);
        (drop.on_drop, drop.on_slide) = (Some("files.add".into()), Some("vol.set".into()));
        let card = Node::new("w/c")
            .grow(1.0)
            .col()
            .clip()
            .radius(12.0)
            .fill(Color([0.1, 0.2, 0.3, 1.0]))
            .border(1.0, Color([1.0; 4]))
            .shadow(8.0, 2.0, Color([0.0, 0.0, 0.0, 0.5]))
            .child(Node::text("w/c/title", "Hello", 14.0, Color([1.0; 4])))
            .child(Node::text("w/c/body", "A long sentence that has to wrap in a narrow box", 12.0, Color([1.0; 4])).w(60.0).with_text(|t| t.wrap = true))
            .child(Node::new("w/c/btn").wh(40.0, 20.0).on("tog:x").hover_fill(Color([1.0; 4])).enter(300, 8.0, 0))
            .child(list)
            .child(strip)
            .child(drop)
            .child(ticks)
            .child(pic)
            .child(Node::new("w/c/pop").overlay().abs(Some(0.0), Some(0.0), None, None).wh(10.0, 10.0).fill(Color([1.0; 4])));
        Node::new("w").wh(200.0, 300.0).opacity(0.5).child(card)
    }

    fn traced(root: &Node, size: (f32, f32), scale: f32, trace: bool, text: &mut TextEngine) -> Frame {
        layout(root, size, &mut Env { text, anim: &mut Anim::default(), hover: None, now: Instant::now(), scale, trace })
    }

    fn node<'a>(f: &'a Frame, key: &str) -> &'a Placed {
        f.nodes.iter().find(|p| p.key == key).unwrap_or_else(|| panic!("no `{key}` in the trace"))
    }

    #[test]
    fn tracing_off_records_nothing_and_on_records_every_node_beside_rects() {
        let (root, mut text) = (fixture(), TextEngine::new());
        assert!(traced(&root, (200.0, 300.0), 1.0, false, &mut text).nodes.is_empty(), "the flag is opt-in");
        let f = traced(&root, (200.0, 300.0), 1.0, true, &mut text);
        assert_eq!(f.nodes.len(), f.rects.len());
        assert!(f.nodes.iter().zip(&f.rects).all(|(p, (k, r))| p.key == *k && p.rect == *r), "pre-order, the same keys and rects");
        let depths: Vec<(&str, u32)> = ["w", "w/c", "w/c/title", "w/c/list/3"].iter().map(|k| (*k, node(&f, k).depth)).collect();
        assert_eq!(depths, [("w", 0), ("w/c", 1), ("w/c/title", 2), ("w/c/list/3", 3)]);
    }

    #[test]
    fn the_draw_list_is_the_same_with_the_trace_on_or_off() {
        let (root, mut text) = (fixture(), TextEngine::new());
        for scale in [1.0, 1.25, 2.0] {
            let off = traced(&root, (200.0, 300.0), scale, false, &mut text);
            let on = traced(&root, (200.0, 300.0), scale, true, &mut text);
            assert!(!off.list.to_bytes().is_empty());
            assert!(off.list.to_bytes() == on.list.to_bytes(), "draw list differs at scale {scale}");
            assert_eq!((&off.rects, off.content_size, off.animating), (&on.rects, on.content_size, on.animating));
            let hits = |f: &Frame| f.hits.iter().map(|h| (h.key.clone(), h.rect, h.clip, h.action.clone())).collect::<Vec<_>>();
            assert_eq!(hits(&off), hits(&on));
            assert_eq!(off.scrolls.len(), on.scrolls.len());
        }
    }

    #[test]
    fn the_trace_says_how_each_node_is_drawn() {
        let (root, mut text) = (fixture(), TextEngine::new());
        let f = traced(&root, (200.0, 300.0), 2.0, true, &mut text);
        let (w, card) = (node(&f, "w"), node(&f, "w/c"));
        assert!((w.opacity - 0.5).abs() < 1e-6 && (card.opacity - 0.5).abs() < 1e-6, "a child inherits its parent's opacity");
        assert!((card.look.opacity - 1.0).abs() < 1e-6, "look.opacity is the node's own");
        assert_eq!((card.look.fill, card.look.border, card.look.radius), (Color([0.1, 0.2, 0.3, 1.0]), 1.0, 12.0));
        assert_eq!(card.look.shadow.map(|s| (s.blur, s.dy)), Some((8.0, 2.0)));
        assert!(card.drawn && !card.hit && card.layer == 0);
        // clips are logical px whatever the scale: the card imposes its own rect on its children
        let [x, y, cw, ch] = card.rect;
        assert_eq!(card.imposes, Some([x, y, x + cw, y + ch]));
        assert_eq!(node(&f, "w/c/title").clip, [x, y, x + cw, y + ch]);
        assert_eq!(w.imposes, None);
        assert!(w.clip[0] < -1e3 && w.clip[2] > 1e3, "nothing above the window clips");

        let btn = node(&f, "w/c/btn");
        assert_eq!((btn.hit, btn.action.as_deref(), btn.enter.map(|e| (e.ms, e.dy))), (true, Some("tog:x"), Some((300, 8.0))));
        assert_eq!(btn.hover.fill, Some(Color([1.0; 4])));
        let drop = node(&f, "w/c/drop");
        assert_eq!((drop.hit, drop.on_drop.as_deref(), drop.on_slide.as_deref()), (true, Some("files.add"), Some("vol.set")));

        let list = node(&f, "w/c/list");
        assert_eq!(list.scroll_y, Some(ScrollAxis { offset: 10.0, view: 50.0, content: 120.0 }));
        assert_eq!((list.scroll_x, list.imposes.is_some()), (None, true));
        assert_eq!(node(&f, "w/c/list/0").rect[1], list.rect[1] - 10.0, "scrolled content sits above the top by the offset");
        let strip = node(&f, "w/c/strip");
        assert_eq!(strip.scroll_x, Some(ScrollAxis { offset: 5.0, view: 100.0, content: 240.0 }));

        let pop = node(&f, "w/c/pop");
        assert_eq!((pop.layer, pop.rect), (1, [0.0, 0.0, 10.0, 10.0]));
    }

    #[test]
    fn the_trace_carries_shape_and_image_facts() {
        let (root, mut text) = (fixture(), TextEngine::new());
        let f = traced(&root, (200.0, 300.0), 1.0, true, &mut text);
        let PlacedKind::Shape { name, attrs } = &node(&f, "w/c/ticks").kind else { panic!("a shape") };
        let attrs: Vec<String> = attrs.iter().map(|(k, v)| format!("{k}={v}")).collect();
        assert_eq!(*name, "ticks");
        assert_eq!(attrs.join(" "), "count=12 major_every=3 len=4 major_len=8 width=1 major_width=2 color=#ff0000 major_color=#00ff0080 inset=6.5");
        let PlacedKind::Image { spec, shown } = &node(&f, "w/c/pic").kind else { panic!("an image") };
        assert_eq!((spec.id.as_str(), spec.ready, shown.as_slice()), ("pic.png", true, &["pic.png".to_string()][..]));
        assert!(matches!(node(&f, "w/c").kind, PlacedKind::Box));
    }

    #[test]
    fn the_trace_carries_text_facts_from_the_engine() {
        let (root, mut text) = (fixture(), TextEngine::new());
        let f = traced(&root, (200.0, 300.0), 1.0, true, &mut text);
        let PlacedKind::Text { spec, color, run } = &node(&f, "w/c/title").kind else { panic!("text") };
        let run = run.as_ref().expect("the engine shaped it");
        assert_eq!((spec.text.as_str(), *color, run.lines), ("Hello", Color([1.0; 4]), 1));
        assert_eq!((run.natural_w, run.natural_h), text.measure("w/c/title", spec, None), "natural size is the unlimited measure");
        assert!(!run.faces.is_empty() && run.faces.iter().all(|n| !n.is_empty()), "some face shaped it: {:?}", run.faces);
        let PlacedKind::Text { run: body, .. } = &node(&f, "w/c/body").kind else { panic!("text") };
        let body = body.as_ref().unwrap();
        assert!(body.lines >= 2, "wrapped in 60 px: {} lines", body.lines);
        assert!(body.natural_w > 60.0, "its natural width is more than the box: {}", body.natural_w);
    }

    #[test]
    fn a_hovered_text_reports_the_colour_it_is_drawn_in() {
        let mut n = Node::text("w/t", "Hi", 14.0, Color([1.0, 0.0, 0.0, 1.0]));
        n.hover.text_color = Some(Color([0.0, 0.0, 1.0, 1.0]));
        let root = Node::new("w").wh(100.0, 40.0).child(n);
        let mut env = Env { text: &mut TextEngine::new(), anim: &mut Anim::default(), hover: Some("w/t"), now: Instant::now(), scale: 1.0, trace: true };
        let f = layout(&root, (100.0, 40.0), &mut env);
        let PlacedKind::Text { color, .. } = &node(&f, "w/t").kind else { panic!("text") };
        assert_eq!(*color, Color([0.0, 0.0, 1.0, 1.0]));
    }

    #[test]
    fn a_node_that_is_not_drawn_is_still_in_the_trace() {
        let root = Node::new("w").wh(100.0, 40.0).child(Node::new("w/zero").wh(0.0, 10.0).fill(Color([1.0; 4]))).child(Node::new("w/ghost").wh(10.0, 10.0).opacity(0.0));
        let f = traced(&root, (100.0, 40.0), 1.0, true, &mut TextEngine::new());
        assert_eq!((node(&f, "w/zero").drawn, node(&f, "w/ghost").drawn, node(&f, "w").drawn), (false, false, true));
    }

    #[test]
    fn a_gauge_that_wraps_to_a_new_row_glides_there() {
        let (mut text, mut anim) = (TextEngine::new(), Anim::default());
        let t0 = Instant::now();
        let mut at = |w: f32, now: Instant| {
            let mut env = Env { text: &mut text, anim: &mut anim, hover: None, now, scale: 1.0, trace: false };
            let f = layout(&gauges(w), (w, 300.0), &mut env);
            (f.rect_of("w/c/row/2").unwrap(), f.animating)
        };
        let (wide, _) = at(300.0, t0);
        assert_eq!((wide[0], wide[1]), (180.0, 0.0), "all three in one row");
        let (mid, animating) = at(200.0, t0 + Duration::from_millis(16));
        assert!(animating && mid[1] < 90.0, "the third gauge is on its way down, not already there: {mid:?}");
        let (end, animating) = at(200.0, t0 + Duration::from_millis(400));
        let mut fresh = Anim::default();
        let mut env = Env { text: &mut TextEngine::new(), anim: &mut fresh, hover: None, now: t0, scale: 1.0, trace: false };
        let settled = layout(&gauges(200.0), (200.0, 300.0), &mut env).rect_of("w/c/row/2").unwrap();
        assert!(settled[1] > 80.0, "sanity: 200 px wraps it onto a second row");
        assert_eq!((end, animating), (settled, false), "and lands where a fresh layout puts it");
    }
}
