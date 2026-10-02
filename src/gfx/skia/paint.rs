//! A `DrawList` onto a Skia canvas. Backend-agnostic: the canvas may sit on a raster
//! surface or any other, and everything here is physical pixels with no transform.
//!
//! The semantics follow what `shader.wgsl` does: painter's order inside a layer is
//! shapes, images, then text; layer 1 is above layer 0; each item's clip is a hard
//! (non-antialiased) rectangle; colours are straight RGBA in, premultiplied out.

use std::collections::HashMap;

use skia_safe::canvas::{SaveLayerRec, SrcRectConstraint};
use skia_safe::gradient::{self, Gradient, Interpolation};
use skia_safe::{
    AlphaType, BlendMode, BlurStyle, Canvas, ClipOp, Color4f, ColorType, Data, FilterMode, Image, ImageInfo, MaskFilter, MipmapMode, Paint, PaintCap,
    PaintStyle, Point, RRect, Rect, RuntimeEffect, SamplingOptions, TileMode, color_filters,
};

use super::glyphs::Glyphs;
use crate::draw::*;
use crate::images::{self, Frames};
use crate::text::TextEngine;

/// How far a soft shadow's Gaussian spreads for a shader `soft` of 1: the sigma whose
/// edge slope at the rect's boundary equals that of the shader's `smoothstep(-s, s, d)`.
const SHADOW_SIGMA: f32 = 0.53;

/// The image edge fade, as a mask: opaque `feather` px inside the rounded rect's edge,
/// clear at the edge, with a smoothstep between (what `fs_img` does).
const FEATHER_SKSL: &str = "
uniform float2 center;
uniform float2 half_size;
uniform float radius;
uniform float feather;
half4 main(float2 p) {
    float2 q = abs(p - center) - half_size + float2(radius);
    float d = length(max(q, float2(0.0))) + min(max(q.x, q.y), 0.0) - radius;
    float t = clamp(-d / feather, 0.0, 1.0);
    t = t * t * (3.0 - 2.0 * t);
    return half4(t);
}
";

/// An uploaded picture: straight-alpha RGBA, or an animation's packed frames.
pub struct SkImg {
    pub image: Image,
    /// Its layout size: one frame's, for an animation.
    pub w: u32,
    pub h: u32,
    /// The whole texture's size, which `uv` is a fraction of.
    pub tex: (u32, u32),
    pub frames: Option<Frames>,
    /// Its pixels' size in memory.
    pub bytes: u64,
}

impl SkImg {
    /// `rgba` is straight alpha; `frames` says how an animation is packed into it.
    pub fn new(rgba: &[u8], w: u32, h: u32, frames: Option<Frames>) -> Option<SkImg> {
        let info = ImageInfo::new((w as i32, h as i32), ColorType::RGBA8888, AlphaType::Unpremul, None);
        let image = skia_safe::images::raster_from_data(&info, Data::new_copy(rgba), w as usize * 4)?;
        let (fw, fh) = frames.as_ref().map_or((w, h), |f| (f.frame_w, f.frame_h));
        Some(SkImg { image, w: fw, h: fh, tex: (w, h), frames, bytes: w as u64 * h as u64 * 4 })
    }
}

pub type Images = HashMap<String, SkImg>;

pub struct Painter {
    glyphs: Glyphs,
    feather: Option<RuntimeEffect>,
}

impl Default for Painter {
    fn default() -> Self {
        Self::new()
    }
}

impl Painter {
    pub fn new() -> Self {
        let feather = RuntimeEffect::make_for_shader(FEATHER_SKSL, None).map_err(|e| eprintln!("wayfinder: skia: feather shader: {e}")).ok();
        Self { glyphs: Glyphs::new(), feather }
    }

