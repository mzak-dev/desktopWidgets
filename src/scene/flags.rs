//! Layout flags: what `widgets/fits.rs` proved of every built-in Widget, now computed on any
//! `SceneDump`. A flag is a fact about one node (`TRUNCATED`, `OUTSIDE`...); a scene's
//! `[expect].flags` says whether it is an error (exit 1), a warning or ignored.
//!
//! The rules are `fits.rs`'s `must_fit`: they look at text and vector shapes outside any scroll
//! container (a scroller is meant to hold more than shows), with the same tolerances (half a
//! pixel against the card and clips, a pixel of text width). They read the dump alone, so a
//! baseline holds everything needed to recompute them. Text widths are the machine's fonts', so
//! a flag that depends on them (TRUNCATED, SQUASHED, TOFU, and OUTSIDE or OVERFLOW through a text
//! box) is exact on the machine and font set the dump was made on, like the dump itself.

use super::dump::{FlagRow, Kind, Node, SceneDump, n, quote};

/// How far a rect may stick out of the card, a clip or its parent before it is flagged.
const EDGE: f64 = 0.5;
/// How much wider than its box a one-line text may be: the width of a pixel.
const CUT: f64 = 1.0;

/// Every flag, as it prints.
pub const ALL: &[&str] = &["TRUNCATED", "CLIPPED", "OUTSIDE", "SQUASHED", "OVERFLOW-X", "OVERFLOW-Y", "ZERO", "EMPTY-TEXT-BOX", "NO-IMAGE", "TOFU"];

/// `[x0, y0, x1, y1]`.
type Edges = [f64; 4];

fn edges([x, y, w, h]: [f64; 4]) -> Edges {
    [x, y, x + w, y + h]
}

fn shown(e: Edges) -> String {
    format!("[{},{},{}x{}]", n(e[0]), n(e[1]), n(e[2] - e[0]), n(e[3] - e[1]))
}

fn meet(a: Option<Edges>, b: Option<Edges>) -> Option<Edges> {
    match (a, b) {
        (Some(a), Some(b)) => Some([a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])]),
        (a, b) => a.or(b),
    }
}

/// What a node inherits from the ones above it.
struct Above {
    depth: u32,
    /// In or at a scroll container.
    scrolls: bool,
    /// The clip its children are drawn under.
    clip: Option<Edges>,
}

fn row(flag: &str, nd: &Node, detail: String) -> FlagRow {
    FlagRow { flag: flag.to_string(), key: nd.key.clone(), detail }
}

/// The direct children of the node at `i`.
fn children(nodes: &[Node], i: usize) -> impl Iterator<Item = &Node> {
    let depth = nodes[i].depth;
    nodes[i + 1..].iter().take_while(move |c| c.depth > depth).filter(move |c| c.depth == depth + 1)
}

