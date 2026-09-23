//! Edit Mode (decision 20): handle, snap and resize maths in physical px, plus the overlay.

use crate::color::Color;
use crate::theme::Theme;
use crate::ui::Node;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    Move,
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
}

impl Handle {
    fn edges(self) -> (bool, bool, bool, bool) {
        // (left, top, right, bottom) edges that move
        match self {
            Handle::Move => (false, false, false, false),
            Handle::N => (false, true, false, false),
            Handle::S => (false, false, false, true),
            Handle::E => (false, false, true, false),
            Handle::W => (true, false, false, false),
            Handle::NE => (false, true, true, false),
            Handle::NW => (true, true, false, false),
            Handle::SE => (false, false, true, true),
            Handle::SW => (true, false, false, true),
        }
    }

    pub fn cursor(self) -> winit::window::CursorIcon {
        use winit::window::CursorIcon::*;
        match self {
            Handle::Move => Move,
            Handle::N | Handle::S => NsResize,
            Handle::E | Handle::W => EwResize,
            Handle::NE | Handle::SW => NeswResize,
            Handle::NW | Handle::SE => NwseResize,
        }
    }
}

/// Logical px. Edges are `grip` thick; everything else, gutter included, moves the Instance.
pub fn hit_handle(px: f32, py: f32, card: [f32; 4], grip: f32) -> Handle {
    let [x, y, w, h] = card;
    let g = grip.min(w / 3.0).min(h / 3.0).max(6.0);
    let (l, r) = (px < x + g, px > x + w - g);
    let (t, b) = (py < y + g, py > y + h - g);
    match (l, r, t, b) {
        (true, _, true, _) => Handle::NW,
        (_, true, true, _) => Handle::NE,
        (true, _, _, true) => Handle::SW,
        (_, true, _, true) => Handle::SE,
        (true, ..) => Handle::W,
        (_, true, ..) => Handle::E,
        (_, _, true, _) => Handle::N,
        (_, _, _, true) => Handle::S,
        _ => Handle::Move,
    }
}

#[derive(Clone, Debug, Default)]
pub struct Snap {
    pub xs: Vec<i32>,
    pub ys: Vec<i32>,
    pub threshold: i32,
    /// 0 = off; `origin` is a work-area corner.
    pub grid: i32,
    pub origin: (i32, i32),
}

fn nearest(v: i32, lines: &[i32], thr: i32) -> Option<i32> {
    lines.iter().copied().filter(|l| (l - v).abs() <= thr).min_by_key(|l| (l - v).abs())
}

fn grid_snap(v: i32, origin: i32, grid: i32) -> i32 {
    if grid <= 0 { v } else { origin + ((v - origin) as f32 / grid as f32).round() as i32 * grid }
}

impl Snap {
    fn snap_coordinate(&self, v: i32, lines: &[i32], origin: i32) -> i32 {
        nearest(v, lines, self.threshold).unwrap_or_else(|| grid_snap(v, origin, self.grid))
    }
}

