use super::{ElementKind, Shape, ShapeCx, rgba_with_opacity};
use crate::color::Color;
use crate::draw::{Inst, KIND_CAPSULE, KIND_RECT};
use crate::format::Attrs;
use crate::ui::Kind;
use crate::value::Value;

/// A line through `values`, oldest on the left, scaled so `max` touches the top.
pub const KIND: ElementKind = ElementKind { name: "graph", own_attrs: &["values", "max", "stroke", "color", "area"], fills_parent_when_unsized: false, build };

#[derive(Clone, Debug)]
pub struct GraphSpec {
    pub values: Vec<f32>,
    pub max: f32,
    pub stroke: f32,
    pub color: Color,
    /// Fills under the line, fading down; `None` draws the line only.
    pub area: Option<Color>,
}

fn build(a: &mut Attrs) -> Result<Kind, String> {
    let values = match a.value("values")? {
        Some(Value::List(l)) => l.iter().filter_map(|v| v.as_f64()).map(|v| v as f32).collect(),
        _ => vec![],
    };
    Ok(Kind::shape(GraphSpec {
        values,
        max: a.num("max")?.unwrap_or(100.0).max(1e-3),
        stroke: a.num("stroke")?.unwrap_or(2.0).max(0.5),
        color: a.color("color")?.unwrap_or(a.theme().color("accent")),
        area: a.color("area")?,
    }))
}

impl Shape for GraphSpec {
    fn name(&self) -> &'static str {
        "graph"
    }

    fn emit(&self, cx: &ShapeCx, out: &mut Vec<Inst>) {
        let n = self.values.len();
        if n < 2 {
            return;
        }
        let ((w, h), s) = (cx.logical_size, cx.scale);
        let r = self.stroke * s / 2.0;
        // inset by the stroke so the line never pokes out of the box
        let (left, top) = (cx.center_px[0] - w * s / 2.0 + r, cx.center_px[1] - h * s / 2.0 + r);
        let (span_w, span_h) = ((w * s - 2.0 * r).max(1.0), (h * s - 2.0 * r).max(1.0));
        let point = |i: usize| [left + span_w * i as f32 / (n - 1) as f32, top + span_h * (1.0 - (self.values[i] / self.max).clamp(0.0, 1.0))];
        let op = cx.inherited_opacity;
        if let Some(area) = self.area {
            let col_w = span_w / (n - 1) as f32;
            let bottom = top + span_h;
            for i in 0..n {
                let [x, y] = point(i);
                out.push(Inst {
                    a: [x, (y + bottom) / 2.0],
                    b: [col_w / 2.0 + 0.5, ((bottom - y) / 2.0).max(0.0)],
                    kind: KIND_RECT,
                    fill_top: rgba_with_opacity(area, op),
                    fill_bot: rgba_with_opacity(area.mul_alpha(0.15), op),
                    clip: cx.clip_px,
                    ..Default::default()
                });
            }
        }
        for i in 0..n - 1 {
            out.push(Inst {
                a: point(i),
                b: point(i + 1),
                radius: r,
                kind: KIND_CAPSULE,
                fill_top: rgba_with_opacity(self.color, op),
                fill_bot: rgba_with_opacity(self.color, op),
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

    fn emit(g: &GraphSpec) -> Vec<Inst> {
        let cx = ShapeCx { center_px: [50.0, 25.0], logical_size: (100.0, 50.0), scale: 1.0, inherited_opacity: 1.0, clip_px: NO_CLIP };
        let mut out = Vec::new();
        g.emit(&cx, &mut out);
        out
    }

    #[test]
    fn a_line_segment_per_step_scaled_to_max_and_inside_the_box() {
        let g = GraphSpec { values: vec![0.0, 50.0, 100.0], max: 100.0, stroke: 2.0, color: Color([1.0; 4]), area: None };
        let out = emit(&g);
        assert_eq!(out.len(), 2, "one capsule per step");
        assert_eq!((out[0].a, out[1].b), ([1.0, 49.0], [99.0, 1.0]), "0 sits on the bottom, max on the top, inset by the stroke");
        assert_eq!(out[0].b[1], 25.0, "50 is halfway up");
        let filled = emit(&GraphSpec { area: Some(Color([1.0, 0.0, 0.0, 0.5])), ..g.clone() });
        assert_eq!(filled.len(), 2 + 3, "plus one fading column per sample");
        assert!(emit(&GraphSpec { values: vec![42.0], ..g }).is_empty(), "a single sample draws nothing");
    }
}
