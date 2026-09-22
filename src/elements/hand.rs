//! `hand`: a clock-style hand from the centre of its rect.

use super::{ElementKind, Shape, ShapeCx, rgba};
use crate::color::Color;
use crate::draw::{Inst, KIND_CAPSULE};
use crate::format::Attrs;
use crate::ui::Kind;

pub const KIND: ElementKind = ElementKind { name: "hand", attrs: &["angle", "length", "tail", "stroke", "color"], fills_parent: true, build };

#[derive(Clone, Copy, Debug)]
pub struct HandSpec {
    /// Degrees clockwise from 12 o'clock.
    pub angle: f32,
    /// Fractions of the radius (half the smaller side).
    pub length: f32,
    pub tail: f32,
    pub width: f32,
    pub color: Color,
}

fn build(a: &mut Attrs) -> Result<Kind, String> {
    let hand = a.theme().color("hand");
    Ok(Kind::shape(HandSpec {
        angle: a.num("angle")?.unwrap_or(0.0),
        length: a.num("length")?.unwrap_or(0.8),
        tail: a.num("tail")?.unwrap_or(0.1),
        width: a.num("stroke")?.unwrap_or(2.0),
        color: a.color("color")?.unwrap_or(hand),
    }))
}

impl Shape for HandSpec {
    fn name(&self) -> &'static str {
        "hand"
    }

    fn emit(&self, cx: &ShapeCx, out: &mut Vec<Inst>) {
        let ((w, h), s, [x, y]) = (cx.size, cx.scale, cx.center);
        let rad = (w.min(h) / 2.0) * s;
        let a = self.angle.to_radians();
        let dir = [a.sin(), -a.cos()];
        let (p0, p1) = ([x - dir[0] * rad * self.tail, y - dir[1] * rad * self.tail], [x + dir[0] * rad * self.length, y + dir[1] * rad * self.length]);
        out.push(Inst {
            a: p0,
            b: p1,
            radius: self.width * s / 2.0,
            kind: KIND_CAPSULE,
            fill_top: rgba(self.color, cx.opacity),
            fill_bot: rgba(self.color, cx.opacity),
            clip: cx.clip,
            ..Default::default()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::NO_CLIP;

    #[test]
    fn a_hand_is_one_capsule_pointing_at_its_angle() {
        let h = HandSpec { angle: 90.0, length: 0.5, tail: 0.0, width: 2.0, color: Color([1.0; 4]) };
        let mut out = Vec::new();
        h.emit(&ShapeCx { center: [100.0, 100.0], size: (200.0, 200.0), scale: 1.0, opacity: 1.0, clip: NO_CLIP }, &mut out);
        assert_eq!(out.len(), 1);
        assert!((out[0].b[0] - 150.0).abs() < 1e-3 && (out[0].b[1] - 100.0).abs() < 1e-3, "3 o'clock is to the right");
    }
}
