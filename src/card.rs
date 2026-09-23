//! The card is an Instance's visible body. Its window adds a transparent gutter
//! so the shadow isn't clipped, except under blur, which must not spill past the card.

use crate::color::Color;
use crate::edit::Rect;
use crate::theme::Theme;
use crate::ui::{Kind, Node};
use crate::widgets::ExpandInfo;

const MAX_FILL_ALPHA_OVER_BLUR: f32 = 0.6;

/// (DWM corner preference, card radius) for a roundness under blur. DWM clips the
/// blur to square, small (4 px) or standard (8 px) corners only; both scale with DPI.
pub fn blur_corners(radius: f32) -> (i32, f32) {
    if radius < 2.0 {
        (1, 0.0) // DWMWCP_DONOTROUND
    } else if radius < 6.0 {
        (3, 4.0) // DWMWCP_ROUNDSMALL
    } else {
        (2, 8.0) // DWMWCP_ROUND
    }
}
const NEUTRAL_GLASS: &str = "#141414";

/// The Style tokens the engine applies to every Widget's card, read from the Instance's Theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Card {
    /// Logical px.
    pub gutter: f32,
    pub blur: bool,
    pub outlines: bool,
    /// `radius-lg`, logical px.
    pub radius: f32,
    /// The fill alpha while the background is transparent.
    pub bg_alpha: Option<f32>,
    pub tint: bool,
    /// 0..=1, times the shadow's own alpha.
    pub shadow: f32,
    pub text_scale: f32,
}

impl Card {
    pub fn new(theme: &Theme) -> Card {
        let blur = theme.flag("blur");
        Card {
            gutter: if blur { 0.0 } else { theme.num("gutter").max(0.0) },
            blur,
            outlines: theme.flag("outlines"),
            radius: theme.num("radius-lg").max(0.0),
            bg_alpha: theme.flag("transparent").then(|| (theme.num("bg-opacity") / 100.0).clamp(0.0, 1.0)),
            tint: theme.flag("tint"),
            shadow: (theme.num("shadow") / 100.0).clamp(0.0, 1.0),
            text_scale: (theme.num("text-scale") / 100.0).max(0.1),
        }
    }

    pub fn blur_corner_pref(&self) -> i32 {
        blur_corners(self.radius).0
    }

    pub fn gutter_px(&self, scale: f64) -> i32 {
        (self.gutter * scale as f32).round() as i32
    }

    pub fn card_of_window(&self, win: Rect, scale: f64) -> Rect {
        let g = self.gutter_px(scale);
        Rect { x: win.x + g, y: win.y + g, w: win.w - 2 * g, h: win.h - 2 * g }
    }

    pub fn window_of_card(&self, card: Rect, scale: f64) -> Rect {
        let g = self.gutter_px(scale);
        Rect { x: card.x - g, y: card.y - g, w: card.w + 2 * g, h: card.h + 2 * g }
    }

    pub fn card_size(&self, window: (f32, f32)) -> (f32, f32) {
        ((window.0 - 2.0 * self.gutter).max(1.0), (window.1 - 2.0 * self.gutter).max(1.0))
    }

    pub fn window_size(&self, card: (f32, f32)) -> (f32, f32) {
        (card.0 + 2.0 * self.gutter, card.1 + 2.0 * self.gutter)
    }

    pub fn card_rect_in(&self, window: (f32, f32)) -> [f32; 4] {
        let g = self.gutter;
        [g, g, window.0 - 2.0 * g, window.1 - 2.0 * g]
    }

    pub fn min_window_px(&self, min_card: (f32, f32), scale: f64) -> (i32, i32) {
        let g = 2.0 * self.gutter as f64;
        (((min_card.0 as f64 + g) * scale) as i32, ((min_card.1 as f64 + g) * scale) as i32)
    }

    pub fn expand_in_window_units(&self, e: ExpandInfo) -> ExpandInfo {
        let g = 2.0 * self.gutter;
        ExpandInfo { active: e.active, width: e.width.map(|w| w + g), height: e.height.map(|h| h + g) }
    }

    pub fn window_node(&self, key: &str, window: (f32, f32), mut card: Node) -> Node {
        let look = &mut card.look;
        if !self.tint {
            let glass = Color::parse(NEUTRAL_GLASS).expect("valid colour");
            look.fill = glass.with_alpha(look.fill.0[3]);
            look.gradient_bottom = look.gradient_bottom.map(|c| glass.with_alpha(c.0[3]));
        }
        let alpha = |a: f32| match self.bg_alpha {
            Some(bg) => bg,
            None if self.blur => a.min(MAX_FILL_ALPHA_OVER_BLUR),
            None => a,
        };
        look.fill = look.fill.with_alpha(alpha(look.fill.0[3]));
        look.gradient_bottom = look.gradient_bottom.map(|c: Color| c.with_alpha(alpha(c.0[3])));
        if self.blur {
            look.radius = blur_corners(self.radius).1;
        }
        look.shadow = look.shadow.filter(|_| self.shadow > 0.0).map(|mut s| {
            s.color = s.color.mul_alpha(self.shadow);
            s
        });
        fn walk(n: &mut Node, outlines: bool, text_scale: f32) {
            if !outlines {
                n.look.border = 0.0;
            }
            if let Kind::Text(t) = &mut n.kind {
                t.size *= text_scale;
            }
            n.children.iter_mut().for_each(|c| walk(c, outlines, text_scale));
        }
        walk(&mut card, self.outlines, self.text_scale);
        Node::new(format!("{key}~")).wh(window.0, window.1).pad(self.gutter).child(card)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Library, Selection};
    use crate::ui::Kind;
    use crate::value::Value;
    use std::collections::BTreeMap;
    use std::path::Path;

