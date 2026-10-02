//! Renders the settings window offscreen (software adapter by default) so the
//! pages, popups, the first-run setup and animations' settled state can be
//! inspected as PNGs.
//!
//!   cargo run --release --example render_settings -- <out_dir>

use std::path::Path;
use std::time::{Duration, Instant};

use wayfinder::anim::Anim;
use wayfinder::content::Contents;
use wayfinder::data::Shortcut;
use wayfinder::gfx::{Gpu, Power};
use wayfinder::icons::IconService;
use wayfinder::plugins::{CodeRow, PluginRow};
use wayfinder::settings::{Ctx, Level, LogLine, UiState};
use wayfinder::text::TextEngine;
use wayfinder::theme::Library;
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
    let reg = Registry::load(Path::new("nope"));
    let families = text.family_names();

    let win = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let items: Vec<Shortcut> = [("Notepad", "notepad.exe"), ("Calculator", "System32\\calc.exe"), ("Explorer", "explorer.exe"), ("Terminal", "System32\\cmd.exe")]
        .into_iter()
        .map(|(n, t)| Shortcut { name: n.into(), target: format!("{win}\\{t}"), icon: String::new() })
        .collect();
    let mut ws = Workspace::default();
    ws.onboarded = true;
    ws.theme.palette = std::env::var("WAYFINDER_PALETTE").unwrap_or_else(|_| "Aurora".into());
    let mon = MonitorRef { name: "\\\\.\\DISPLAY1".into(), width: 1920, height: 1080 };
    for (id, w, sz) in [("clock-1", "clock", (260.0, 260.0)), ("digital_clock-1", "digital_clock", (340.0, 172.0)), ("system_monitor-1", "system_monitor", (340.0, 190.0)), ("drawer-1", "drawer", (260.0, 220.0)), ("icon_list-1", "icon_list", (280.0, 360.0)), ("icon_folder-1", "icon_folder", (132.0, 152.0)), ("media_controller-1", "media_controller", (340.0, 160.0)), ("photo_frame-1", "photo_frame", (320.0, 240.0)), ("agent_status-1", "agent_status", (340.0, 160.0))] {
        let mut c = InstanceCfg { id: id.into(), widget: w.into(), monitor: mon.clone(), w: sz.0, h: sz.1, ..Default::default() };
        if w.starts_with("icon_") {
            c.set_items(&items);
        }
        ws.instances.push(c);
    }
    ws.style.insert("radius-lg".into(), 15.into());
    // the large monitor with a few modules put away, as the tray shows them
    let hide = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    ws.instances[2].layout.insert("large".into(), [("gauges".to_string(), hide(&["gauge:cpu", "gauge:ram", "gauge:disk"])), ("graphs".to_string(), hide(&["graph:cpu"])), ("footer".to_string(), hide(&["footer"]))].into());
    let extra = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let plugins = vec![
        PluginRow {
            id: "wayfinder-extra".into(),
            name: "Wayfinder Extra".into(),
            version: "0.2.0".into(),
            author: "wayfinder-extra".into(),
            description: "Agent status for Claude Code, Copilot CLI and Antigravity CLI, a photo gallery, a GIF player, a media controller and an audio visualizer, plus three palettes.".into(),
            summary: "5 widgets · 3 palettes".into(),
            enabled: true,
            contents: Contents { widgets: extra(&["agents", "gallery", "gif", "media", "audio"]), palettes: extra(&["Neon Noir", "Paper", "Sunset"]), ..Default::default() },
            code: vec![
                CodeRow { source: "agents".into(), reads: extra(&["~/.claude", "~/.copilot", "~/.gemini"]), runs: true, status: "Starting".into(), ..Default::default() },
                CodeRow { source: "gallery".into(), reads: extra(&["folders you pick for its widgets"]), runs: true, status: "Running".into(), ..Default::default() },
            ],
            ..Default::default()
        },
        PluginRow { id: "neon-icons".into(), name: "Neon Icons".into(), version: "0.3".into(), author: "Lin".into(), summary: "1 icon pack".into(), enabled: false, contents: Contents { icon_packs: extra(&["Neon"]), ..Default::default() }, ..Default::default() },
    ];
    let line = |time: &str, level, source: &str, text: &str| LogLine { time: time.into(), level, source: source.into(), text: text.into() };
    let log = vec![
        line("01:29:58", Level::Info, "core", "ready: 6 widgets, theme Aurora / System / Fluent"),
        line("01:29:58", Level::Warning, "gpu", "Using the software adapter (Microsoft Basic Render Driver, DX12, CPU). Integrated GPU recommended."),
        line("01:29:58", Level::Info, "gpu", "alpha PreMultiplied, present Mailbox"),
        line("01:29:58", Level::Info, "display", "\\\\.\\DISPLAY1: 1920×1080 @ 1.00×, work area 0,0 - 1920,1032"),
        line("01:29:59", Level::Info, "plugins", "wayfinder-extra v0.2.0 loaded: 5 widgets, 3 palettes"),
        line("01:29:59", Level::Info, "plugins", "agents: starting (no network; reads ~/.claude, ~/.copilot, ~/.gemini)"),
        line("01:29:59", Level::Error, "plugins", "could not install broken.wfplugin: no plugin.toml at the top of the plugin"),
        line("01:30:02", Level::Warning, "core", r"hotkey Ctrl+Shift+E unavailable (HotKey already registered: HotKey { mods: Modifiers(CONTROL | SHIFT), key: KeyE, id: 34078743 }); use the tray menu for Edit Mode, or C:\Users\someone\AppData\Local\Wayfinder\current\wayfinder.exe"),
    ];
    let sources = wayfinder::data::DataSources::builtin();
    let names = sources.names();
    let mut fresh = ws.clone();
    fresh.onboarded = false;
    // the Workspaces page: one tied to a desktop and the dock's monitors, one to a desktop alone
    let desktops = [
        wayfinder::platform::vdesk::Desktop { id: "{0B6E3A2C-1F4D-4E0A-9C1B-2D3E4F5A6B7C}".into(), name: "Desktop 1".into() },
        wayfinder::platform::vdesk::Desktop { id: "{7C1D2E3F-4A5B-4C6D-8E9F-0A1B2C3D4E5F}".into(), name: "Games".into() },
    ];
    let dock = vec![MonitorRef { name: "\\\\.\\DISPLAY2".into(), width: 2560, height: 1440 }, mon.clone()];
    let mut spaces = ws.clone();
    let work = spaces.add_workspace("Work", true);
    spaces.rules_mut(&work).unwrap().monitors = dock.clone();
    spaces.rules_mut(&work).unwrap().desktops = vec![desktops[0].id.clone()];
    let games = spaces.add_workspace("Games", false);
    spaces.rules_mut(&games).unwrap().desktops = vec![desktops[1].id.clone()];

    let (big, small) = ((1440.0, 900.0), (960.0, 600.0));
    let states: Vec<(&str, Vec<&str>, (f32, f32))> = vec![
        ("setup_1", vec![], big),
        ("setup_2", vec!["ob:next"], big),
        ("setup_3", vec!["ob:next", "ob:next"], big),
        ("setup_4", vec!["ob:next", "ob:next", "ob:next"], big),
        ("widgets_gallery", vec!["gallery:open"], big),
        ("widgets_gallery_narrow", vec!["gallery:open", "cat:Launchers"], small),
        ("widgets_gallery_photos", vec!["gallery:open", "cat:Photos"], big),
        ("widgets_media", vec!["sel:media_controller-1"], big),
        ("widgets_frame", vec!["sel:photo_frame-1"], big),
        ("widgets_folder", vec!["sel:icon_folder-1"], big),
        ("widgets_monitor", vec!["sel:system_monitor-1"], big),
        ("widgets_monitor_large", vec!["sel:system_monitor-1", "tier:large"], big),
        ("widgets_monitor_narrow", vec!["sel:system_monitor-1"], small),
        ("widgets_clock_dropdown", vec!["sel:clock-1", "sec:advanced", "dd:z:clock-1"], big),
        ("appearance", vec!["nav:appearance"], big),
        ("appearance_picker", vec!["nav:appearance", "cp:sy:*:accent"], big),
        ("appearance_narrow", vec!["nav:appearance"], small),
        ("general", vec!["nav:general"], big),
        ("general_narrow", vec!["nav:general"], small),
        ("plugins", vec!["nav:plugins"], big),
        ("plugins_empty", vec!["nav:plugins"], big),
        ("log", vec!["nav:log"], big),
        ("log_narrow", vec!["nav:log"], small),
        ("workspaces", vec!["nav:workspaces"], big),
        ("workspaces_other", vec!["nav:workspaces", "wssel:2"], big),
    ];
    for (name, acts, size) in states {
        let ws = if name.starts_with("setup") { &fresh } else if name.starts_with("workspaces") { &spaces } else { &ws };
        let (desks, desk, setup): (&[wayfinder::platform::vdesk::Desktop], Option<&str>, &[MonitorRef]) = if name.starts_with("workspaces") { (&desktops, Some(desktops[1].id.as_str()), &dock) } else { (&[], None, std::slice::from_ref(&mon)) };
        let plugins: &[PluginRow] = if name == "plugins_empty" { &[] } else { &plugins };
        let theme = ws.global_theme(&lib);
        let ctx = Ctx { ws, reg: &reg, lib: &lib, theme: &theme, log: &log, gpu_info: "Microsoft Basic Render Driver / Dx12 / Cpu / alpha PreMultiplied / present Mailbox", fonts: &families, edit: false, hidden: &[], plugins, plugin_note: "", sources: &names, plugin_files: &wayfinder::platform::win32::FileOwner::Me, data: &sources, desktops: desks, desktop: desk, setup, update_releases: &[], update_note: "" };
        let mut ui = UiState::default();
        for a in acts {
            let _ = ui.act(a, &ctx, None);
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
            ui.record_preview(&frame);
            if pass == 1 {
                png = Some(gpu.render_offscreen(size.0 as u32, size.1 as u32, &frame.list, &mut text).expect("render"));
            }
        }
        let px = png.unwrap();
        let (w, h) = (size.0 as u32, size.1 as u32);
        let img = image::RgbaImage::from_fn(w, h, |x, y| {
            let i = ((y * w + x) * 4) as usize;
            let a = px[i + 3] as f32 / 255.0;
            let rgb = [0, 1, 2].map(|k| (px[i + k] as f32 + 128.0 * (1.0 - a)).clamp(0.0, 255.0) as u8);
            image::Rgba([rgb[0], rgb[1], rgb[2], 255])
        });
        let path = format!("{out}/settings_{name}.png");
        img.save(&path).expect("save");
        println!("wrote {path}");
    }
}