    /// Draws `list` onto `canvas`, which must already be cleared. `elapsed_ms` drives
    /// playing animations.
    pub fn paint(&mut self, canvas: &Canvas, list: &DrawList, text: &mut TextEngine, images: &Images, elapsed_ms: u64) {
        for layer in &list.layers {
            for s in &layer.shapes {
                clipped(canvas, s.clip, || shape(canvas, s));
            }
            for d in &layer.images {
                let Some(img) = images.get(&d.tex) else { continue };
                clipped(canvas, d.inst.clip, || self.image(canvas, d, img, elapsed_ms));
            }
            for t in &layer.texts {
                // glyphon's bounds are whole pixels
                let clip = if t.clip == NO_CLIP { NO_CLIP } else { t.clip.map(|v| v as i32 as f32) };
                clipped(canvas, clip, || self.glyphs.draw(canvas, t, text));
            }
        }
    }

    fn image(&self, canvas: &Canvas, d: &ImgDraw, img: &SkImg, elapsed_ms: u64) {
        let inst = &d.inst;
        let mut uv = inst.uv;
        if let Some(f) = &img.frames {
            let i = match d.frame {
                Some(n) => n,
                None if d.play => images::frame_at(f, elapsed_ms),
                None => 0,
            };
            uv = images::compose(images::cell_uv(f, i), uv);
        }
        let (tw, th) = (img.tex.0 as f32, img.tex.1 as f32);
        let src = Rect::new(uv[0] * tw, uv[1] * th, uv[2] * tw, uv[3] * th);
        let (hx, hy) = (inst.half[0], inst.half[1]);
        let dst = Rect::new(inst.center[0] - hx, inst.center[1] - hy, inst.center[0] + hx, inst.center[1] + hy);

        // tint and alpha scale every channel, as the shader's `c * tint * alpha` does
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        let a = inst.tint[3] * inst.alpha;
        if a < 1.0 || inst.tint[..3] != [1.0, 1.0, 1.0] {
            let c = Color4f::new(inst.tint[0], inst.tint[1], inst.tint[2], a).to_color();
            paint.set_color_filter(color_filters::blend(c, BlendMode::Modulate));
        }
        let sampling = SamplingOptions::new(FilterMode::Linear, MipmapMode::None);
        let radius = inst.radius.min(hx.min(hy)).max(0.0);
        let feather = inst.feather > 0.5;

        let layer = feather.then(|| self.feather.as_ref()).flatten();
        if layer.is_some() {
            canvas.save_layer(&SaveLayerRec::default().bounds(&dst));
        }
        canvas.save();
        if radius > 0.5 {
            canvas.clip_rrect(RRect::new_rect_xy(dst, radius, radius), ClipOp::Intersect, true);
        }
        // an animation's frames share one atlas, so keep the filter inside the frame asked for
        let constraint = if img.frames.is_some() { SrcRectConstraint::Strict } else { SrcRectConstraint::Fast };
        canvas.draw_image_rect_with_sampling_options(&img.image, Some((&src, constraint)), dst, sampling, &paint);
        canvas.restore();
        if let Some(effect) = layer {
            let mut u = Vec::with_capacity(24);
            for v in [inst.center[0], inst.center[1], hx, hy, radius, inst.feather] {
                u.extend_from_slice(&v.to_ne_bytes());
            }
            let mut mask = Paint::default();
            mask.set_blend_mode(BlendMode::DstIn);
            mask.set_shader(effect.make_shader(Data::new_copy(&u), &[], None));
            canvas.draw_paint(&mask);
            canvas.restore();
        }
    }
}

/// Runs `draw` inside the item's hard clip, if it has one.
fn clipped(canvas: &Canvas, clip: [f32; 4], draw: impl FnOnce()) {
    if clip == NO_CLIP {
        return draw();
    }
    canvas.save();
    canvas.clip_rect(Rect::new(clip[0], clip[1], clip[2].max(clip[0]), clip[3].max(clip[1])), ClipOp::Intersect, false);
    draw();
    canvas.restore();
}

fn color(c: [f32; 4]) -> Color4f {
    Color4f::new(c[0], c[1], c[2], c[3])
}