    fn theme_with(pairs: &[(&str, Value)]) -> Theme {
        let layer: BTreeMap<String, Value> = pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        Theme::compose(&Library::load(Path::new("nope")), &Selection::default(), &[&layer])
    }

    fn theme() -> Theme {
        theme_with(&[])
    }

    #[test]
    fn window_and_card_rects_round_trip_at_any_scale() {
        let c = Card::new(&theme());
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
    fn blur_removes_the_gutter() {
        assert_eq!(Card::new(&theme_with(&[("blur", Value::Bool(true))])).gutter, 0.0);
        assert_eq!(Card::new(&theme()).gutter, 20.0);
    }

    #[test]
    fn window_node_adds_the_gutter_and_strips_outlines() {
        let card = || Node::new("c").border(1.0, Color([1.0; 4])).fill(Color([0.1, 0.1, 0.1, 1.0])).child(Node::new("c/0").border(2.0, Color([1.0; 4])));
        let w = Card::new(&theme()).window_node("k", (140.0, 100.0), card());
        assert_eq!((w.key.as_str(), w.children.len()), ("k~", 1));
        assert_eq!(w.children[0].look.border, 1.0);

        let flat = Card::new(&theme_with(&[("outlines", Value::Bool(false))])).window_node("k", (140.0, 100.0), card());
        assert_eq!((flat.children[0].look.border, flat.children[0].children[0].look.border), (0.0, 0.0));
    }

    #[test]
    fn style_tokens_reach_the_card_and_its_text() {
        let card = || Node::new("c").fill(Color([0.2, 0.3, 0.4, 0.9])).shadow(14.0, 5.0, Color([0.0, 0.0, 0.0, 0.5])).child(Node::text("c/t", "hi", 10.0, Color([1.0; 4])));
        let root = |pairs: &[(&str, Value)]| Card::new(&theme_with(pairs)).window_node("k", (100.0, 100.0), card()).children.remove(0);
        let see_through = root(&[("transparent", Value::Bool(true)), ("bg-opacity", Value::Num(40.0))]);
        assert!((see_through.look.fill.0[3] - 0.4).abs() < 1e-6);
        let glass = root(&[("tint", Value::Bool(false))]);
        assert_eq!((glass.look.fill.with_alpha(1.0).to_hex(), glass.look.fill.0[3]), ("#141414".to_string(), 0.9));
        assert!(root(&[("shadow", Value::Num(0.0))]).look.shadow.is_none());
        assert!((root(&[("shadow", Value::Num(50.0))]).look.shadow.unwrap().color.0[3] - 0.25).abs() < 1e-6);
        let big = root(&[("text-scale", Value::Num(120.0))]);
        assert!(matches!(&big.children[0].kind, Kind::Text(t) if (t.size - 12.0).abs() < 1e-4));
        let blurred = root(&[("blur", Value::Bool(true))]);
        assert_eq!((blurred.look.radius, blurred.look.fill.0[3]), (8.0, MAX_FILL_ALPHA_OVER_BLUR), "the default roundness is Windows' standard corner");
        let blurred_see_through = root(&[("blur", Value::Bool(true)), ("transparent", Value::Bool(true)), ("bg-opacity", Value::Num(20.0))]);
        assert!((blurred_see_through.look.fill.0[3] - 0.2).abs() < 1e-6, "transparent wins over the blur cap");
    }

    #[test]
    fn roundness_under_blur_snaps_to_what_windows_can_draw() {
        assert_eq!(blur_corners(0.0), (1, 0.0));
        assert_eq!(blur_corners(4.0), (3, 4.0));
        assert_eq!(blur_corners(22.0), (2, 8.0));
        let c = Card::new(&theme_with(&[("blur", Value::Bool(true)), ("radius-lg", Value::Num(3.0))]));
        let n = c.window_node("k", (100.0, 100.0), Node::new("c").radius(22.0)).children.remove(0);
        assert_eq!((n.look.radius, c.blur_corner_pref()), (4.0, 3));
    }

    #[test]
    fn expand_sizes_gain_the_gutter() {
        let c = Card::new(&theme());
        let e = c.expand_in_window_units(ExpandInfo { active: true, width: Some(100.0), height: None });
        assert_eq!((e.width, e.height), (Some(140.0), None));
    }
}
