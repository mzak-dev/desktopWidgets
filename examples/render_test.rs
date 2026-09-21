//! Renders a hardcoded clock through the real pipeline (taffy -> DrawList ->
//! wgpu) offscreen and writes a PNG composited over a backdrop, so layout,
//! text, gradients, shadows and alpha can be inspected without a window.
//!
//!   cargo run --release --example render_test -- out.png

use std::time::Instant;

use wayfinder::anim::Anim;
use wayfinder::color::Color;
use wayfinder::gfx::{Gpu, Power};
use wayfinder::text::TextEngine;
use wayfinder::ui::*;

fn c(h: &str) -> Color {
    Color::parse(h).unwrap()
}

fn clock(angle_h: f32, angle_m: f32, angle_s: f32) -> Node {
    let face = Node::new("face")
        .abs_fill()
        .radius(999.0)
        .border(1.0, c("#ffffff40"))
        .fill(c("#ffffff14"));
    let mut ticks = Node::new("ticks").abs_fill();
    ticks.kind = Kind::Ticks(TicksSpec {
        count: 60,
        major_every: 5,
        len: 5.0,
        major_len: 10.0,
        width: 1.2,
        major_width: 2.4,
        color: c("#ffffff90"),
        major_color: c("#ffffffee"),
        inset: 8.0,
    });
    let hand = |key: &str, angle, length, width, color: &str| {
        let mut n = Node::new(key).abs_fill();
        n.kind = Kind::Hand(HandSpec { angle, length, tail: 0.12, width, color: c(color) });
        n
    };
    Node::new("clock")
        .wh(240.0, 240.0)
        .fill(c("#1c2333e6"))
        .radius(28.0)
        .border(1.0, c("#ffffff30"))
        .shadow(18.0, 6.0, c("#00000080"))
        .pad(14.0)
        .child(face)
        .child(ticks)
        .child(hand("h", angle_h, 0.5, 5.0, "#f0f4ff"))
        .child(hand("m", angle_m, 0.78, 3.5, "#f0f4ff"))
        .child(hand("s", angle_s, 0.86, 1.6, "#ff6a5a"))
}

fn digital() -> Node {
    let mut big = Node::text("t", "10:08", 56.0, c("#ffffff"));
    big = big.with_text(|t| t.weight = 300);
    let sub = Node::text("d", "Sunday, 21 September", 15.0, c("#c7d2f0")).with_text(|t| t.weight = 500);
    Node::new("digital")
        .wh(320.0, 150.0)
        .col()
        .center()
        .gap(2.0)
        .look_gradient(c("#3b4a7ae6"), c("#1d2540e6"))
        .radius(24.0)
        .border(1.0, c("#ffffff30"))
        .shadow(16.0, 5.0, c("#00000070"))
        .child(big)
        .child(sub)
}

trait LookExt {
    fn look_gradient(self, a: Color, b: Color) -> Self;
}
impl LookExt for Node {
    fn look_gradient(mut self, a: Color, b: Color) -> Self {
        self.look.fill = a;
        self.look.fill2 = Some(b);
        self
    }
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "render_test.png".into());
    let scale = 1.5f32;
    let (lw, lh) = (620.0f32, 290.0f32);
    let (pw, ph) = ((lw * scale) as u32, (lh * scale) as u32);

    let mut gpu = Gpu::new_headless(Power::parse(&std::env::var("WAYFINDER_GPU").unwrap_or_else(|_| "software".into()))).expect("gpu");
    println!("gpu: {}", gpu.info);
    let mut text = TextEngine::new();
    let mut anim = Anim::default();

    let root = Node::new("root")
        .wh(lw, lh)
        .row()
        .align(taffy::AlignItems::CENTER)
        .justify(taffy::JustifyContent::SPACE_EVENLY)
        .child(clock(300.0, 60.5, 132.0))
        .child(digital());
    let mut env = Env { text: &mut text, anim: &mut anim, hover: None, now: Instant::now(), scale };
    let frame = layout(&root, (lw, lh), &mut env);
    println!(
        "content {:?}, shapes {}, texts {}, hits {}",
        frame.content,
        frame.list.layers[0].shapes.len(),
        frame.list.layers[0].texts.len(),
        frame.hits.len()
    );

    let px = gpu.render_offscreen(pw, ph, &frame.list, &mut text).expect("render");
    // composite over a gradient "wallpaper" so the alpha is visible
    let mut img = image::RgbaImage::new(pw, ph);
    for y in 0..ph {
        for x in 0..pw {
            let i = ((y * pw + x) * 4) as usize;
            let t = x as f32 / pw as f32;
            let bg = [40.0 + 120.0 * t, 90.0 + 40.0 * (1.0 - t), 170.0 - 60.0 * t];
            let a = px[i + 3] as f32 / 255.0;
            let rgb = [0, 1, 2].map(|k| (px[i + k] as f32 + bg[k] * (1.0 - a)).clamp(0.0, 255.0) as u8);
            img.put_pixel(x, y, image::Rgba([rgb[0], rgb[1], rgb[2], 255]));
        }
    }
    img.save(&out).expect("save");
    println!("wrote {out}");
}
