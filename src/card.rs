//! The Card: the visible body of an Instance. Its window adds a transparent
//! shadow gutter on every side so the card's shadow is not clipped, except
//! while the Instance blurs the desktop behind it: the blur fills the whole
//! window and must not spill past the card. Every card-vs-window question is
//! answered here, so the `± gutter` arithmetic lives in one place.

use crate::color::Color;
use crate::edit::Rect;
use crate::theme::Theme;
use crate::ui::Node;
use crate::widgets::ExpandInfo;
use crate::workspace::{InstanceCfg, Workspace};

/// Radius Windows 11 gives the blur region; a globally blurred card matches it.
const BLUR_RADIUS: f32 = 8.0;
/// Most a globally blurred card's fill may cover the blur, so it shows through.
const BLUR_FILL_ALPHA: f32 = 0.6;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Card {
    /// Logical px of transparent margin on every side of the card.
    pub gutter: f32,
    /// The desktop behind the window is blurred (Workspace-wide or this Instance's `blur` param).
    pub blur: bool,
    /// Borders are drawn (off = the flat, outline-free look).
    pub outlines: bool,
}

impl Card {
    pub fn new(theme: &Theme, blur: bool, outlines: bool) -> Card {
        Card { gutter: if blur { 0.0 } else { theme.num("gutter").max(0.0) }, blur, outlines }
    }

    /// The Card of one Instance under the current Workspace style.
    pub fn of(ws: &Workspace, cfg: &InstanceCfg, theme: &Theme) -> Card {
        Card::new(theme, Self::blurs(ws, cfg), ws.outlines)
    }

    /// Blur is on for every Instance when the Workspace asks for it, otherwise per Instance.
    pub fn blurs(ws: &Workspace, cfg: &InstanceCfg) -> bool {
        ws.blur || cfg.params.get("blur").and_then(|v| v.as_bool()).unwrap_or(false)
    }

    /// The gutter in physical px at `scale`.
    pub fn gutter_px(&self, scale: f64) -> i32 {
        (self.gutter * scale as f32).round() as i32
    }

    /// Physical card rect inside a physical window rect.
    pub fn card_of_window(&self, win: Rect, scale: f64) -> Rect {
        let g = self.gutter_px(scale);
        Rect { x: win.x + g, y: win.y + g, w: win.w - 2 * g, h: win.h - 2 * g }
    }

    /// Physical window rect around a physical card rect.
    pub fn window_of_card(&self, card: Rect, scale: f64) -> Rect {
        let g = self.gutter_px(scale);
        Rect { x: card.x - g, y: card.y - g, w: card.w + 2 * g, h: card.h + 2 * g }
    }

    /// Logical card size inside a logical window size.
    pub fn card_size(&self, window: (f32, f32)) -> (f32, f32) {
        ((window.0 - 2.0 * self.gutter).max(1.0), (window.1 - 2.0 * self.gutter).max(1.0))
    }

    /// Logical window size around a logical card size.
    pub fn window_size(&self, card: (f32, f32)) -> (f32, f32) {
        (card.0 + 2.0 * self.gutter, card.1 + 2.0 * self.gutter)
    }

    /// The card's rect (x, y, w, h) in a window of logical `size`.
    pub fn rect_in(&self, window: (f32, f32)) -> [f32; 4] {
        let g = self.gutter;
        [g, g, window.0 - 2.0 * g, window.1 - 2.0 * g]
    }

    /// Smallest physical window for a Widget whose card may not go below `min_card` (logical).
    pub fn min_window_px(&self, min_card: (f32, f32), scale: f64) -> (i32, i32) {
        let g = 2.0 * self.gutter as f64;
        (((min_card.0 as f64 + g) * scale) as i32, ((min_card.1 as f64 + g) * scale) as i32)
    }

    /// An expand size in card units, as the window size it needs.
    pub fn expand_window(&self, e: ExpandInfo) -> ExpandInfo {
        let g = 2.0 * self.gutter;
        ExpandInfo { active: e.active, width: e.width.map(|w| w + g), height: e.height.map(|h| h + g) }
    }