fn fill(c: [f32; 4]) -> Paint {
    let mut p = Paint::new(color(c), None);
    p.set_anti_alias(true);
    p
}

fn shape(canvas: &Canvas, s: &Inst) {
    // the shader's own thresholds
    if s.kind > KIND_ARC - 0.5 {
        arc(canvas, s)
    } else if s.kind > KIND_SHADOW - 0.5 {
        shadow(canvas, s)
    } else if s.kind > KIND_CAPSULE - 0.5 {
        capsule(canvas, s)
    } else {
        rect(canvas, s)
    }
}

fn bounds(s: &Inst) -> Rect {
    Rect::new(s.a[0] - s.b[0], s.a[1] - s.b[1], s.a[0] + s.b[0], s.a[1] + s.b[1])
}

fn rrect(r: Rect, radius: f32) -> RRect {
    let radius = radius.clamp(0.0, (r.width().min(r.height())) / 2.0);
    RRect::new_rect_xy(r, radius, radius)
}

fn rect(canvas: &Canvas, s: &Inst) {
    let r = bounds(s);
    let outer = rrect(r, s.radius);
    let mut paint = fill(s.fill_top);
    if s.fill_top != s.fill_bot {
        // straight colours, interpolated before premultiplying (the shader mixes, then premultiplies)
        let ends = [color(s.fill_top), color(s.fill_bot)];
        let colors = gradient::Colors::new(&ends, None, TileMode::Clamp, None);
        let h = r.height().max(1.0);
        paint.set_shader(gradient::shaders::linear_gradient(((0.0, r.top), (0.0, r.top + h)), &Gradient::new(colors, Interpolation::default()), None));
    }
    canvas.draw_rrect(outer, &paint);

    if s.border > 0.0 {
        // the border replaces the fill inside its band (the shader mixes, it doesn't composite)
        let w = s.border.min(r.width().min(r.height()) / 2.0);
        let inner = rrect(r.with_inset((w, w)), s.radius - w);
        canvas.save();
        canvas.clip_rrect(outer, ClipOp::Intersect, true);
        canvas.clip_rrect(inner, ClipOp::Difference, true);
        let mut band = fill(s.border_color);
        band.set_blend_mode(BlendMode::Src);
        canvas.draw_paint(&band);
        canvas.restore();
    }
}

fn capsule(canvas: &Canvas, s: &Inst) {
    if s.radius <= 0.0 {
        return;
    }
    let mut paint = fill(s.fill_top);
    paint.set_style(PaintStyle::Stroke);
    paint.set_stroke_cap(PaintCap::Round);
    paint.set_stroke_width(s.radius * 2.0);
    canvas.draw_line(Point::new(s.a[0], s.a[1]), Point::new(s.b[0], s.b[1]), &paint);
}

fn shadow(canvas: &Canvas, s: &Inst) {
    let mut paint = fill(s.fill_top);
    paint.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, SHADOW_SIGMA * s.soft.max(0.5), false));
    canvas.draw_rrect(rrect(bounds(s), s.radius), &paint);
}

