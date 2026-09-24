//! Renders the settings window offscreen (software adapter by default) so the
//! pages, popups and animations' settled state can be inspected as PNGs.
//!
//!   cargo run --release --example render_settings -- <out_dir>

use std::path::Path;
use std::time::{Duration, Instant};

use wayfinder::anim::Anim;
use wayfinder::data::Shortcut;
use wayfinder::gfx::{Gpu, Power};
use wayfinder::icons::IconService;
use wayfinder::plugins::PluginRow;
use wayfinder::settings::{Ctx, UiState};
use wayfinder::text::TextEngine;
use wayfinder::theme::{Library, Selection, Theme};
use wayfinder::ui::{self, Env};
use wayfinder::widgets::Registry;
use wayfinder::workspace::{InstanceCfg, MonitorRef, Workspace};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let power = Power::parse(&std::env::var("WAYFINDER_GPU").unwrap_or_else(|_| "software".into()));
    let mut gpu = Gpu::new_headless(power).expect("gpu");
    println!("gpu: {}", gpu.info);
    let mut text = TextEngine::new();
    let mut icons = IconService::default();
    let lib = Library::load(Path::new("nope"));
    let theme = Theme::compose(&lib, &Selection::default(), &[]);
    let reg = Registry::load(Path::new("nope"));
    let families = text.family_names();

    let win = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let items: Vec<Shortcut> = [("Notepad", "notepad.exe"), ("Calculator", "System32\\calc.exe"), ("Explorer", "explorer.exe"), ("Terminal", "System32\\cmd.exe")]
        .into_iter()
        .map(|(n, t)| Shortcut { name: n.into(), target: format!("{win}\\{t}"), icon: String::new() })
        .collect();
    let mut ws = Workspace::default();
    let mon = MonitorRef { name: "\\\\.\\DISPLAY1".into(), width: 1920, height: 1080 };
    for (id, w, sz) in [("clock-1", "clock", (260.0, 260.0)), ("digital_clock-1", "digital_clock", (340.0, 172.0)), ("icon_list-1", "icon_list", (280.0, 360.0)), ("icon_folder-1", "icon_folder", (132.0, 152.0))] {
        let mut c = InstanceCfg { id: id.into(), widget: w.into(), monitor: mon.clone(), w: sz.0, h: sz.1, ..Default::default() };
        if w.starts_with("icon_") {
            c.set_items(&items);
        }
        ws.instances.push(c);
    }
    ws.style.insert("radius-lg".into(), 26.into());
    let plugins = vec![
        PluginRow {
            id: "sunset".into(),
            name: "Sunset".into(),
            version: "1.2.0".into(),
            author: "Ada".into(),
            description: "Warm evening colours, a serif font set and a weather card.".into(),
            summary: "1 widget · 2 palettes · 1 font set".into(),
            enabled: true,
            notes: vec!["Restyles Analog Clock".into()],
            ..Default::default()
        },
        PluginRow { id: "neon-icons".into(), name: "Neon Icons".into(), version: "0.3".into(), author: "Lin".into(), summary: "1 icon pack".into(), enabled: false, ..Default::default() },
    ];
    let log: Vec<String> = ["monitor \\\\.\\DISPLAY1: 1920x1080 @ 1.00x", "gpu: AMD Radeon(TM) Graphics / Dx12 / IntegratedGpu", "ready: 4 instance(s), theme Midnight / System / Fluent", "edit mode on"].iter().map(|s| s.to_string()).collect();
    let ctx = Ctx { ws: &ws, reg: &reg, lib: &lib, theme: &theme, log: &log, gpu_info: "AMD Radeon(TM) Graphics / Dx12 / IntegratedGpu / alpha PreMultiplied / present Mailbox", fonts: &families, edit: false, hidden: &[], plugins: &plugins, plugin_note: "", sources: &wayfinder::data::DataSources::builtin().names(), plugin_files: &wayfinder::platform::win32::FileOwner::Me };

    let size = (960.0f32, 680.0f32);
    let states: Vec<(&str, Vec<&str>)> = vec![
        ("widgets_folder", vec!["nav:widgets", "sel:icon_folder-1"]),
        ("widgets_clock_dropdown", vec!["nav:widgets", "sel:clock-1", "dd:z:clock-1"]),
        ("appearance_picker", vec!["nav:appearance", "cp:sy:*:accent"]),
        ("widgets_clock_style", vec!["nav:widgets", "sel:clock-1"]),
        ("general", vec!["nav:general"]),
        ("plugins", vec!["nav:plugins"]),
    ];
    for (name, acts) in states {
        let mut ui = UiState::default();
        for a in acts {
            let _ = ui.act(a, &ctx, None);
        }
        if name.ends_with("_style") {
            ui.scroll.insert("w/scroll-r".into(), 520.0);
        }
        let mut anim = Anim::default();
        let base = Instant::now();
        let mut png = None;
        // pass 0 seeds enter animations and popup anchors, pass 1 is the settled frame
        for pass in 0..2 {
            let now = base + Duration::from_millis(if pass == 0 { 0 } else { 2500 });
            let (root, images) = ui.build(&ctx, size);
            for id in &images {
                icons.ensure(&mut gpu, id);
            }
            let mut env = Env { text: &mut text, anim: &mut anim, hover: None, now, scale: 1.0 };
            let frame = ui::layout(&root, size, &mut env);
            ui.record_anchors(&frame);
            if pass == 1 {
                let px = gpu.render_offscreen(size.0 as u32, size.1 as u32, &frame.list, &mut text).expect("render");
                png = Some(px);
            }
        }
        let px = png.unwrap();
        let (w, h) = (size.0 as u32, size.1 as u32);
        let mut img = image::RgbaImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let (tx, ty) = (x as f32 / w as f32, y as f32 / h as f32);
                let bg = [28.0 + 60.0 * tx, 46.0 + 40.0 * (1.0 - ty), 92.0 + 70.0 * ty];
                let a = px[i + 3] as f32 / 255.0;
                let rgb = [0, 1, 2].map(|k| (px[i + k] as f32 + bg[k] * (1.0 - a)).clamp(0.0, 255.0) as u8);
                img.put_pixel(x, y, image::Rgba([rgb[0], rgb[1], rgb[2], 255]));
            }
        }
        let path = format!("{out}/settings_{name}.png");
        img.save(&path).expect("save");
        println!("wrote {path}");
    }
}