/// `max`, when given, bounds a resize the way `min` does; a move ignores both.
pub fn dragged_rect(handle: Handle, start: Rect, dx: i32, dy: i32, min: (i32, i32), max: Option<(i32, i32)>, snap: &Snap) -> Rect {
    if handle == Handle::Move {
        let mut r = Rect { x: start.x + dx, y: start.y + dy, ..start };
        // try both edges of the box against the guide lines; the closer hit wins
        let pick = |lo: i32, size: i32, lines: &[i32], origin: i32| -> i32 {
            let a = nearest(lo, lines, snap.threshold).map(|l| l - lo);
            let b = nearest(lo + size, lines, snap.threshold).map(|l| l - (lo + size));
            match (a, b) {
                (Some(a), Some(b)) => lo + if a.abs() <= b.abs() { a } else { b },
                (Some(a), None) => lo + a,
                (None, Some(b)) => lo + b,
                (None, None) => grid_snap(lo, origin, snap.grid),
            }
        };
        r.x = pick(r.x, r.w, &snap.xs, snap.origin.0);
        r.y = pick(r.y, r.h, &snap.ys, snap.origin.1);
        return r;
    }
    let (el, et, er, eb) = handle.edges();
    let (mut l, mut t, mut rr, mut b) = (start.x, start.y, start.right(), start.bottom());
    if el {
        l = snap.snap_coordinate(l + dx, &snap.xs, snap.origin.0).min(rr - min.0);
    }
    if er {
        rr = snap.snap_coordinate(rr + dx, &snap.xs, snap.origin.0).max(l + min.0);
    }
    if et {
        t = snap.snap_coordinate(t + dy, &snap.ys, snap.origin.1).min(b - min.1);
    }
    if eb {
        b = snap.snap_coordinate(b + dy, &snap.ys, snap.origin.1).max(t + min.1);
    }
    if let Some(m) = max {
        if el {
            l = l.max(rr - m.0);
        }
        if er {
            rr = rr.min(l + m.0);
        }
        if et {
            t = t.max(b - m.1);
        }
        if eb {
            b = b.min(t + m.1);
        }
    }
    Rect { x: l, y: t, w: rr - l, h: b - t }
}

/// Diameter of the Edit Mode remove button, logical px.
pub const REMOVE_BUTTON: f32 = 24.0;

/// Bottom-right, inside the card: clear of the position label and of every resize grip.
pub fn remove_button_center(card: [f32; 4]) -> (f32, f32) {
    let [x, y, w, h] = card;
    (x + w - 22.0, y + h - 22.0)
}

pub fn over_remove_button(px: f32, py: f32, card: [f32; 4]) -> bool {
    let (cx, cy) = remove_button_center(card);
    (px - cx).hypot(py - cy) <= REMOVE_BUTTON / 2.0 + 2.0
}