/// The flags of `d`, with the index of the node each belongs to, in node order.
pub fn compute(d: &SceneDump) -> Vec<(usize, FlagRow)> {
    let g = [(d.header.window[0] - d.header.card[0]) / 2.0, (d.header.window[1] - d.header.card[1]) / 2.0];
    let card = edges([g[0], g[1], d.header.card[0], d.header.card[1]]);
    let mut out = Vec::new();
    let mut above: Vec<Above> = Vec::new();
    for (i, nd) in d.nodes.iter().enumerate() {
        while above.last().is_some_and(|a| a.depth >= nd.depth) {
            above.pop();
        }
        let (parent_scrolls, clip) = above.last().map_or((false, None), |a| (a.scrolls, a.clip));
        let scrolls = parent_scrolls || nd.scroll_y.is_some() || nd.scroll_x.is_some();
        // the card's own clip is its edge, which OUTSIDE already says
        let imposed = nd.clip.map(edges).filter(|_| nd.depth >= 2);
        above.push(Above { depth: nd.depth, scrolls, clip: meet(clip, imposed) });
        let r = edges(nd.rect);
        let [_, _, w, h] = nd.rect;
        let mut put = |flag: &str, detail: String| out.push((i, row(flag, nd, detail)));
        if !scrolls {
            let drawn_here = matches!(&nd.kind, Kind::Shape { .. }) || matches!(&nd.kind, Kind::Text(t) if !t.text.trim().is_empty());
            if drawn_here {
                let squashed = matches!(&nd.kind, Kind::Text(_)) && (w < 1.0 || h < 1.0);
                if squashed {
                    put("SQUASHED", format!("{} {}x{}", text_of(nd), n(w), n(h)));
                } else {
                    if r[0] < card[0] - EDGE || r[1] < card[1] - EDGE || r[2] > card[2] + EDGE || r[3] > card[3] + EDGE {
                        put("OUTSIDE", format!("{}  card {}", shown(r), shown(card)));
                    }
                    if let Some(c) = clip.filter(|c| r[0] < c[0] - EDGE || r[1] < c[1] - EDGE || r[2] > c[2] + EDGE || r[3] > c[3] + EDGE) {
                        put("CLIPPED", format!("{}  clip {}", shown(r), shown(c)));
                    }
                    if let Kind::Text(t) = &nd.kind {
                        if let (false, Some([nw, _])) = (t.wrap, t.nat) {
                            if nw > w + CUT {
                                put("TRUNCATED", format!("{} nat {} > box {}", text_of(nd), n(nw), n(w)));
                            }
                        }
                    }
                }
            }
            if let Kind::Text(t) = &nd.kind {
                if t.text.is_empty() && w > 0.0 && h > 0.0 {
                    put("EMPTY-TEXT-BOX", format!("{}x{}", n(w), n(h)));
                }
                if t.tofu > 0 {
                    put("TOFU", format!("{} {} glyph{} no face has, shaped with {}", text_of(nd), t.tofu, if t.tofu == 1 { "" } else { "s" }, t.got.join("+")));
                }
            }
            let has_children = children(&d.nodes, i).next().is_some();
            let paints = !matches!(nd.kind, Kind::Text(_)) && (has_children || nd.fill.is_some() || matches!(nd.kind, Kind::Shape { .. } | Kind::Image(_)));
            if paints && (w <= 0.0 || h <= 0.0) {
                put("ZERO", format!("{}x{}{}", n(w), n(h), if has_children { " with children" } else { "" }));
            }
            if matches!(nd.kind, Kind::Box) && w > 0.0 && h > 0.0 {
                let kids: Vec<Edges> = children(&d.nodes, i).filter(|c| c.layer == nd.layer && c.rect[2] > 0.0 && c.rect[3] > 0.0).map(|c| edges(c.rect)).collect();
                let reach = |lo: fn(&Edges) -> f64, hi: fn(&Edges) -> f64, (a, b): (f64, f64)| -> Option<String> {
                    let (first, last) = (kids.iter().map(lo).fold(f64::INFINITY, f64::min), kids.iter().map(hi).fold(f64::NEG_INFINITY, f64::max));
                    let (before, after) = (a - first, last - b);
                    match (before > EDGE, after > EDGE) {
                        (false, false) => None,
                        (true, false) => Some(format!("children reach {} px before the box", n(before))),
                        (false, true) => Some(format!("children reach {} px past the box", n(after))),
                        (true, true) => Some(format!("children reach {} px before and {} px past the box", n(before), n(after))),
                    }
                };
                if !kids.is_empty() {
                    if let Some(s) = reach(|e| e[0], |e| e[2], (r[0], r[2])) {
                        put("OVERFLOW-X", format!("{s} (box {}..{})", n(r[0]), n(r[2])));
                    }
                    if let Some(s) = reach(|e| e[1], |e| e[3], (r[1], r[3])) {
                        put("OVERFLOW-Y", format!("{s} (box {}..{})", n(r[1]), n(r[3])));
                    }
                }
            }
            if let Kind::Image(im) = &nd.kind {
                if !im.resident {
                    put("NO-IMAGE", format!("{} is not in the Image Store{}", quote(&im.id), if im.ready { "" } else { " and the widget built it without a size" }));
                }
            }
        }
    }
    out
}

fn text_of(nd: &Node) -> String {
    match &nd.kind {
        Kind::Text(t) => quote(&t.text),
        _ => String::new(),
    }
}

