//! Renders the shipped widgets through the real pipeline (TOML -> bindings ->
//! taffy -> wgpu) offscreen, composited over a backdrop, to a PNG contact sheet.
//!
//!   cargo run --release --example render_widgets -- out.png [palette]

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use wayfinder::anim::Anim;
use wayfinder::card::Card;
use wayfinder::data::{DataSources, Shortcut, Tm};
use wayfinder::gfx::{Gpu, Power};
use wayfinder::icons::IconService;
use wayfinder::text::TextEngine;
use wayfinder::theme::{Library, Selection, Theme};
use wayfinder::value::Value;
use wayfinder::widgets::{prepare, Registry, Services, View};
use wayfinder::workspace::InstanceCfg;

struct Case {
    widget: &'static str,
    size: (f32, f32),
    params: Vec<(&'static str, serde_json::Value)>,
    state: Vec<(&'static str, Value)>,
    /// Use the expand size when the widget asks for one.
    expanded: bool,
}

fn shortcuts() -> Vec<Shortcut> {
    let win = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    [
        ("Notepad", format!("{win}\\notepad.exe")),
        ("Calculator", format!("{win}\\System32\\calc.exe")),
        ("Paint", format!("{win}\\System32\\mspaint.exe")),
        ("Explorer", format!("{win}\\explorer.exe")),
        ("Terminal", format!("{win}\\System32\\cmd.exe")),
        ("Registry", format!("{win}\\regedit.exe")),
        ("Snipping Tool", format!("{win}\\System32\\SnippingTool.exe")),
    ]
    .into_iter()
    .map(|(n, t)| Shortcut { name: n.into(), target: t, icon: String::new() })
    .collect()
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "widgets.png".into());
    let palette = std::env::args().nth(2).unwrap_or_else(|| "Midnight".into());
    let scale = 1.25f32;
    let items = shortcuts();

    let mut gpu = Gpu::new_headless(Power::parse(&std::env::var("WAYFINDER_GPU").unwrap_or_else(|_| "software".into()))).expect("gpu");
    let mut text = TextEngine::new();
    let mut icons = IconService::new("iconpacks".into());
    let lib = Library::load(Path::new("nope"));
    let sel = Selection { palette: palette.clone(), ..Default::default() };
    let theme = Theme::compose(&lib, &sel, &BTreeMap::new());
    let reg = Registry::load(Path::new("nope"));
    let card = Card::new(&theme, false, true);
    let sources = DataSources::builtin();
    let tm = Tm { year: 2026, month: 9, day: 21, dow: 1, hour: 15, minute: 42, second: 18, ms: 400 };

    let cases = vec![
        Case { widget: "clock", size: (260.0, 260.0), params: vec![], state: vec![], expanded: false },
        Case { widget: "clock", size: (150.0, 150.0), params: vec![("smooth_seconds", true.into())], state: vec![], expanded: false },
        Case { widget: "digital_clock", size: (340.0, 172.0), params: vec![], state: vec![], expanded: false },
        Case { widget: "digital_clock", size: (250.0, 100.0), params: vec![("hour24", false.into()), ("show_seconds", true.into())], state: vec![], expanded: false },
        Case { widget: "icon_list", size: (260.0, 360.0), params: vec![("title", "Quick launch".into())], state: vec![], expanded: false },
        Case { widget: "icon_list", size: (200.0, 180.0), params: vec![("icon_size", 24.into()), ("title", "".into())], state: vec![], expanded: false },
        Case { widget: "icon_folder", size: (132.0, 152.0), params: vec![("title", "Tools".into())], state: vec![], expanded: false },
        Case { widget: "icon_folder", size: (132.0, 152.0), params: vec![("title", "Tools".into())], state: vec![("expanded", true.into())], expanded: true },
    ];

    let mut tiles = Vec::new();
    let mut anim = Anim::default();
    for c in &cases {
        let mut cfg = InstanceCfg { id: format!("{}-{}", c.widget, tiles.len()), widget: c.widget.into(), w: c.size.0, h: c.size.1, ..Default::default() };
        for (k, v) in &c.params {
            cfg.params.insert(k.to_string(), v.clone());
        }
        cfg.set_items(&items);
        let state: BTreeMap<String, Value> = c.state.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        let def = reg.get(c.widget).expect("widget");
        let mut size = c.size;
        // Enter animations start at alpha 0, so render once at t0 to seed them
        // and capture the frame after they have settled.
        let base = Instant::now();
        {
            let v = View { cfg: &cfg, state: &state, size, theme: &theme, pack: "Default", tm, hover: None, scale, now: base, card };
            let mut sv = Services { gpu: &mut gpu, icons: &mut icons, text: &mut text, anim: &mut anim, sources: &sources };
            let _ = prepare(def, &v, &mut sv);
        }
        // two passes: the first learns the expand size, the second lays out at it
        for pass in 0..2 {
            let now = base + Duration::from_millis(2000);
            let v = View { cfg: &cfg, state: &state, size, theme: &theme, pack: "Default", tm, hover: None, scale, now, card };
            let mut sv = Services { gpu: &mut gpu, icons: &mut icons, text: &mut text, anim: &mut anim, sources: &sources };
            let p = prepare(def, &v, &mut sv);
            if let Some(e) = &p.error {
                println!("{}: ERROR {e}", c.widget);
            }
            for w in &p.warnings {
                println!("{}: warning {w}", c.widget);
            }
            if pass == 0 && c.expanded {
                if let Some(x) = p.expand.filter(|x| x.active) {
                    size = (x.width.unwrap_or(size.0), x.height.unwrap_or(size.1));
                    continue;
                }
            }
            let (pw, ph) = ((size.0 * scale) as u32, (size.1 * scale) as u32);
            let px = gpu.render_offscreen(pw, ph, &p.frame.list, &mut text).expect("render");
            println!("{:<14} {:>4.0}x{:<4.0} deps={:?} shapes={} texts={}", c.widget, size.0, size.1, p.deps, p.frame.list.layers[0].shapes.len(), p.frame.list.layers[0].texts.len());
            tiles.push((pw, ph, px));
            break;
        }
    }

    // contact sheet: two rows over a soft "wallpaper"
    let pad = 24u32;
    let row_w = |r: &[(u32, u32, Vec<u8>)]| r.iter().map(|t| t.0 + pad).sum::<u32>() + pad;
    let (top, bot) = tiles.split_at(4);
    let (sw, rh1, rh2) = (row_w(top).max(row_w(bot)), top.iter().map(|t| t.1).max().unwrap() + pad, bot.iter().map(|t| t.1).max().unwrap() + pad);
    let sh = rh1 + rh2 + pad;
    let mut sheet = image::RgbaImage::new(sw, sh);
    for y in 0..sh {
        for x in 0..sw {
            let (tx, ty) = (x as f32 / sw as f32, y as f32 / sh as f32);
            let bg = if palette == "Daylight" { [200.0 + 30.0 * tx, 215.0 + 20.0 * ty, 235.0] } else { [30.0 + 70.0 * tx, 60.0 + 50.0 * (1.0 - ty), 120.0 + 60.0 * ty] };
            sheet.put_pixel(x, y, image::Rgba([bg[0] as u8, bg[1] as u8, bg[2] as u8, 255]));
        }
    }
    let mut blit = |row: &[(u32, u32, Vec<u8>)], y0: u32| {
        let mut x0 = pad;
        for (w, h, px) in row {
            for y in 0..*h {
                for x in 0..*w {
                    let i = ((y * w + x) * 4) as usize;
                    let a = px[i + 3] as f32 / 255.0;
                    let dst = sheet.get_pixel_mut(x0 + x, y0 + y);
                    for k in 0..3 {
                        dst.0[k] = (px[i + k] as f32 + dst.0[k] as f32 * (1.0 - a)).clamp(0.0, 255.0) as u8;
                    }
                }
            }
            x0 += w + pad;
        }
    };
    blit(top, pad / 2);
    blit(bot, rh1 + pad / 2);
    sheet.save(&out).expect("save");
    println!("wrote {out} ({sw}x{sh})");
}
