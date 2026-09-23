//! Node tree -> taffy layout -> draw list + hit regions, shared by widgets and
//! the settings window (decision 9).

use std::sync::Arc;
use std::time::Instant;

use taffy::prelude::*;

use crate::anim::{Anim, Ease};
use crate::color::Color;
use crate::draw::*;
use crate::elements::{Shape, ShapeCx, rgba_with_opacity};
use crate::text::{TextEngine, TextSpec};

pub use crate::elements::{ArcSpec, HandSpec, TicksSpec};

#[derive(Clone, Debug)]
pub enum Kind {
    Box,
    Text(TextSpec),
    Image(ImageSpec),
    Shape(Arc<dyn Shape>),
}

impl Kind {
    pub fn shape(s: impl Shape + 'static) -> Kind {
        Kind::Shape(Arc::new(s))
    }
}

#[derive(Clone, Debug, Default)]
pub struct ImageSpec {
    pub id: String,
    /// Intrinsic size, used when the style sets none.
    pub w: f32,
    pub h: f32,
    pub tint: Option<Color>,
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
}

#[derive(Clone, Debug)]
pub struct ScrollInfo {
    pub key: String,
    pub view_h: f32,
    pub content_h: f32,
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
    pub animating: bool,
    pub content_size: (f32, f32),
}

impl Frame {
    pub fn hit_at(&self, x: f32, y: f32) -> Option<&Hit> {
        self.hits.iter().rev().find(|h| {
            let [rx, ry, rw, rh] = h.rect;
            let c = h.clip;
            x >= rx && x < rx + rw && y >= ry && y < ry + rh && x >= c[0] && x < c[2] && y >= c[1] && y < c[3]
        })
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
    if n.scroll_offset.is_some() {
        style.overflow = taffy::Point { x: taffy::Overflow::Visible, y: taffy::Overflow::Scroll };
    }
    let id = if n.children.is_empty() {
        match n.kind {
            Kind::Text(_) | Kind::Image(_) => tree.new_leaf_with_context(style, idx),
            _ => tree.new_leaf(style),
        }
        .expect("leaf")
    } else {
        let kids: Vec<NodeId> = n.children.iter().map(|c| build(tree, c, nodes, ids, n.scroll_offset.is_some())).collect();
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

    if w > 0.0 && h > 0.0 && op > 0.001 {
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
                let (iw, ih) = (im.w.max(1.0), im.h.max(1.0));
                let k = (w / iw).min(h / ih);
                let (dw, dh) = (iw * k / 2.0 * s, ih * k / 2.0 * s);
                list.images.push(ImgDraw {
                    tex: im.id.clone(),
                    inst: ImgInst {
                        center: [cx, cy],
                        half: [dw, dh],
                        radius: r.min(dw).min(dh),
                        alpha: op,
                        tint: im.tint.map_or([1.0; 4], |t| t.0),
                        clip,
                        ..Default::default()
                    },
                });
            }
            Kind::Shape(shape) => shape.emit(&ShapeCx { center_px: [cx, cy], logical_size: (w, h), scale: s, inherited_opacity: op, clip_px: clip }, &mut list.shapes),
        }
    }

    if n.hit_testable || n.action.is_some() || n.hover.any() {
        let h = Hit { rect, clip: [clip[0] / s, clip[1] / s, clip[2] / s, clip[3] / s], key: key.to_string(), action: n.action.clone() };
        if layer == 1 { out.overlay_hits.push(h) } else { out.hits.push(h) }
    }
    out.rects.push((key.to_string(), rect));

    let mut child_ctx = Ctx { clip, opacity: op, layer, depth: ctx.depth + 1 };
    let mut child_origin = (x, y);
    if let Some(off) = n.scroll_offset {
        child_ctx.clip = intersect(clip, [x * s, y * s, (x + w) * s, (y + h) * s]);
        child_origin.1 -= off;
        out.scrolls.push(ScrollInfo { key: key.to_string(), view_h: h, content_h: l.scrollable_overflow_rect.bottom.max(h) });
    } else if n.clip {
        child_ctx.clip = intersect(clip, [x * s, y * s, (x + w) * s, (y + h) * s]);
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
    fn a_gauge_that_wraps_to_a_new_row_glides_there() {
        let (mut text, mut anim) = (TextEngine::new(), Anim::default());
        let t0 = Instant::now();
        let mut at = |w: f32, now: Instant| {
            let mut env = Env { text: &mut text, anim: &mut anim, hover: None, now, scale: 1.0 };
            let f = layout(&gauges(w), (w, 300.0), &mut env);
            (f.rect_of("w/c/row/2").unwrap(), f.animating)
        };
        let (wide, _) = at(300.0, t0);
        assert_eq!((wide[0], wide[1]), (180.0, 0.0), "all three in one row");
        let (mid, animating) = at(200.0, t0 + Duration::from_millis(16));
        assert!(animating && mid[1] < 90.0, "the third gauge is on its way down, not already there: {mid:?}");
        let (end, animating) = at(200.0, t0 + Duration::from_millis(400));
        let mut fresh = Anim::default();
        let mut env = Env { text: &mut TextEngine::new(), anim: &mut fresh, hover: None, now: t0, scale: 1.0 };
        let settled = layout(&gauges(200.0), (200.0, 300.0), &mut env).rect_of("w/c/row/2").unwrap();
        assert!(settled[1] > 80.0, "sanity: 200 px wraps it onto a second row");
        assert_eq!((end, animating), (settled, false), "and lands where a fresh layout puts it");
    }
}