/// Computes the flags of `d` and records them: on each node (the tokens at the end of its line)
/// and in the `flags` trailer.
pub fn apply(d: &mut SceneDump) {
    let found = compute(d);
    for (i, f) in &found {
        d.nodes[*i].flags.push(f.flag.clone());
    }
    d.flags = found.into_iter().map(|(_, f)| f).collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::dump::fixtures::{dump, plain, text};

    /// A 100x60 card at (10,10) in a 120x80 window, `kids` inside it.
    fn card_with(kids: Vec<Node>) -> SceneDump {
        let mut window = plain("w", 0);
        window.rect = [0.0, 0.0, 120.0, 80.0];
        let mut card = plain("w/c", 1);
        card.rect = [10.0, 10.0, 100.0, 60.0];
        let mut d = dump([vec![window, card], kids].concat());
        d.header.card = [100.0, 60.0];
        d.header.window = [120.0, 80.0];
        d
    }

    fn at(mut nd: Node, rect: [f64; 4]) -> Node {
        nd.rect = rect;
        nd
    }

    fn sized_text(key: &str, s: &str, rect: [f64; 4], nat_w: f64) -> Node {
        let mut t = at(text(key, 2, s), rect);
        if let Kind::Text(f) = &mut t.kind {
            f.nat = Some([nat_w, rect[3]]);
        }
        t
    }

    fn found(d: &mut SceneDump) -> Vec<String> {
        apply(d);
        d.flags.iter().map(|f| format!("{} {}", f.flag, f.key)).collect()
    }

    #[test]
    fn a_text_that_fits_has_no_flags_and_one_wider_than_its_box_is_truncated() {
        let mut d = card_with(vec![sized_text("w/c/a", "Fits", [20.0, 20.0, 40.0, 14.0], 40.5)]);
        assert!(found(&mut d).is_empty());
        let mut d = card_with(vec![sized_text("w/c/a", "Wednesday", [20.0, 20.0, 40.0, 14.0], 41.5)]);
        assert_eq!(found(&mut d), ["TRUNCATED w/c/a"]);
        assert_eq!(d.flags[0].detail, "\"Wednesday\" nat 41.5 > box 40");
        assert_eq!(d.nodes[2].flags, ["TRUNCATED"], "the node's line ends with the flag");
        let mut wrapped = sized_text("w/c/a", "Wednesday", [20.0, 20.0, 40.0, 14.0], 90.0);
        if let Kind::Text(t) = &mut wrapped.kind {
            t.wrap = true;
        }
        assert!(found(&mut card_with(vec![wrapped])).is_empty(), "a wrapping text is not cut off by being narrow");
    }

    #[test]
    fn text_and_shapes_sticking_out_of_the_card_are_outside_by_more_than_half_a_pixel() {
        let mut shape = at(plain("w/c/g", 2), [10.0, 10.0, 100.5, 20.0]);
        shape.kind = Kind::Shape { shape: "ticks".into(), attrs: vec![] };
        let mut d = card_with(vec![shape.clone(), sized_text("w/c/t", "x", [20.0, 60.0, 30.0, 11.0], 10.0)]);
        assert_eq!(found(&mut d), ["OVERFLOW-Y w/c", "OUTSIDE w/c/t"], "half a pixel is allowed: the shape reaches 110.5 and the card ends at 110 (what is outside the card is also past it)");
        shape.rect = [10.0, 10.0, 101.0, 20.0];
        let mut d = card_with(vec![shape]);
        assert_eq!(found(&mut d), ["OVERFLOW-X w/c", "OUTSIDE w/c/g"]);
        assert_eq!(d.flags[1].detail, "[10,10,101x20]  card [10,10,100x60]");
    }

    #[test]
    fn a_squashed_text_says_so_and_nothing_else_about_its_degenerate_box() {
        let mut d = card_with(vec![sized_text("w/c/a", "Hi", [20.0, 20.0, 0.0, 14.0], 20.0)]);
        assert_eq!(found(&mut d), ["SQUASHED w/c/a"]);
        assert_eq!(d.flags[0].detail, "\"Hi\" 0x14");
    }

    #[test]
    fn a_node_under_a_scroller_may_reach_past_the_card() {
        let mut scroller = at(plain("w/c/s", 2), [10.0, 10.0, 100.0, 60.0]);
        scroller.scroll_y = Some([0.0, 300.0]);
        let mut d = card_with(vec![scroller, Node { depth: 3, ..sized_text("w/c/s/t", "Row 9", [20.0, 200.0, 40.0, 14.0], 90.0) }]);
        assert!(found(&mut d).is_empty(), "outside the card and too wide, but scrolled: {:?}", d.flags);
    }

    #[test]
    fn a_clip_inside_the_card_flags_what_it_cuts() {
        let mut clip = at(plain("w/c/k", 2), [20.0, 20.0, 40.0, 30.0]);
        clip.clip = Some([20.0, 20.0, 40.0, 30.0]);
        let mut d = card_with(vec![clip, Node { depth: 3, ..sized_text("w/c/k/t", "Long text", [20.0, 20.0, 60.0, 14.0], 55.0) }]);
        assert_eq!(found(&mut d), ["OVERFLOW-X w/c/k", "CLIPPED w/c/k/t"], "what a clip cuts is also past its box");
        assert_eq!(d.flags[1].detail, "[20,20,60x14]  clip [20,20,40x30]");
        let mut card_clip = plain("w/c", 1);
        card_clip.clip = Some([10.0, 10.0, 100.0, 60.0]);
        let mut d = card_with(vec![sized_text("w/c/t", "x", [20.0, 60.0, 30.0, 11.0], 10.0)]);
        d.nodes[1] = Node { rect: [10.0, 10.0, 100.0, 60.0], ..card_clip };
        assert_eq!(found(&mut d), ["OVERFLOW-Y w/c", "OUTSIDE w/c/t"], "the card's own clip is its edge: OUTSIDE, not CLIPPED too");
    }

    #[test]
    fn a_box_whose_children_reach_past_it_overflows_on_that_axis() {
        let parent = at(plain("w/c/p", 2), [20.0, 20.0, 40.0, 20.0]);
        let kid = at(plain("w/c/p/k", 3), [20.0, 20.0, 55.0, 20.0]);
        let mut d = card_with(vec![parent, kid]);
        assert_eq!(found(&mut d), ["OVERFLOW-X w/c/p"]);
        assert_eq!(d.flags[0].detail, "children reach 15 px past the box (box 20..60)");
        let (parent, kid) = (at(plain("w/c/p", 2), [20.0, 20.0, 40.0, 20.0]), at(plain("w/c/p/k", 3), [20.0, 20.0, 40.4, 20.4]));
        assert!(found(&mut card_with(vec![parent, kid])).is_empty(), "within the half pixel");
    }

    #[test]
    fn nothing_that_paints_has_no_area_and_an_empty_text_with_area_is_an_unbound_text() {
        let mut gap = at(plain("w/c/z", 2), [20.0, 20.0, 0.0, 20.0]);
        gap.fill = Some("#ffffffff".into());
        let mut d = card_with(vec![gap, at(plain("w/c/q", 2), [30.0, 20.0, 0.0, 0.0]), sized_text("w/c/e", "", [20.0, 40.0, 30.0, 14.0], 0.0)]);
        assert_eq!(found(&mut d), ["ZERO w/c/z", "EMPTY-TEXT-BOX w/c/e"], "a plain empty box is just a spacer");
        let mut blank = card_with(vec![sized_text("w/c/e", " ", [20.0, 40.0, 30.0, 14.0], 4.0)]);
        assert!(found(&mut blank).is_empty(), "a space is a deliberate gap");
    }

    #[test]
    fn an_image_the_store_does_not_hold_and_a_glyph_no_face_has_are_flagged() {
        use crate::scene::dump::ImageFacts;
        let mut img = at(plain("w/c/i", 2), [20.0, 20.0, 30.0, 30.0]);
        img.kind = Kind::Image(ImageFacts { id: "gone.png".into(), natural: None, fit: "contain".into(), tint: None, feather: None, fade: None, play: false, frame: None, ready: false, resident: false, shown: vec![] });
        let mut t = sized_text("w/c/t", "a\u{e000}", [20.0, 55.0, 30.0, 12.0], 20.0);
        if let Kind::Text(f) = &mut t.kind {
            f.tofu = 1;
            f.got = vec!["Segoe UI".into()];
        }
        let mut d = card_with(vec![img, t]);
        assert_eq!(found(&mut d), ["NO-IMAGE w/c/i", "TOFU w/c/t"]);
        assert_eq!(d.flags[0].detail, "\"gone.png\" is not in the Image Store and the widget built it without a size");
        assert!(d.flags[1].detail.ends_with("1 glyph no face has, shaped with Segoe UI"), "{}", d.flags[1].detail);
    }

    #[test]
    fn the_flags_print_in_the_trailer_and_at_the_end_of_the_line() {
        let mut d = card_with(vec![sized_text("w/c/a", "Wednesday", [20.0, 20.0, 40.0, 14.0], 60.0)]);
        apply(&mut d);
        assert!(d.to_text().contains("--- flags (1)\nTRUNCATED  w/c/a  \"Wednesday\" nat 60 > box 40\n"), "{}", d.to_text());
        assert!(d.nodes[2].line(false).ends_with(" TRUNCATED"));
        assert!(ALL.iter().all(|f| f.chars().all(|c| c.is_ascii_uppercase() || c == '-')));
    }
}