    /// Wrap a Widget's card tree in its window: the Workspace-wide blur look
    /// (unless the Widget styles blur itself), the outline switch and the gutter.
    pub fn dress(&self, key: &str, window: (f32, f32), mut card: Node, widget_styles_blur: bool) -> Node {
        if self.blur && !widget_styles_blur {
            card.look.radius = BLUR_RADIUS;
            card.look.fill = card.look.fill.with_alpha(card.look.fill.0[3].min(BLUR_FILL_ALPHA));
            card.look.fill2 = card.look.fill2.map(|c: Color| c.with_alpha(c.0[3].min(BLUR_FILL_ALPHA)));
        }
        if !self.outlines {
            fn flat(n: &mut Node) {
                n.look.border = 0.0;
                n.children.iter_mut().for_each(flat);
            }
            flat(&mut card);
        }
        Node::new(format!("{key}~")).wh(window.0, window.1).pad(self.gutter).child(card)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Library, Selection};
    use std::collections::BTreeMap;
    use std::path::Path;

    fn theme() -> Theme {
        Theme::compose(&Library::load(Path::new("nope")), &Selection::default(), &BTreeMap::new())
    }

    #[test]
    fn window_and_card_rects_round_trip_at_any_scale() {
        let c = Card::new(&theme(), false, true);
        assert_eq!(c.gutter, 20.0);
        for scale in [1.0, 1.5] {
            let win = Rect::new(100, 50, 300, 200);
            let card = c.card_of_window(win, scale);
            assert_eq!(card.x - win.x, c.gutter_px(scale));
            assert_eq!(c.window_of_card(card, scale), win);
        }
        assert_eq!(c.card_size(c.window_size((120.0, 80.0))), (120.0, 80.0));
    }

    #[test]
    fn blur_removes_the_gutter_and_comes_from_the_workspace_or_the_instance() {
        let t = theme();
        assert_eq!(Card::new(&t, true, true).gutter, 0.0);
        let mut ws = Workspace::default();
        let mut cfg = InstanceCfg::default();
        assert!(!Card::of(&ws, &cfg, &t).blur);
        cfg.params.insert("blur".into(), serde_json::Value::Bool(true));
        assert!(Card::of(&ws, &cfg, &t).blur);
        cfg.params.insert("blur".into(), serde_json::Value::Bool(false));
        ws.blur = true;
        assert!(Card::of(&ws, &cfg, &t).blur, "the Workspace switch wins over an Instance's saved `false`");
    }

    #[test]
    fn dress_adds_the_gutter_and_applies_the_style_switches() {
        let t = theme();
        let card = || Node::new("c").border(1.0, Color([1.0; 4])).fill(Color([0.1, 0.1, 0.1, 1.0])).child(Node::new("c/0").border(2.0, Color([1.0; 4])));
        let w = Card::new(&t, false, true).dress("k", (140.0, 100.0), card(), false);
        assert_eq!((w.key.as_str(), w.children.len()), ("k~", 1));
        assert_eq!(w.children[0].look.border, 1.0);

        let flat = Card::new(&t, false, false).dress("k", (140.0, 100.0), card(), false);
        assert_eq!((flat.children[0].look.border, flat.children[0].children[0].look.border), (0.0, 0.0));

        let blurred = Card::new(&t, true, true).dress("k", (140.0, 100.0), card(), false);
        assert_eq!((blurred.children[0].look.radius, blurred.children[0].look.fill.0[3]), (BLUR_RADIUS, BLUR_FILL_ALPHA));
        let own = Card::new(&t, true, true).dress("k", (140.0, 100.0), card(), true);
        assert_eq!(own.children[0].look.fill.0[3], 1.0, "a Widget that styles blur itself is left alone");
    }

    #[test]
    fn expand_sizes_gain_the_gutter() {
        let c = Card::new(&theme(), false, true);
        let e = c.expand_window(ExpandInfo { active: true, width: Some(100.0), height: None });
        assert_eq!((e.width, e.height), (Some(140.0), None));
    }
}
