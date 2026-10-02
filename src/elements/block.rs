use super::{Attrs, ElementKind, Kind, Shape, ShapeCx, num_str, rgba_with_opacity};
use crate::color::Color;
use crate::draw::{Inst, KIND_RECT};


/// A block seen from the front, a little from above and from the left: its front, a lit top
/// and a shaded right side reaching `depth` px back, up and to the right. The front is the
/// box less `depth` on the right and on the top, fading from `color` to `color_bottom`. A box
/// no taller than its depth is a flat tile.
pub const KIND: ElementKind = ElementKind { name: "block", own_attrs: &["depth", "color", "color_bottom"], fills_parent_when_unsized: false, build };

/// How far toward white the top is, and toward black the side.
const LIGHT: f32 = 0.3;
const SHADE: f32 = 0.45;

#[derive(Clone, Copy, Debug)]
pub struct BlockSpec {
    pub depth: f32,
    pub color: Color,
    pub bottom: Color,
}

fn build(a: &mut Attrs) -> Result<Kind, String> {
    let color = a.color("color")?.unwrap_or(a.theme().color("accent"));
    Ok(Kind::shape(BlockSpec { depth: a.num("depth")?.unwrap_or(6.0).max(0.0), color, bottom: a.color("color_bottom")?.unwrap_or(color) }))
}

impl Shape for BlockSpec {
    fn name(&self) -> &'static str {
        "block"
    }

    fn describe(&self) -> (&'static str, Vec<(&'static str, String)>) {
        ("block", vec![("depth", num_str(self.depth)), ("color", self.color.to_hex()), ("color_bottom", self.bottom.to_hex())])
    }

    fn emit(&self, cx: &ShapeCx, out: &mut Vec<Inst>) {
        let ((w, h), s) = (cx.logical_size, cx.scale);
        // on whole pixels, so the slices below meet without seams
        let (left, right) = ((cx.center_px[0] - w * s / 2.0).round(), (cx.center_px[0] + w * s / 2.0).round());
        let (top, bottom) = ((cx.center_px[1] - h * s / 2.0).round(), (cx.center_px[1] + h * s / 2.0).round());
        let d = (self.depth * s).round().min(right - left).min(bottom - top).max(0.0);
        // the front's right and top edges
        let (fr, ft) = (right - d, top + d);
        let op = cx.inherited_opacity;
        let rect = |x0: f32, y0: f32, x1: f32, y1: f32, hi: Color, lo: Color| Inst {
            a: [(x0 + x1) / 2.0, (y0 + y1) / 2.0],
            b: [(x1 - x0) / 2.0, (y1 - y0) / 2.0],
            kind: KIND_RECT,
            fill_top: rgba_with_opacity(hi, op),
            fill_bot: rgba_with_opacity(lo, op),
            clip: cx.clip_px,
            ..Default::default()
        };
        let lit = self.color.lerp(Color([1.0, 1.0, 1.0, self.color.0[3]]), LIGHT);
        let shade = |c: Color| c.lerp(Color([0.0, 0.0, 0.0, c.0[3]]), SHADE);
        // a pixel's column of the side and row of the top at a time, each moved along the
        // slant by its middle, so the diagonal edges are antialiased like any other
        for k in 0..d as usize {
            let (k, t) = (k as f32, k as f32 + 0.5);
            if bottom > ft {
                out.push(rect(fr + k, ft - t, fr + k + 1.0, bottom - t, shade(self.color), shade(self.bottom)));
            }
            if fr > left {
                out.push(rect(left + t, ft - k - 1.0, fr + t, ft - k, lit, lit));
            }
        }
        if bottom > ft && fr > left {
            out.push(rect(left, ft, fr, bottom, self.color, self.bottom));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::NO_CLIP;

    fn emit(b: &BlockSpec, w: f32, h: f32) -> Vec<Inst> {
        let cx = ShapeCx { center_px: [w / 2.0, h / 2.0], logical_size: (w, h), scale: 1.0, inherited_opacity: 1.0, clip_px: NO_CLIP };
        let mut out = Vec::new();
        b.emit(&cx, &mut out);
        out
    }

    #[test]
    fn a_front_a_lit_top_and_a_shaded_side() {
        let b = BlockSpec { depth: 4.0, color: Color([0.5, 0.5, 0.5, 1.0]), bottom: Color([0.0, 0.0, 1.0, 1.0]) };
        let out = emit(&b, 20.0, 30.0);
        assert_eq!(out.len(), 4 * 2 + 1, "a column of side and a row of top per pixel of depth, and the front");
        let front = out.last().unwrap();
        assert_eq!((front.a, front.b), ([8.0, 17.0], [8.0, 13.0]), "the front is 16 x 26, under the top and left of the side");
        assert_eq!((front.fill_top, front.fill_bot), ([0.5, 0.5, 0.5, 1.0], [0.0, 0.0, 1.0, 1.0]));
        let (side, top) = (&out[0], &out[1]);
        assert!(top.fill_top[0] > 0.6 && side.fill_top[0] < 0.3, "a lit top and a shaded side: {:?} {:?}", top.fill_top, side.fill_top);
        assert_eq!((side.a, side.b), ([16.5, 16.5], [0.5, 13.0]), "the side's first column, half a pixel up the slant");
        assert_eq!((out[7].a, out[7].b), ([11.5, 0.5], [8.0, 0.5]), "the top's last row, 3.5 px along the slant");
        let tile = emit(&b, 20.0, 3.0);
        assert!(tile.len() == 3 && tile.iter().all(|i| i.a[1] < 3.0 && i.fill_top == top.fill_top), "no taller than its depth, it is a flat tile");
    }
}
