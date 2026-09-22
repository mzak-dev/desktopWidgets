//! `arc`: a gauge, a track ring with a value arc over it, sized by the smaller side.

use super::{ElementKind, Shape, ShapeCx, rgba};
use crate::color::Color;
use crate::draw::{Inst, KIND_ARC};
use crate::format::Attrs;
use crate::ui::Kind;

pub const KIND: ElementKind = ElementKind { name: "arc", attrs: &["value", "start", "sweep", "stroke", "color", "track"], fills_parent: true, build };

#[derive(Clone, Copy, Debug)]
pub struct ArcSpec {
    /// Degrees clockwise from 12 o'clock.
    pub start: f32,
    pub sweep: f32,
    /// 0..=100.
    pub value: f32,
    pub width: f32,
    pub color: Color,
    pub track: Color,
}

fn build(a: &mut Attrs) -> Result<Kind, String> {
    let (accent, track) = (a.theme().color("accent"), a.theme().color("track"));
    Ok(Kind::shape(ArcSpec {
        start: a.num("start")?.unwrap_or(225.0),
        sweep: a.num("sweep")?.unwrap_or(270.0).clamp(1.0, 360.0),
        value: a.num("value")?.unwrap_or(0.0).clamp(0.0, 100.0),
        width: a.num("stroke")?.unwrap_or(6.0).max(1.0),
        color: a.color("color")?.unwrap_or(accent),
        track: a.color("track")?.unwrap_or(track),
    }))
}

impl Shape for ArcSpec {
    fn name(&self) -> &'static str {
        "arc"
    }

    fn emit(&self, cx: &ShapeCx, out: &mut Vec<Inst>) {
        let ((w, h), s) = (cx.size, cx.scale);
        let radius = (w.min(h) - self.width) / 2.0 * s;
        let mut push = |sweep: f32, col: Color| {
            out.push(Inst {
                a: cx.center,
                b: [self.start.to_radians(), sweep.to_radians()],
                radius,
                border: self.width * s / 2.0,
                kind: KIND_ARC,
                fill_top: rgba(col, cx.opacity),
                fill_bot: rgba(col, cx.opacity),
                clip: cx.clip,
                ..Default::default()
            });
        };
        push(self.sweep, self.track);
        if self.value > 0.0 {
            push(self.sweep * self.value.min(100.0) / 100.0, self.color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::NO_CLIP;

    #[test]
    fn a_track_always_and_a_value_arc_only_above_zero() {
        let arc = |value| ArcSpec { start: 225.0, sweep: 270.0, value, width: 8.0, color: Color([1.0; 4]), track: Color([0.5; 4]) };
        let cx = ShapeCx { center: [50.0, 50.0], size: (100.0, 100.0), scale: 1.0, opacity: 1.0, clip: NO_CLIP };
        let count = |v| {
            let mut out = Vec::new();
            arc(v).emit(&cx, &mut out);
            out.len()
        };
        assert_eq!((count(0.0), count(50.0)), (1, 2));
    }
}