fn arc(canvas: &Canvas, s: &Inst) {
    let (start, sweep) = (s.b[0], s.b[1]);
    let half_stroke = s.border;
    if half_stroke <= 0.0 {
        return;
    }
    let (cx, cy) = (s.a[0], s.a[1]);
    if sweep.abs() < 1e-3 {
        // nothing between the caps: a dot where the arc starts
        let at = Point::new(cx + s.radius * start.sin(), cy - s.radius * start.cos());
        canvas.draw_circle(at, half_stroke, &fill(s.fill_top));
        return;
    }
    let mut paint = fill(s.fill_top);
    paint.set_style(PaintStyle::Stroke);
    paint.set_stroke_cap(PaintCap::Round);
    paint.set_stroke_width(half_stroke * 2.0);
    let oval = Rect::new(cx - s.radius, cy - s.radius, cx + s.radius, cy + s.radius);
    // the shader measures from 12 o'clock, Skia from 3
    canvas.draw_arc(oval, start.to_degrees() - 90.0, sweep.to_degrees(), false, &paint);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;
    use crate::text::TextSpec;
    use skia_safe::surfaces;

    const W: u32 = 100;
    const H: u32 = 100;

    fn inst(kind: f32) -> Inst {
        Inst { kind, clip: NO_CLIP, ..Default::default() }
    }

    fn solid(c: [f32; 4]) -> Inst {
        Inst { fill_top: c, fill_bot: c, ..inst(KIND_RECT) }
    }

    /// Premultiplied RGBA8.
    fn render(list: &DrawList, text: &mut TextEngine, images: &Images) -> Vec<u8> {
        let info = ImageInfo::new((W as i32, H as i32), ColorType::RGBA8888, AlphaType::Premul, None);
        let mut surface = surfaces::raster(&info, None, None).unwrap();
        surface.canvas().clear(skia_safe::Color::TRANSPARENT);
        Painter::new().paint(surface.canvas(), list, text, images, 0);
        let mut out = vec![0u8; (W * H * 4) as usize];
        assert!(surface.read_pixels(&info, &mut out, (W * 4) as usize, (0, 0)));
        out
    }

    fn draw(list: &DrawList) -> Vec<u8> {
        render(list, &mut TextEngine::new(), &Images::new())
    }

    fn at(px: &[u8], x: u32, y: u32) -> [u8; 4] {
        let i = ((y * W + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    }

    fn near(got: [u8; 4], want: [u8; 4], tol: i32) {
        assert!(got.iter().zip(want).all(|(g, w)| (*g as i32 - w as i32).abs() <= tol), "got {got:?}, wanted {want:?} (+-{tol})");
    }

    fn one_shape(s: Inst) -> DrawList {
        let mut l = DrawList::default();
        l.layers[0].shapes.push(s);
        l
    }

    fn one_image(img: SkImg, inst: ImgInst, frame: Option<u32>) -> (DrawList, Images) {
        let mut l = DrawList::default();
        l.layers[0].images.push(ImgDraw { tex: "i".into(), inst, play: false, frame });
        (l, Images::from([("i".to_string(), img)]))
    }

    fn img_inst(radius: f32, feather: f32) -> ImgInst {
        ImgInst { center: [50.0, 50.0], half: [30.0, 30.0], radius, alpha: 1.0, feather, tint: [1.0; 4], clip: NO_CLIP, uv: [0.0, 0.0, 1.0, 1.0], ..Default::default() }
    }

    fn white() -> SkImg {
        SkImg::new(&[255; 4 * 4 * 4], 4, 4, None).unwrap()
    }

    #[test]
    fn an_opaque_rect_fills_exactly_its_bounds() {
        let px = draw(&one_shape(Inst { a: [50.0, 50.0], b: [20.0, 10.0], ..solid([1.0, 0.0, 0.0, 1.0]) }));
        near(at(&px, 50, 50), [255, 0, 0, 255], 1);
        near(at(&px, 31, 41), [255, 0, 0, 255], 1);
        assert_eq!(at(&px, 28, 50)[3], 0);
        assert_eq!(at(&px, 50, 38)[3], 0);
    }

    #[test]
    fn colours_are_straight_in_and_premultiplied_out() {
        let px = draw(&one_shape(Inst { a: [50.0, 50.0], b: [20.0, 20.0], ..solid([1.0, 0.5, 0.0, 0.5]) }));
        near(at(&px, 50, 50), [128, 64, 0, 128], 2);
    }

    #[test]
    fn a_vertical_gradient_runs_from_the_top_colour_to_the_bottom_one() {
        let g = Inst { a: [50.0, 50.0], b: [20.0, 40.0], fill_top: [1.0, 0.0, 0.0, 1.0], fill_bot: [0.0, 0.0, 1.0, 1.0], ..inst(KIND_RECT) };
        let px = draw(&one_shape(g));
        near(at(&px, 50, 11), [255, 0, 0, 255], 12);
        near(at(&px, 50, 88), [0, 0, 255, 255], 12);
        let mid = at(&px, 50, 50);
        assert!(mid[0] > 90 && mid[2] > 90, "the middle mixes both: {mid:?}");
    }

    #[test]
    fn a_rounded_corner_is_clear_where_a_square_one_is_not() {
        let px = draw(&one_shape(Inst { a: [50.0, 50.0], b: [30.0, 30.0], radius: 15.0, ..solid([1.0; 4]) }));
        assert_eq!(at(&px, 21, 21)[3], 0, "the corner pixel is outside the arc");
        near(at(&px, 50, 21), [255; 4], 1);
    }

    #[test]
    fn a_border_replaces_the_fill_in_its_band_instead_of_covering_it() {
        let r = Inst { a: [50.0, 50.0], b: [30.0, 30.0], border: 6.0, border_color: [1.0, 0.0, 0.0, 0.5], ..solid([0.0, 1.0, 0.0, 1.0]) };
        let px = draw(&one_shape(r));
        near(at(&px, 50, 50), [0, 255, 0, 255], 1);
        // half-transparent red alone, with no green left under it
        near(at(&px, 50, 22), [128, 0, 0, 128], 3);
        near(at(&px, 22, 50), [128, 0, 0, 128], 3);
    }

    #[test]
    fn a_hard_clip_cuts_a_shape_to_its_rectangle() {
        let r = Inst { a: [50.0, 50.0], b: [40.0, 40.0], clip: [40.0, 40.0, 60.0, 60.0], ..solid([1.0; 4]) };
        let px = draw(&one_shape(r));
        near(at(&px, 50, 50), [255; 4], 1);
        assert_eq!((at(&px, 39, 50)[3], at(&px, 60, 50)[3], at(&px, 50, 39)[3], at(&px, 50, 60)[3]), (0, 0, 0, 0));
    }

    #[test]
    fn a_capsule_is_round_ended_and_a_point_is_a_dot() {
        let px = draw(&one_shape(Inst { a: [20.0, 50.0], b: [80.0, 50.0], radius: 5.0, fill_top: [1.0; 4], ..inst(KIND_CAPSULE) }));
        near(at(&px, 50, 50), [255; 4], 1);
        near(at(&px, 17, 50), [255; 4], 1); // the cap reaches past the end point
        assert_eq!(at(&px, 15, 45)[3], 0, "the cap is round, not square");
        let dot = draw(&one_shape(Inst { a: [50.0, 50.0], b: [50.0, 50.0], radius: 8.0, fill_top: [1.0; 4], ..inst(KIND_CAPSULE) }));
        near(at(&dot, 50, 50), [255; 4], 1);
        assert_eq!(at(&dot, 43, 43)[3], 0);
    }

    #[test]
    fn an_arc_starts_at_twelve_o_clock_and_sweeps_clockwise() {
        let quarter = Inst { a: [50.0, 50.0], b: [0.0, std::f32::consts::FRAC_PI_2], radius: 30.0, border: 4.0, fill_top: [1.0; 4], ..inst(KIND_ARC) };
        let px = draw(&one_shape(quarter));
        near(at(&px, 50, 20), [255; 4], 1); // 12 o'clock
        near(at(&px, 80, 50), [255; 4], 1); // 3 o'clock, where the sweep ends
        assert_eq!(at(&px, 50, 80)[3], 0, "6 o'clock is outside the sweep");
        assert_eq!(at(&px, 20, 50)[3], 0, "9 o'clock is outside the sweep");
        near(at(&px, 50, 50), [0; 4], 0);
    }

    #[test]
    fn an_arc_with_no_sweep_is_a_dot_where_it_starts() {
        let dot = Inst { a: [50.0, 50.0], b: [std::f32::consts::FRAC_PI_2, 0.0], radius: 30.0, border: 5.0, fill_top: [1.0; 4], ..inst(KIND_ARC) };
        let px = draw(&one_shape(dot));
        near(at(&px, 80, 50), [255; 4], 1);
        assert_eq!(at(&px, 50, 20)[3], 0);
    }

    #[test]
    fn a_full_sweep_closes_the_ring() {
        let ring = Inst { a: [50.0, 50.0], b: [0.0, 7.0], radius: 30.0, border: 3.0, fill_top: [1.0; 4], ..inst(KIND_ARC) };
        let px = draw(&one_shape(ring));
        for (x, y) in [(50, 20), (80, 50), (50, 80), (20, 50)] {
            near(at(&px, x, y), [255; 4], 1);
        }
    }

    #[test]
    fn a_shadow_is_half_strength_at_the_edge_and_fades_out_beyond_it() {
        let s = Inst { a: [50.0, 50.0], b: [20.0, 20.0], radius: 4.0, soft: 6.0, fill_top: [0.0, 0.0, 0.0, 1.0], ..inst(KIND_SHADOW) };
        let px = draw(&one_shape(s));
        assert!(at(&px, 50, 50)[3] >= 250, "solid in the middle");
        let edge = at(&px, 50, 30)[3] as i32;
        assert!((edge - 128).abs() < 40, "about half at the edge: {edge}");
        assert!(at(&px, 50, 24)[3] < at(&px, 50, 28)[3], "fading outwards");
        assert_eq!(at(&px, 50, 5)[3], 0, "gone well past the edge");
    }

    #[test]
    fn layer_one_is_above_layer_zero_whatever_the_order_inside() {
        let mut l = DrawList::default();
        l.layers[1].shapes.push(Inst { a: [50.0, 50.0], b: [20.0, 20.0], ..solid([0.0, 0.0, 1.0, 1.0]) });
        l.layers[0].shapes.push(Inst { a: [50.0, 50.0], b: [30.0, 30.0], ..solid([1.0, 0.0, 0.0, 1.0]) });
        let px = draw(&l);
        near(at(&px, 50, 50), [0, 0, 255, 255], 1);
        near(at(&px, 25, 50), [255, 0, 0, 255], 1);
    }

    #[test]
    fn inside_a_layer_images_are_above_shapes() {
        let (mut l, images) = one_image(white(), img_inst(0.0, 0.0), None);
        l.layers[0].shapes.push(Inst { a: [50.0, 50.0], b: [40.0, 40.0], ..solid([1.0, 0.0, 0.0, 1.0]) });
        let px = render(&l, &mut TextEngine::new(), &images);
        near(at(&px, 50, 50), [255; 4], 1);
        near(at(&px, 15, 50), [255, 0, 0, 255], 1);
    }

    #[test]
    fn a_missing_image_is_skipped() {
        let mut l = DrawList::default();
        l.layers[0].images.push(ImgDraw { tex: "gone".into(), inst: img_inst(0.0, 0.0), play: false, frame: None });
        assert!(draw(&l).iter().all(|b| *b == 0));
    }

    #[test]
    fn tint_and_alpha_scale_the_picture_like_the_shader_does() {
        let mut i = img_inst(0.0, 0.0);
        i.tint = [1.0, 0.0, 0.5, 1.0];
        i.alpha = 0.5;
        let (l, images) = one_image(white(), i, None);
        let px = render(&l, &mut TextEngine::new(), &images);
        near(at(&px, 50, 50), [128, 0, 64, 128], 3);
    }

    #[test]
    fn the_uv_rectangle_picks_part_of_the_picture() {
        let mut rgba = Vec::new();
        for _ in 0..2 {
            rgba.extend_from_slice(&[255, 0, 0, 255, 0, 0, 255, 255]); // red | blue
        }
        let mut i = img_inst(0.0, 0.0);
        i.uv = [0.5, 0.0, 1.0, 1.0];
        let (l, images) = one_image(SkImg::new(&rgba, 2, 2, None).unwrap(), i, None);
        let px = render(&l, &mut TextEngine::new(), &images);
        near(at(&px, 50, 50), [0, 0, 255, 255], 1);
        near(at(&px, 75, 25), [0, 0, 255, 255], 1);
    }

    #[test]
    fn a_picture_with_a_radius_is_clipped_to_rounded_corners() {
        let (l, images) = one_image(white(), img_inst(15.0, 0.0), None);
        let px = render(&l, &mut TextEngine::new(), &images);
        assert_eq!(at(&px, 21, 21)[3], 0);
        near(at(&px, 50, 50), [255; 4], 1);
    }

    #[test]
    fn a_feather_fades_the_picture_out_towards_its_edge() {
        let (l, images) = one_image(white(), img_inst(0.0, 12.0), None);
        let px = render(&l, &mut TextEngine::new(), &images);
        near(at(&px, 50, 50), [255; 4], 1);
        let (edge, inner) = (at(&px, 50, 21)[3], at(&px, 50, 27)[3]);
        assert!(edge < 40, "nearly clear at the edge: {edge}");
        assert!(inner > edge + 40 && inner < 255, "partway in: {inner}");
    }

    #[test]
    fn an_animation_shows_the_frame_asked_for() {
        let cells = vec![(image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255])), 100), (image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 255, 0, 255])), 100)];
        let d = crate::images::pack(cells);
        let make = || SkImg::new(&d.px, d.w, d.h, d.frames.clone()).unwrap();
        for (frame, want) in [(0, [255, 0, 0, 255]), (1, [0, 255, 0, 255])] {
            let (l, images) = one_image(make(), img_inst(0.0, 0.0), Some(frame));
            let px = render(&l, &mut TextEngine::new(), &images);
            near(at(&px, 50, 50), want, 1);
        }
        assert_eq!((make().w, make().h), (2, 2), "the layout size is one frame's");
    }

    fn text_list(clip: [f32; 4]) -> (DrawList, TextEngine) {
        text_of("Hello MMMMMM", clip)
    }

    fn text_of(text: &str, clip: [f32; 4]) -> (DrawList, TextEngine) {
        let mut te = TextEngine::new();
        te.begin_frame();
        let spec = TextSpec { text: text.into(), size: 20.0, color: Color([1.0; 4]), ..Default::default() };
        te.measure("t", &spec, None);
        te.prepare("t", &spec, 200.0);
        let mut l = DrawList::default();
        l.layers[0].texts.push(TextItem { key: "t".into(), x: 5.0, y: 30.0, scale: 1.0, color: [255, 255, 255, 255], clip });
        (l, te)
    }

    fn lit(px: &[u8], x0: u32, x1: u32) -> usize {
        (0..H).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| at(px, x, y)[3] > 0).count()
    }

    #[test]
    fn text_is_drawn_and_a_clip_stops_it() {
        let (l, mut te) = text_list(NO_CLIP);
        let free = render(&l, &mut te, &Images::new());
        assert!(lit(&free, 0, W) > 50, "some glyphs were drawn");
        assert!(lit(&free, 60, W) > 0, "the text runs past x=60");

        let (l, mut te) = text_list([0.0, 0.0, 60.0, 100.0]);
        let cut = render(&l, &mut te, &Images::new());
        assert!(lit(&cut, 0, 60) > 0);
        assert_eq!(lit(&cut, 60, W), 0, "nothing past the clip");
    }

    #[test]
    fn text_takes_its_colour_and_scale_from_the_item() {
        let (mut l, mut te) = text_of("Hi", NO_CLIP);
        let small = lit(&render(&l, &mut te, &Images::new()), 0, W);
        l.layers[0].texts[0].scale = 2.0;
        l.layers[0].texts[0].color = [255, 0, 0, 255];
        let big = render(&l, &mut te, &Images::new());
        assert!(lit(&big, 0, W) > small * 2, "twice the size covers much more");
        let strongest = big.chunks_exact(4).max_by_key(|p| p[3]).unwrap();
        assert!(strongest[0] > 200 && strongest[1] == 0 && strongest[2] == 0, "red: {strongest:?}");
    }
}
