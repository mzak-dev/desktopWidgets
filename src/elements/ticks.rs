use super::{ElementKind, Shape, ShapeCx, rgba_with_opacity};
use crate::color::Color;
use crate::draw::{Inst, KIND_CAPSULE};
use crate::format::Attrs;
use crate::ui::Kind;

pub const KIND: ElementKind = ElementKind {
    name: "ticks",
    own_attrs: &["count", "major_every", "tick_length", "major_length", "tick_width", "major_width", "color", "major_color", "tick_inset"],
    fills_parent_when_unsized: true,
    build,
};

#[derive(Clone, Copy, Debug)]
pub struct TicksSpec {
    pub count: u32,
    pub major_every: u32,
    pub len: f32,
    pub major_len: f32,
    pub width: f32,
    pub major_width: f32,
    pub color: Color,
    pub major_color: Color,
    /// Distance from the rim, px.
    pub inset: f32,
}

fn build(a: &mut Attrs) -> Result<Kind, String> {
    let (tick, hand) = (a.theme().color("tick"), a.theme().color("hand"));
    Ok(Kind::shape(TicksSpec {
        count: a.num("count")?.unwrap_or(60.0).max(1.0) as u32,
        major_every: a.num("major_every")?.unwrap_or(5.0) as u32,
        len: a.num("tick_length")?.unwrap_or(5.0),
        major_len: a.num("major_length")?.unwrap_or(10.0),
        width: a.num("tick_width")?.unwrap_or(1.2),
        major_width: a.num("major_width")?.unwrap_or(2.4),
        color: a.color("color")?.unwrap_or(tick),
        major_color: a.color("major_color")?.unwrap_or(hand),
        inset: a.num("tick_inset")?.unwrap_or(8.0),
    }))
}

impl Shape for TicksSpec {
    fn name(&self) -> &'static str {
        "ticks"
    }

    fn emit(&self, cx: &ShapeCx, out: &mut Vec<Inst>) {
        let ((w, h), s, [x, y]) = (cx.logical_size, cx.scale, cx.center_px);
        let rad = (w.min(h) / 2.0 - self.inset) * s;
        for i in 0..self.count {
            let major = self.major_every > 0 && i % self.major_every == 0;
            let (len, wd, col) = if major { (self.major_len, self.major_width, self.major_color) } else { (self.len, self.width, self.color) };
            let a = (i as f32 / self.count as f32) * std::f32::consts::TAU;
            let dir = [a.sin(), -a.cos()];
            out.push(Inst {
                a: [x + dir[0] * rad, y + dir[1] * rad],
                b: [x + dir[0] * (rad - len * s), y + dir[1] * (rad - len * s)],
                radius: wd * s / 2.0,
                kind: KIND_CAPSULE,
                fill_top: rgba_with_opacity(col, cx.inherited_opacity),
                fill_bot: rgba_with_opacity(col, cx.inherited_opacity),
                clip: cx.clip_px,
                ..Default::default()
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::NO_CLIP;

    #[test]
    fn one_capsule_per_tick() {
        let t = TicksSpec { count: 60, major_every: 5, len: 5.0, major_len: 10.0, width: 1.0, major_width: 2.0, color: Color([1.0; 4]), major_color: Color([1.0; 4]), inset: 8.0 };
        let mut out = Vec::new();
        t.emit(&ShapeCx { center_px: [100.0, 100.0], logical_size: (200.0, 200.0), scale: 1.0, inherited_opacity: 1.0, clip_px: NO_CLIP }, &mut out);
        assert_eq!(out.len(), 60);
        assert_eq!((out[0].radius, out[1].radius), (1.0, 0.5), "every fifth tick is major");
    }
}
