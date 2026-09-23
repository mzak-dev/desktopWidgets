use super::{ElementKind, Shape, ShapeCx, rgba_with_opacity};
use crate::color::Color;
use crate::draw::{Inst, KIND_CAPSULE, KIND_RECT};
use crate::format::Attrs;
use crate::ui::Kind;
use crate::value::Value;

/// A line through `values`, newest on the right, scaled so `max` touches the top. With
/// `span` slots across (e.g. a minute of samples) a short history fills in from the right.
pub const KIND: ElementKind = ElementKind { name: "graph", own_attrs: &["values", "max", "span", "stroke", "color", "area"], fills_parent_when_unsized: false, build };

#[derive(Clone, Debug)]
pub struct GraphSpec {
    pub values: Vec<f32>,
    pub max: f32,
    /// Slots across; at least `values.len()`.
    pub span: usize,
    pub stroke: f32,
    pub color: Color,
    /// Fills under the line, fading from 35% to 5% of this colour; `None` draws the line only.
    pub area: Option<Color>,
}

fn build(a: &mut Attrs) -> Result<Kind, String> {
    let values = match a.value("values")? {
        Some(Value::List(l)) => l.iter().filter_map(|v| v.as_f64()).map(|v| v as f32).collect(),
        _ => vec![],
    };
    let span = (a.num("span")?.unwrap_or(0.0).max(0.0) as usize).max(values.len());
    Ok(Kind::shape(GraphSpec {
        values,
        span,
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
        let span = self.span.max(n);
        let step = span_w / (span - 1) as f32;
        // the newest sample sits on the right edge
        let x0 = left + span_w - step * (n - 1) as f32;
        let y_of = |v: f32| top + span_h * (1.0 - (v / self.max).clamp(0.0, 1.0));
        let point = |i: usize| [x0 + step * i as f32, y_of(self.values[i])];
        let op = cx.inherited_opacity;
        if let Some(area) = self.area {
            // thin slices under the line, so the area follows its slope instead of stepping;
            // their edges sit on whole pixels, where antialiasing adds no seam between them
            let bottom = top + span_h;
            let slice = 2.0;
            let mut x = x0.ceil();
            while x < left + span_w {
                let t = ((x - x0) / step).clamp(0.0, (n - 1) as f32);
                let i = (t.floor() as usize).min(n - 2);
                let y = y_of(self.values[i] + (self.values[i + 1] - self.values[i]) * (t - i as f32));
                out.push(Inst {
                    a: [x + slice / 2.0, (y + bottom) / 2.0],
                    b: [slice / 2.0, ((bottom - y) / 2.0).max(0.0)],
                    kind: KIND_RECT,
                    fill_top: rgba_with_opacity(area.mul_alpha(0.35), op),
                    fill_bot: rgba_with_opacity(area.mul_alpha(0.05), op),
                    clip: cx.clip_px,
                    ..Default::default()
                });
                x += slice;
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
        let g = GraphSpec { values: vec![0.0, 50.0, 100.0], max: 100.0, span: 3, stroke: 2.0, color: Color([1.0; 4]), area: None };
        let out = emit(&g);
        assert_eq!(out.len(), 2, "one capsule per step");
        assert_eq!((out[0].a, out[1].b), ([1.0, 49.0], [99.0, 1.0]), "0 sits on the bottom, max on the top, inset by the stroke");
        assert_eq!(out[0].b[1], 25.0, "50 is halfway up");
        let filled = emit(&GraphSpec { area: Some(Color([1.0, 0.0, 0.0, 0.5])), ..g.clone() });
        assert_eq!(filled.len(), 2 + 49, "plus a 2 px slice every 2 px across the 98 px span");
        assert!(filled[2..].iter().all(|i| i.a[1] - i.b[1] >= 1.0 - 1e-3), "slices start on the line: the middle one at 25 px");
        assert!(emit(&GraphSpec { values: vec![42.0], ..g.clone() }).is_empty(), "a single sample draws nothing");
        let young = emit(&GraphSpec { values: vec![10.0, 20.0], span: 60, ..g });
        assert_eq!((young.len(), young[0].b[0]), (1, 99.0), "a short history hugs the right edge");
        assert!((young[0].a[0] - (99.0 - 98.0 / 59.0)).abs() < 1e-3);
    }
}