/// Outline, eight handles, a live position/size label and the remove button, which
/// asks "Remove?" once `remove_armed`.
pub fn overlay(id: &str, window: (f32, f32), gutter: f32, label: &str, theme: &Theme, active: Option<Handle>, remove_armed: bool) -> Node {
    let accent = theme.color("accent");
    let card = (window.0 - 2.0 * gutter, window.1 - 2.0 * gutter);
    let mut root = Node::new(format!("{id}!ov")).wh(window.0, window.1);
    root.overlay = true;

    let mut outline = Node::new(format!("{id}!ov/o")).abs(Some(gutter), Some(gutter), Some(gutter), Some(gutter)).radius(theme.num("radius-lg")).border(2.0, accent).fill(accent.with_alpha(0.07));
    outline.look.border_color = accent;
    root = root.child(outline);

    let (cx, cy) = (gutter + card.0 / 2.0, gutter + card.1 / 2.0);
    let (x0, y0, x1, y1) = (gutter, gutter, gutter + card.0, gutter + card.1);
    let s = 12.0;
    let pts = [
        (Handle::NW, x0, y0),
        (Handle::N, cx, y0),
        (Handle::NE, x1, y0),
        (Handle::E, x1, cy),
        (Handle::SE, x1, y1),
        (Handle::S, cx, y1),
        (Handle::SW, x0, y1),
        (Handle::W, x0, cy),
    ];
    for (i, (hd, px, py)) in pts.into_iter().enumerate() {
        let on = active == Some(hd);
        let fill = if on { accent } else { Color([1.0, 1.0, 1.0, 1.0]) };
        root = root.child(
            Node::new(format!("{id}!ov/h{i}"))
                .wh(s, s)
                .abs(Some(px - s / 2.0), Some(py - s / 2.0), None, None)
                .radius(s / 2.0)
                .fill(fill)
                .border(2.0, accent)
                .shadow(4.0, 1.0, Color([0.0, 0.0, 0.0, 0.35])),
        );
    }
    let pill = Node::new(format!("{id}!ov/l"))
        .abs(None, Some(gutter + 8.0), None, None)
        .center()
        .pad_xy(10.0, 4.0)
        .radius(11.0)
        .fill(Color([0.04, 0.05, 0.09, 0.86]))
        .child(Node::text(format!("{id}!ov/lt"), label, 12.0, Color([1.0, 1.0, 1.0, 1.0])).with_text(|t| t.family = theme.str("font-mono")));
    // centre the pill horizontally: an absolute node spans its own width, so wrap it
    let row = Node::new(format!("{id}!ov/lr")).abs(Some(gutter), Some(0.0), Some(gutter), None).center().child(pill);
    let (bx, by) = remove_button_center([gutter, gutter, card.0, card.1]);
    let r = REMOVE_BUTTON / 2.0;
    let danger = theme.color("danger");
    let remove = if remove_armed {
        Node::new(format!("{id}!ov/x"))
            .h(REMOVE_BUTTON)
            .abs(None, None, Some(window.0 - bx - r), Some(window.1 - by - r))
            .center()
            .pad_xy(10.0, 0.0)
            .radius(r)
            .fill(danger)
            .child(Node::text(format!("{id}!ov/xt"), "Remove?", 12.0, Color([1.0; 4])).with_text(|t| t.weight = 700))
    } else {
        let glyph = Node::text(format!("{id}!ov/xg"), theme.str("glyph-close"), 11.0, Color([1.0; 4])).with_text(|t| t.family = theme.str("font-glyph"));
        Node::new(format!("{id}!ov/x")).wh(REMOVE_BUTTON, REMOVE_BUTTON).abs(Some(bx - r), Some(by - r), None, None).center().radius(r).fill(Color([0.04, 0.05, 0.09, 0.86])).border(2.0, danger).enter(180, 6.0, 0).child(glyph)
    };
    root.child(row).child(remove)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::Kind;

    fn theme() -> Theme {
        Theme::compose(&crate::theme::Library::load(std::path::Path::new("nope")), &crate::theme::Selection::default(), &[])
    }

    #[test]
    fn the_remove_button_sits_inside_the_card_and_never_steals_a_resize_grip() {
        for card in [[20.0, 20.0, 220.0, 220.0], [0.0, 0.0, 72.0, 44.0], [20.0, 20.0, 48.0, 48.0]] {
            let (cx, cy) = remove_button_center(card);
            assert!(over_remove_button(cx, cy, card));
            assert_eq!(hit_handle(cx, cy, card, 16.0), Handle::Move, "{card:?}: its centre is not a resize grip");
            let [x, y, w, h] = card;
            let r = REMOVE_BUTTON / 2.0;
            assert!(cx - r >= x && cy - r >= y && cx + r <= x + w && cy + r <= y + h, "{card:?}: fully inside the card");
        }
        assert!(!over_remove_button(30.0, 30.0, [20.0, 20.0, 220.0, 220.0]));
    }

    #[test]
    fn overlay_shows_a_remove_button_that_turns_into_a_confirm_pill() {
        fn find<'a>(n: &'a Node, key: &str) -> Option<&'a Node> {
            if n.key == key { Some(n) } else { n.children.iter().find_map(|c| find(c, key)) }
        }
        let plain = overlay("w", (140.0, 120.0), 20.0, "0, 0", &theme(), None, false);
        assert!(find(&plain, "w!ov/x").is_some() && find(&plain, "w!ov/xt").is_none());
        let armed = overlay("w", (140.0, 120.0), 20.0, "0, 0", &theme(), None, true);
        assert!(matches!(&find(&armed, "w!ov/xt").unwrap().kind, Kind::Text(s) if s.text == "Remove?"));
    }

    fn snap() -> Snap {
        Snap { xs: vec![0, 500, 1000], ys: vec![0, 300], threshold: 8, grid: 0, origin: (0, 0) }
    }

    #[test]
    fn grips_pick_corners_edges_and_the_body() {
        let card = [20.0, 20.0, 200.0, 100.0];
        assert_eq!(hit_handle(22.0, 22.0, card, 14.0), Handle::NW);
        assert_eq!(hit_handle(218.0, 118.0, card, 14.0), Handle::SE);
        assert_eq!(hit_handle(120.0, 21.0, card, 14.0), Handle::N);
        assert_eq!(hit_handle(21.0, 70.0, card, 14.0), Handle::W);
        assert_eq!(hit_handle(120.0, 70.0, card, 14.0), Handle::Move);
        assert_eq!(hit_handle(5.0, 5.0, card, 14.0), Handle::NW, "the shadow gutter reaches the corner grip");
    }

    #[test]
    fn move_snaps_either_edge_to_a_guide_and_leaves_far_moves_alone() {
        let start = Rect::new(100, 100, 200, 100);
        // right edge lands 4px shy of x=500 -> snaps so right == 500
        let r = dragged_rect(Handle::Move, start, 196, 0, (40, 40), None, &snap());
        assert_eq!((r.x, r.right()), (300, 500));
        // left edge near 0 wins when it is the closer hit
        let r = dragged_rect(Handle::Move, start, -97, 0, (40, 40), None, &snap());
        assert_eq!(r.x, 0);
        // nothing within the threshold: unchanged
        let r = dragged_rect(Handle::Move, start, 60, 0, (40, 40), None, &snap());
        assert_eq!(r.x, 160);
    }

    #[test]
    fn resize_keeps_the_opposite_edge_fixed_and_honours_the_minimum() {
        let start = Rect::new(400, 300, 200, 100);
        let free = Snap { threshold: 0, ..Default::default() };
        let r = dragged_rect(Handle::E, start, 55, 0, (60, 40), None, &free);
        assert_eq!((r.x, r.w, r.y, r.h), (400, 255, 300, 100));
        // shrinking past the minimum stops at it, anchored on the far edge
        let r = dragged_rect(Handle::W, start, 500, 0, (60, 40), None, &free);
        assert_eq!((r.w, r.right()), (60, 600));
        let r = dragged_rect(Handle::NW, start, 500, 500, (60, 40), None, &free);
        assert_eq!((r.w, r.h, r.right(), r.bottom()), (60, 40, 600, 400));
        // a corner moves two edges at once
        let r = dragged_rect(Handle::SE, start, 10, 20, (60, 40), None, &free);
        assert_eq!((r.w, r.h), (210, 120));
    }

    #[test]
    fn resize_stops_at_the_max_only_while_a_max_is_given() {
        let start = Rect::new(400, 300, 200, 100);
        let free = Snap { threshold: 0, ..Default::default() };
        let r = dragged_rect(Handle::SE, start, 500, 500, (60, 40), Some((300, 150)), &free);
        assert_eq!((r.x, r.y, r.w, r.h), (400, 300, 300, 150));
        let r = dragged_rect(Handle::NW, start, -500, -500, (60, 40), Some((300, 150)), &free);
        assert_eq!((r.right(), r.bottom(), r.w, r.h), (600, 400, 300, 150), "anchored on the far edge");
        let r = dragged_rect(Handle::E, start, 500, 0, (60, 40), None, &free);
        assert_eq!(r.w, 700, "no limit");
        let r = dragged_rect(Handle::W, start, 500, 0, (60, 40), Some((300, 150)), &free);
        assert_eq!(r.w, 60, "the min holds either way");
        let r = dragged_rect(Handle::Move, start, 30, 0, (60, 40), Some((100, 50)), &free);
        assert_eq!((r.w, r.h), (200, 100), "moving never resizes");
    }

    #[test]
    fn grid_applies_only_when_no_guide_is_close() {
        let g = Snap { grid: 16, origin: (0, 0), threshold: 8, xs: vec![1000], ys: vec![] };
        let r = dragged_rect(Handle::Move, Rect::new(0, 0, 100, 100), 21, 21, (40, 40), None, &g);
        assert_eq!((r.x, r.y), (16, 16));
        let r = dragged_rect(Handle::Move, Rect::new(0, 0, 100, 100), 903, 0, (40, 40), None, &g); // right edge 1003 ~ guide 1000
        assert_eq!(r.right(), 1000);
    }
}
