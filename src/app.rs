//! The running application: one OS window per Instance, event-driven
//! scheduling (a window redraws only on a changed binding, an in-flight
//! animation or input; decision 16), Edit Mode, tray and hotkey.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::{CursorIcon, Window, WindowAttributes, WindowId};

use crate::anim::{Anim, Ease};
use crate::data::{self, Shortcut};
use crate::draw::DrawList;
use crate::edit::{self, Handle, Rect, Snap};
use crate::format::{self, ExpandInfo};
use crate::gfx::{Gpu, Power, RenderError, Target};
use crate::icons::IconService;
use crate::platform::win32::{self, ZMode};
use crate::settings::{self, Cmd, SettingsWin};
use crate::text::TextEngine;
use crate::theme::{Library, Theme};
use crate::ui::{self, Env, Frame};
use crate::value::Value;
use crate::widgets::{self, Def, Registry, Services, View};
use crate::workspace::{self, InstanceCfg, MonitorInfo, Workspace};

#[derive(Debug)]
pub enum UserEvent {
    Menu(String),
    Hotkey,
    TrayClick,
    /// Something on disk changed (debounced).
    Files,
    /// Foreground window or minimise state changed: re-check Show Desktop.
    Shell,
    Display,
}

pub struct Options {
    pub dir: PathBuf,
    /// Drive the app with synthetic input and report pass/fail (see `selftest_tick`).
    pub selftest: bool,
    /// `--gpu high|low|software`: overrides the workspace setting for this run.
    pub gpu: Option<String>,
    /// Exit by itself after this many seconds (automated runs).
    pub exit_after: Option<f32>,
}

struct Drag {
    handle: Handle,
    cursor0: (i32, i32),
    rect0: Rect,
    before: Rect,
}

struct SizeTween {
    from: Rect,
    to: Rect,
    start: Instant,
}

/// Runtime state of one Instance, index-aligned with `Workspace::instances`.
struct InstWin {
    window: Option<Arc<Window>>,
    target: Option<Target>,
    state: BTreeMap<String, Value>,
    anim: Anim,
    ov_anim: Anim,
    hover: Option<String>,
    frame: Option<Frame>,
    deps: BTreeSet<String>,
    next_tick: Option<Instant>,
    redraw: bool,
    requested: bool,
    animating: bool,
    last_render: Instant,
    items: Vec<Shortcut>,
    drag: Option<Drag>,
    grab: Option<Handle>,
    mouse: (f32, f32),
    tween: Option<SizeTween>,
    want: Option<Rect>,
    raised: bool,
    error: Option<String>,
    widget_error: Option<String>,
}

impl InstWin {
    fn new() -> Self {
        Self {
            window: None,
            target: None,
            state: BTreeMap::new(),
            anim: Anim::default(),
            ov_anim: Anim::default(),
            hover: None,
            frame: None,
            deps: BTreeSet::new(),
            next_tick: None,
            redraw: true,
            requested: false,
            animating: false,
            last_render: Instant::now(),
            items: Vec::new(),
            drag: None,
            grab: None,
            mouse: (-1.0, -1.0),
            tween: None,
            want: None,
            raised: false,
            error: None,
            widget_error: None,
        }
    }
}

struct SelfTest {
    step: u32,
    at: Instant,
    checks: Vec<(String, bool)>,
    rect: Option<Rect>,
    collapsed: Option<Rect>,
    configures0: u32,
    fake: Option<win32::Sentinel>,
}

struct UndoEntry {
    id: String,
    before: Rect,
}

pub struct App {
    proxy: EventLoopProxy<UserEvent>,
    opts: Options,
    ws: Workspace,
    lib: Library,
    theme: Theme,
    reg: Registry,
    gpu: Option<Gpu>,
    text: TextEngine,
    icons: IconService,
    wins: Vec<InstWin>,
    settings: Option<SettingsWin>,
    edit: bool,
    undo: Vec<UndoEntry>,
    mods: ModifiersState,
    monitors: Vec<MonitorInfo>,
    tray: Option<TrayIcon>,
    _hotkeys: Option<GlobalHotKeyManager>,
    watchers: Vec<RecommendedWatcher>,
    watched_folders: Vec<(String, String)>,
    log: Vec<String>,
    save_at: Option<Instant>,
    reload_at: Option<Instant>,
    /// Times at which to re-check Show Desktop: a retry ladder after each shell event.
    shell_due: Vec<Instant>,
    sentinel: Option<win32::Sentinel>,
    /// Tests substitute a fake icon host so Show Desktop can be exercised without Explorer.
    test_host: Option<windows::Win32::Foundation::HWND>,
    display_at: Option<Instant>,
    desktop_shown: bool,
    started: Instant,
    booted: bool,
    start_edit: bool,
    selftest: Option<SelfTest>,
    /// Set by `render` when the surface could not be recovered; handled in `about_to_wait`.
    gpu_lost: Option<String>,
    gpu_recoveries: Vec<Instant>,
    /// After repeated GPU losses: run on the software renderer for the rest of the session.
    degraded: bool,
    families: Vec<String>,
}

fn tray_icon_image() -> tray_icon::Icon {
    let n = 32u32;
    let mut px = vec![0u8; (n * n * 4) as usize];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 + 0.5 - 16.0, y as f32 + 0.5 - 16.0);
            let r = (fx * fx + fy * fy).sqrt();
            let disc = (0.5 - (r - 14.5)).clamp(0.0, 1.0);
            let ring = (1.0 - ((r - 12.0).abs() - 0.6).max(0.0)).clamp(0.0, 1.0);
            // a needle pointing up-right: two thin triangles along the diagonal
            let k = std::f32::consts::FRAC_1_SQRT_2;
            let (u, v) = ((fx - fy) * k, (fx + fy) * k); // rotate 45 deg
            let width = (8.0 - v.abs()).max(0.0) * 0.0 + (0.6 * (9.0 - u.abs())).max(0.0);
            let needle = if u.abs() < 9.0 && v.abs() < width { 1.0 } else { 0.0 };
            let i = ((y * n + x) * 4) as usize;
            let base = [58.0, 110.0, 240.0];
            let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
            let t = (ring * 0.35 + needle).min(1.0);
            let a = (disc * 255.0) as u8;
            px[i..i + 4].copy_from_slice(&[mix(base[0], 255.0, t) as u8, mix(base[1], 255.0, t) as u8, mix(base[2], 255.0, t) as u8, a]);
        }
    }
    tray_icon::Icon::from_rgba(px, n, n).expect("tray icon")
}

impl App {
    pub fn new(proxy: EventLoopProxy<UserEvent>, opts: Options) -> App {
        let dir = opts.dir.clone();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::remove_file(dir.join("wayfinder.log")); // one log per run
        let (ws, ws_err) = Workspace::load(&dir);
        let lib = Library::load(&dir);
        let overrides = ws.overrides.iter().map(|(k, v)| (k.clone(), Value::Str(v.clone()))).collect();
        let theme = Theme::compose(&lib, &ws.theme, &overrides);
        let reg = Registry::load(&dir.join("widgets"));
        let mut text = TextEngine::new();
        let fonts = text.load_font_dir(&dir.join("fonts"));
        let mut app = App {
            proxy,
            icons: IconService::new(dir.join("iconpacks")),
            opts,
            ws,
            lib,
            theme,
            reg,
            gpu: None,
            text,
            wins: Vec::new(),
            settings: None,
            edit: false,
            undo: Vec::new(),
            mods: ModifiersState::empty(),
            monitors: Vec::new(),
            tray: None,
            _hotkeys: None,
            watchers: Vec::new(),
            watched_folders: Vec::new(),
            log: Vec::new(),
            save_at: None,
            reload_at: None,
            shell_due: Vec::new(),
            sentinel: None,
            test_host: None,
            display_at: None,
            desktop_shown: false,
            started: Instant::now(),
            booted: false,
            start_edit: false,
            selftest: None,
            gpu_lost: None,
            gpu_recoveries: Vec::new(),
            degraded: false,
            families: Vec::new(),
        };
        app.families = app.text.family_names();
        app.selftest = app.opts.selftest.then(|| SelfTest { step: 0, at: Instant::now() + Duration::from_millis(2200), checks: Vec::new(), rect: None, collapsed: None, configures0: 0, fake: None });
        if let Some(e) = ws_err {
            app.log(e);
        }
        if fonts > 0 {
            app.log(format!("loaded {fonts} user font faces"));
        }
        for e in app.lib.errors.clone().into_iter().chain(app.reg.errors()) {
            app.log(e);
        }
        app
    }

    fn power(&self) -> Power {
        if self.degraded {
            return Power::Software;
        }
        Power::parse(self.opts.gpu.as_deref().unwrap_or(&self.ws.gpu))
    }

    pub fn request_edit_on_start(&mut self) {
        self.start_edit = true;
    }

    fn log(&mut self, s: impl Into<String>) {
        let s = s.into();
        eprintln!("wayfinder: {s}");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(self.opts.dir.join("wayfinder.log")) {
            use std::io::Write;
            let _ = writeln!(f, "[{:>7.2}s] {s}", self.started.elapsed().as_secs_f32());
        }
        self.log.push(s);
        if self.log.len() > 300 {
            self.log.drain(..100);
        }
    }

    fn rebuild_theme(&mut self) {
        let ov = self.ws.overrides.iter().map(|(k, v)| (k.clone(), Value::Str(v.clone()))).collect();
        self.theme = Theme::compose(&self.lib, &self.ws.theme, &ov);
        for f in self.lib.fonts(&self.ws.theme.fonts).files.clone() {
            self.text.load_font_file(&f);
        }
        self.redraw_all();
    }

    fn redraw_all(&mut self) {
        for iw in &mut self.wins {
            iw.redraw = true;
        }
    }

    fn mark_save(&mut self) {
        self.save_at = Some(Instant::now() + Duration::from_millis(400));
    }

    fn gutter(&self) -> f32 {
        format::gutter(&self.theme)
    }

    // ---- windows -------------------------------------------------------------

    fn monitor_of(&self, cfg: &InstanceCfg) -> Option<&MonitorInfo> {
        self.monitors.iter().find(|m| m.name == cfg.monitor.name)
    }

    fn sync_windows(&mut self, el: &ActiveEventLoop) {
        while self.wins.len() < self.ws.instances.len() {
            self.wins.push(InstWin::new());
        }
        self.wins.truncate(self.ws.instances.len());
        for i in 0..self.ws.instances.len() {
            let pos = workspace::resolve(&self.ws.instances[i], &self.monitors);
            match (pos, self.wins[i].window.is_some()) {
                (Some(p), false) => {
                    if let Err(e) = self.create_window(el, i, p) {
                        let id = self.ws.instances[i].id.clone();
                        self.log(format!("could not create window for {id}: {e}"));
                    }
                }
                (None, true) => {
                    let id = self.ws.instances[i].id.clone();
                    self.log(format!("monitor for {id} is gone: parked (kept in place for when it returns)"));
                    let iw = &mut self.wins[i];
                    iw.target = None;
                    iw.window = None;
                    iw.frame = None;
                }
                _ => {}
            }
        }
    }

    fn create_window(&mut self, el: &ActiveEventLoop, i: usize, pos: (i32, i32)) -> Result<(), String> {
        let (cid, cw, ch, cz, click_through) = {
            let c = &self.ws.instances[i];
            (c.id.clone(), c.w as f64, c.h as f64, c.z.clone(), c.click_through)
        };
        let attrs = WindowAttributes::default()
            .with_title(format!("Wayfinder {cid}"))
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_visible(false)
            .with_active(false)
            .with_position(PhysicalPosition::new(pos.0, pos.1))
            .with_inner_size(LogicalSize::new(cw, ch))
            .with_skip_taskbar(true)
            // ADR-001: DirectComposition presents beneath the GDI redirection bitmap.
            .with_no_redirection_bitmap(true);
        let window = Arc::new(el.create_window(attrs).map_err(|e| e.to_string())?);
        let target = match self.gpu.as_mut() {
            Some(g) => g.target_for(&window)?,
            None => {
                let (g, t) = Gpu::new(&window, self.power())?;
                let info = g.info.clone();
                self.gpu = Some(g);
                self.log(format!("gpu: {info}"));
                t
            }
        };
        // re-assert the logical size in case the window landed on a different-DPI monitor
        let _ = window.request_inner_size(LogicalSize::new(cw, ch));
        if let Some(h) = win32::hwnd_of(&window) {
            win32::set_zmode(h, ZMode::parse(&cz).unwrap_or(ZMode::Desktop));
        }
        let _ = window.set_cursor_hittest(!click_through || self.edit);
        let iw = &mut self.wins[i];
        iw.window = Some(window.clone());
        iw.target = Some(target);
        iw.redraw = true;
        self.refresh_items(i);
        self.render(i);
        window.set_visible(true);
        // winit rebuilds WS_EX_* from its own flags on every state change, so our
        // styles go on last, and again after anything that touches them
        win32::set_no_activate(&window, !self.edit);
        self.guard_z(i);
        if self.desktop_shown {
            self.apply_show_desktop();
        }
        Ok(())
    }

    fn refresh_items(&mut self, i: usize) {
        let cfg = &self.ws.instances[i];
        let folder = cfg.folder();
        self.wins[i].items = if folder.is_empty() { cfg.items() } else { data::folder_items(&folder, 96) };
    }

    fn index_of(&self, id: WindowId) -> Option<usize> {
        self.wins.iter().position(|w| w.window.as_ref().is_some_and(|w| w.id() == id))
    }

    fn hwnds(&self) -> Vec<windows::Win32::Foundation::HWND> {
        self.wins.iter().filter_map(|w| w.window.as_ref()).filter_map(|w| win32::hwnd_of(w)).collect()
    }

    fn scale_of(&self, i: usize) -> f64 {
        let cfg = &self.ws.instances[i];
        self.monitor_of(cfg).map(|m| m.scale).or_else(|| self.wins[i].window.as_ref().map(|w| w.scale_factor())).unwrap_or(1.0)
    }

    // ---- rendering ---------------------------------------------------------------

    fn render(&mut self, i: usize) {
        let now = Instant::now();
        let gutter = self.gutter();
        let App { gpu, text, icons, theme, reg, ws, wins, edit, .. } = self;
        let (Some(gpu), Some(iw)) = (gpu.as_mut(), wins.get_mut(i)) else { return };
        let (Some(window), Some(target)) = (iw.window.clone(), iw.target.as_mut()) else { return };
        let cfg = &ws.instances[i];

        // advance an expand/collapse size tween before measuring the window
        if let Some(tw) = &iw.tween {
            let t = (now.saturating_duration_since(tw.start).as_secs_f32() / 0.20).clamp(0.0, 1.0);
            let k = Ease::Out.apply(t);
            let l = |a: i32, b: i32| a + ((b - a) as f32 * k).round() as i32;
            if let Some(h) = win32::hwnd_of(&window) {
                win32::set_rect(h, l(tw.from.x, tw.to.x), l(tw.from.y, tw.to.y), l(tw.from.w, tw.to.w).max(1), l(tw.from.h, tw.to.h).max(1));
            }
            if t >= 1.0 {
                iw.tween = None;
            }
        }
        let phys = window.inner_size();
        if target.view() != (phys.width.max(1), phys.height.max(1)) {
            gpu.fit(target, phys.width, phys.height);
        }
        let scale = window.scale_factor() as f32;
        let size = (phys.width as f32 / scale, phys.height as f32 / scale);
        let missing: Def = Err(format!("unknown widget `{}`", cfg.widget));
        let def = reg.get(&cfg.widget).unwrap_or(&missing);
        let tm = data::now_local();
        let pack = ws.theme.icon_pack.clone();
        let v = View { cfg, state: &iw.state, items: &iw.items, size, theme, pack: &pack, tm, hover: iw.hover.as_deref(), scale, now };
        let mut sv = Services { gpu, icons, text, anim: &mut iw.anim };
        let mut p = widgets::prepare(def, &v, &mut sv);

        if *edit {
            let label = format!("{}, {}   {}x{}", cfg.x as i32, cfg.y as i32, (size.0 - 2.0 * gutter) as i32, (size.1 - 2.0 * gutter) as i32);
            let ov = edit::overlay(&cfg.id, size, gutter, &label, theme, iw.drag.as_ref().map(|d| d.handle));
            let mut env = Env { text, anim: &mut iw.ov_anim, hover: None, now, scale };
            let of = ui::layout(&ov, size, &mut env);
            let [l0, _] = of.list.layers;
            p.frame.list.layers[1].shapes.extend(l0.shapes);
            p.frame.list.layers[1].images.extend(l0.images);
            p.frame.list.layers[1].texts.extend(l0.texts);
            p.frame.animating |= of.animating;
        }
        let mut lost = None;
        match gpu.render(target, &p.frame.list, text) {
            Ok(()) => iw.error = None,
            Err(RenderError::Skip(e)) => {
                if iw.error.as_deref() != Some(e.as_str()) {
                    eprintln!("wayfinder: render {}: {e}", cfg.id);
                    iw.error = Some(e);
                }
            }
            Err(RenderError::Lost(e)) => lost = Some(format!("{}: {e}", cfg.id)),
        }
        for w in &p.warnings {
            eprintln!("wayfinder: {}: {w}", cfg.id);
        }
        let continuous = data::is_continuous(&p.deps);
        iw.next_tick = if continuous { None } else { data::next_wake(&p.deps, &tm).map(|d| now + d) };
        iw.widget_error = p.error.clone();
        iw.animating = p.frame.animating || continuous || iw.tween.is_some();
        iw.deps = p.deps;
        iw.frame = Some(p.frame);
        iw.redraw = false;
        iw.requested = false;
        iw.last_render = now;
        let expand = p.expand;
        if lost.is_some() && self.gpu_lost.is_none() {
            self.gpu_lost = lost;
        }
        self.drive_expand(i, expand);
    }

    /// Grow or shrink the window when the widget's `[expand]` state changes
    /// (decision 23): grow away from the nearest screen edge, clamp to the
    /// work area, sit above sibling widgets while open.
    fn drive_expand(&mut self, i: usize, expand: Option<ExpandInfo>) {
        let Some(window) = self.wins[i].window.clone() else { return };
        let cfg = self.ws.instances[i].clone();
        let Some(mon) = self.monitor_of(&cfg).cloned() else { return };
        let active = expand.is_some_and(|e| e.active);
        let scale = mon.scale;
        let Some(pos) = workspace::resolve(&cfg, &self.monitors) else { return };
        let collapsed = Rect::new(pos.0, pos.1, (cfg.w as f64 * scale).round() as i32, (cfg.h as f64 * scale).round() as i32);
        let target = if active {
            let e = expand.unwrap();
            let (tw, th) = (e.width.unwrap_or(cfg.w), e.height.unwrap_or(cfg.h));
            let (w, h) = (((tw as f64 * scale).round() as i32).min(mon.work.2 as i32), ((th as f64 * scale).round() as i32).min(mon.work.3 as i32));
            let (wl, wt, wr, wb) = (mon.work.0, mon.work.1, mon.work.0 + mon.work.2 as i32, mon.work.1 + mon.work.3 as i32);
            let x = if collapsed.x + w > wr { (collapsed.right() - w).max(wl) } else { collapsed.x };
            let y = if collapsed.y + h > wb { (collapsed.bottom() - h).max(wt) } else { collapsed.y };
            Rect::new(x, y, w, h)
        } else {
            collapsed
        };
        if self.wins[i].want == Some(target) {
            return;
        }
        let first = self.wins[i].want.is_none();
        let hwnd = win32::hwnd_of(&window);
        let cur = window.outer_position().ok().map(|p| Rect::new(p.x, p.y, window.outer_size().width as i32, window.outer_size().height as i32)).unwrap_or(collapsed);
        self.wins[i].want = Some(target);
        if first && !active {
            return; // initial layout at the stored size; nothing to animate
        }
        self.wins[i].tween = Some(SizeTween { from: cur, to: target, start: Instant::now() });
        // one reconfigure up front; the animation itself then only moves the window
        if let (Some(g), Some(t)) = (self.gpu.as_ref(), self.wins[i].target.as_mut()) {
            g.fit(t, target.w.max(1) as u32, target.h.max(1) as u32);
        }
        self.wins[i].redraw = true;
        let siblings = self.hwnds();
        if let Some(h) = hwnd {
            let mode = ZMode::parse(&cfg.z).unwrap_or(ZMode::Desktop);
            if active && matches!(mode, ZMode::Desktop | ZMode::Bottom) && !self.wins[i].raised {
                win32::raise_above(h, &siblings);
                self.wins[i].raised = true;
            } else if !active && self.wins[i].raised {
                win32::set_zmode(h, mode);
                self.wins[i].raised = false;
            }
        }
    }

    // ---- Show Desktop (ADR-002, from Rainmeter's System.cpp) -------------------------------

    /// Re-place every widget for the current Show Desktop state:
    /// Desktop-mode widgets float just under the taskbar while the desktop is
    /// shown and sink back after; Bottom-mode widgets stay under the desktop
    /// (hidden by it, by definition); Normal and Topmost are not ours to move.
    fn apply_show_desktop(&mut self) {
        let host = self.test_host.or_else(win32::desktop_icon_host);
        for i in 0..self.wins.len() {
            let Some(h) = self.wins[i].window.as_ref().and_then(|w| win32::hwnd_of(w)) else { continue };
            match (ZMode::parse(&self.ws.instances[i].z).unwrap_or(ZMode::Desktop), self.desktop_shown) {
                (ZMode::Desktop, true) => {
                    if let Some(host) = host {
                        win32::float_over_desktop(h, host);
                    }
                }
                (ZMode::Desktop | ZMode::Bottom, false) => win32::sink_to_desktop(h),
                _ => {}
            }
        }
        // an open folder sits above its sibling widgets; sinking undid that
        let siblings = self.hwnds();
        for i in 0..self.wins.len() {
            if self.wins[i].raised {
                if let Some(h) = self.wins[i].window.as_ref().and_then(|w| win32::hwnd_of(w)) {
                    win32::raise_above(h, &siblings);
                }
            }
        }
    }

    fn check_show_desktop(&mut self) {
        let Some(shown) = self.sentinel.as_ref().and_then(win32::desktop_state) else { return };
        if shown != self.desktop_shown {
            self.desktop_shown = shown;
            self.log(format!("desktop {}", if shown { "shown: Desktop-layer widgets float above it" } else { "hidden: widgets back on the desktop layer" }));
            self.apply_show_desktop();
        }
        // Leaving Show Desktop can happen without a foreground event; keep an eye
        // on it only while it lasts (Rainmeter polls at 100 ms in this state)
        if self.desktop_shown && self.shell_due.is_empty() {
            self.shell_due.push(Instant::now() + Duration::from_millis(250));
        }
    }

    /// Foreign z-order changes are vetoed for widgets that must stay put.
    fn guard_z(&self, i: usize) {
        if let Some(w) = &self.wins[i].window {
            let mode = ZMode::parse(&self.ws.instances[i].z).unwrap_or(ZMode::Desktop);
            win32::set_z_guard(w, matches!(mode, ZMode::Desktop | ZMode::Bottom));
        }
    }

    // ---- Edit Mode -----------------------------------------------------------------

    fn set_edit(&mut self, on: bool) {
        if self.edit == on {
            return;
        }
        self.edit = on;
        self.undo.clear();
        for i in 0..self.wins.len() {
            let click_through = self.ws.instances[i].click_through;
            if let Some(w) = &self.wins[i].window {
                let _ = w.set_cursor_hittest(on || !click_through);
                win32::set_no_activate(w, !on);
                if on {
                    w.focus_window();
                }
            }
            self.wins[i].drag = None;
            self.wins[i].redraw = true;
        }
        if !on {
            self.mark_save();
        }
        self.log(if on { "edit mode on (drag to move, drag edges/corners to resize, Ctrl+Z undo, Esc or the hotkey to finish)" } else { "edit mode off" });
    }

    fn snap_for(&self, i: usize) -> Snap {
        let (mut xs, mut ys) = (Vec::new(), Vec::new());
        let scale = self.scale_of(i);
        let mut origin = (0, 0);
        if let Some(m) = self.monitor_of(&self.ws.instances[i]) {
            xs.extend([m.work.0, m.work.0 + m.work.2 as i32]);
            ys.extend([m.work.1, m.work.1 + m.work.3 as i32]);
            origin = (m.work.0, m.work.1);
        }
        let g = self.gutter() * scale as f32;
        for (j, other) in self.wins.iter().enumerate() {
            if j == i {
                continue;
            }
            if let Some(w) = &other.window {
                if let (Ok(p), s) = (w.outer_position(), w.outer_size()) {
                    // snap card edges (window minus gutter), not the transparent margin
                    xs.extend([p.x + g as i32, p.x + s.width as i32 - g as i32]);
                    ys.extend([p.y + g as i32, p.y + s.height as i32 - g as i32]);
                }
            }
        }
        Snap { xs, ys, threshold: (8.0 * scale) as i32, grid: (self.ws.grid * scale as f32) as i32, origin }
    }

    fn outer_rect(w: &Window) -> Option<Rect> {
        let p = w.outer_position().ok()?;
        let s = w.outer_size();
        Some(Rect::new(p.x, p.y, s.width as i32, s.height as i32))
    }

    fn set_window_rect(&mut self, i: usize, r: Rect) {
        if let Some(w) = &self.wins[i].window {
            if let Some(h) = win32::hwnd_of(w) {
                win32::set_rect(h, r.x, r.y, r.w.max(1), r.h.max(1));
            }
        }
        self.wins[i].redraw = true;
    }

    /// Store where a window really is (anchored to its monitor) into the Workspace.
    fn commit_rect(&mut self, i: usize) {
        let Some(w) = self.wins[i].window.clone() else { return };
        let Some(r) = Self::outer_rect(&w) else { return };
        if let Some((m, x, y)) = workspace::anchor((r.x, r.y), (r.w as u32, r.h as u32), &self.monitors) {
            let scale = self.monitors.iter().find(|mi| mi.name == m.name).map_or(1.0, |mi| mi.scale);
            let cfg = &mut self.ws.instances[i];
            cfg.monitor = m;
            cfg.x = x;
            cfg.y = y;
            cfg.w = (r.w as f64 / scale).round() as f32;
            cfg.h = (r.h as f64 / scale).round() as f32;
            self.wins[i].want = None;
            self.mark_save();
        }
    }

    fn min_size_phys(&self, i: usize) -> (i32, i32) {
        let cfg = &self.ws.instances[i];
        let s = self.scale_of(i);
        let g = 2.0 * self.gutter() as f64;
        let (mw, mh) = match self.reg.get(&cfg.widget) {
            Some(Ok(d)) => d.min_size,
            _ => (48.0, 48.0),
        };
        (((mw as f64 + g) * s) as i32, ((mh as f64 + g) * s) as i32)
    }

    // ---- input -----------------------------------------------------------------------

    fn on_cursor(&mut self, i: usize, pos: PhysicalPosition<f64>) {
        let scale = self.wins[i].window.as_ref().map_or(1.0, |w| w.scale_factor());
        let (x, y) = ((pos.x / scale) as f32, (pos.y / scale) as f32);
        self.wins[i].mouse = (x, y);
        if self.edit {
            if self.wins[i].drag.is_some() {
                self.drag_update(i, win32::cursor_pos());
                return;
            }
            let card = self.card_rect(i);
            let h = edit::hit_handle(x, y, card, 16.0);
            if self.wins[i].grab != Some(h) {
                self.wins[i].grab = Some(h);
                if let Some(w) = &self.wins[i].window {
                    w.set_cursor(h.cursor());
                }
            }
            return;
        }
        let hit = self.wins[i].frame.as_ref().and_then(|f| f.hit_at(x, y)).map(|h| (h.key.clone(), h.action.is_some()));
        let (key, clickable) = match hit {
            Some((k, c)) => (Some(k), c),
            None => (None, false),
        };
        if self.wins[i].hover != key {
            self.wins[i].hover = key;
            self.wins[i].redraw = true;
            if let Some(w) = &self.wins[i].window {
                w.set_cursor(if clickable { CursorIcon::Pointer } else { CursorIcon::Default });
            }
        }
    }

    fn begin_drag(&mut self, i: usize, handle: Handle, cursor0: (i32, i32)) {
        if let Some(r) = self.wins[i].window.as_ref().and_then(|w| Self::outer_rect(w)) {
            self.wins[i].tween = None;
            self.wins[i].drag = Some(Drag { handle, cursor0, rect0: r, before: r });
            self.wins[i].redraw = true;
        }
    }

    /// Live move/resize: apply the drag for the current (screen, physical) cursor.
    fn drag_update(&mut self, i: usize, cursor: (i32, i32)) {
        let Some(d) = &self.wins[i].drag else { return };
        let (handle, cursor0, rect0) = (d.handle, d.cursor0, d.rect0);
        let snap = self.snap_for(i);
        let min = self.min_size_phys(i);
        // shift+drag disables snapping
        let snap = if self.mods.shift_key() { Snap { threshold: 0, grid: 0, ..snap } } else { snap };
        let r = edit::apply(handle, rect0, cursor.0 - cursor0.0, cursor.1 - cursor0.1, min, &snap);
        self.set_window_rect(i, r);
        self.wins[i].tween = None;
    }

    fn end_drag(&mut self, i: usize) {
        if let Some(d) = self.wins[i].drag.take() {
            let changed = self.wins[i].window.as_ref().and_then(|w| Self::outer_rect(w)).is_some_and(|r| r != d.before);
            if changed {
                self.undo.push(UndoEntry { id: self.ws.instances[i].id.clone(), before: d.before });
                self.commit_rect(i);
            }
            self.wins[i].redraw = true;
        }
    }

    fn card_rect(&self, i: usize) -> [f32; 4] {
        let g = self.gutter();
        let Some(w) = &self.wins[i].window else { return [0.0; 4] };
        let s = w.scale_factor() as f32;
        let size = w.inner_size();
        [g, g, size.width as f32 / s - 2.0 * g, size.height as f32 / s - 2.0 * g]
    }

    fn on_mouse(&mut self, i: usize, state: ElementState, button: MouseButton) {
        if button != MouseButton::Left {
            return;
        }
        if self.edit {
            match state {
                ElementState::Pressed => {
                    let (x, y) = self.wins[i].mouse;
                    let handle = edit::hit_handle(x, y, self.card_rect(i), 16.0);
                    self.begin_drag(i, handle, win32::cursor_pos());
                }
                ElementState::Released => self.end_drag(i),
            }
            return;
        }
        if state != ElementState::Pressed {
            return;
        }
        let (x, y) = self.wins[i].mouse;
        let action = self.wins[i].frame.as_ref().and_then(|f| f.hit_at(x, y)).and_then(|h| h.action.clone());
        if let Some(a) = action {
            self.run_action(i, &a);
        }
    }

    fn on_wheel(&mut self, i: usize, delta: MouseScrollDelta) {
        let scale = self.wins[i].window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
        let dy = match delta {
            MouseScrollDelta::LineDelta(_, y) => y * 48.0,
            MouseScrollDelta::PixelDelta(p) => p.y as f32 / scale,
        };
        let (mx, my) = self.wins[i].mouse;
        let Some(frame) = &self.wins[i].frame else { return };
        let region = frame.scrolls.iter().find(|s| {
            frame.rect_of(&s.key).is_some_and(|[x, y, w, h]| mx >= x && mx < x + w && my >= y && my < y + h)
        });
        let Some(region) = region else { return };
        let max = (region.content_h - region.view_h).max(0.0);
        let cur = self.wins[i].state.get("scroll").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        let next = (cur - dy).clamp(0.0, max);
        if (next - cur).abs() > 0.01 {
            self.wins[i].state.insert("scroll".into(), Value::Num(next as f64));
            self.wins[i].redraw = true;
        }
    }

    fn run_action(&mut self, i: usize, action: &str) {
        let (verb, rest) = action.split_once(' ').unwrap_or((action, ""));
        match verb {
            "launch" => {
                if !win32::open(rest.trim()) {
                    self.log(format!("could not open `{}`", rest.trim()));
                }
            }
            "toggle" => {
                let cur = self.wins[i].state.get(rest).map_or(false, |v| v.truthy());
                self.wins[i].state.insert(rest.to_string(), Value::Bool(!cur));
                self.wins[i].state.insert("scroll".into(), Value::Num(0.0));
                self.wins[i].redraw = true;
            }
            "set" => {
                if let Some((name, val)) = rest.split_once(' ') {
                    self.wins[i].state.insert(name.to_string(), Value::Str(val.to_string()));
                    self.wins[i].redraw = true;
                }
            }
            "settings" => {
                let _ = self.proxy.send_event(UserEvent::Menu("settings".into()));
            }
            other => self.log(format!("unknown action `{other}` in `{action}`")),
        }
    }

    fn on_key(&mut self, i: usize, key: &Key, state: ElementState) {
        if !self.edit || state != ElementState::Pressed {
            return;
        }
        let ctrl = self.mods.control_key();
        match key {
            Key::Named(NamedKey::Escape) => self.set_edit(false),
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("z") => self.undo_last(),
            Key::Named(n @ (NamedKey::ArrowLeft | NamedKey::ArrowRight | NamedKey::ArrowUp | NamedKey::ArrowDown)) => {
                let step = if self.mods.shift_key() { 10 } else { 1 };
                let (dx, dy) = match n {
                    NamedKey::ArrowLeft => (-step, 0),
                    NamedKey::ArrowRight => (step, 0),
                    NamedKey::ArrowUp => (0, -step),
                    _ => (0, step),
                };
                if let Some(r) = self.wins[i].window.as_ref().and_then(|w| Self::outer_rect(w)) {
                    self.undo.push(UndoEntry { id: self.ws.instances[i].id.clone(), before: r });
                    self.set_window_rect(i, Rect { x: r.x + dx, y: r.y + dy, ..r });
                    self.commit_rect(i);
                }
            }
            _ => {}
        }
    }

    fn idx(&self, id: &str) -> Option<usize> {
        self.ws.instances.iter().position(|c| c.id == id)
    }

    fn first_hit_with(&self, i: usize, action: &str) -> Option<(f32, f32)> {
        let f = self.wins[i].frame.as_ref()?;
        f.hits.iter().find(|h| h.action.as_deref() == Some(action)).map(|h| (h.rect[0] + h.rect[2] / 2.0, h.rect[1] + h.rect[3] / 2.0))
    }

    fn click_at(&mut self, i: usize, p: (f32, f32)) {
        self.wins[i].mouse = p;
        self.on_mouse(i, ElementState::Pressed, MouseButton::Left);
    }

    fn rect_now(&self, i: usize) -> Option<Rect> {
        self.wins[i].window.as_ref().and_then(|w| Self::outer_rect(w))
    }

    /// Scripted end-to-end check of the interactive paths, using synthetic
    /// input (never the real mouse). Results go to the log and `selftest.txt`.
    fn selftest_tick(&mut self, el: &ActiveEventLoop, now: Instant) {
        let Some(mut st) = self.selftest.take() else { return };
        if now < st.at {
            self.selftest = Some(st);
            return;
        }
        let check = |st: &mut SelfTest, name: &str, ok: bool, detail: String| {
            eprintln!("selftest: {} {name} {detail}", if ok { "PASS" } else { "FAIL" });
            st.checks.push((format!("{name} {detail}"), ok));
        };
        let both = 0x80u32 | 0x0800_0000; // WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE
        let clock = self.idx("clock-1");
        let folder = self.idx("icon_folder-1");
        let mut next = 350u64;
        match st.step {
            0 => {
                let hw = self.hwnds();
                check(&mut st, "every instance has a window", hw.len() == self.ws.instances.len() && hw.len() >= 4, format!("({} windows)", hw.len()));
                check(&mut st, "widgets are tool + no-activate windows", hw.iter().all(|h| win32::ex_style(*h) & both == both), String::new());
                let host = win32::desktop_icon_host().and_then(win32::z_index);
                let z: Vec<usize> = hw.iter().filter_map(|h| win32::z_index(*h)).collect();
                check(&mut st, "widgets sit directly above the desktop layer", host.is_some_and(|hz| z.iter().all(|zi| *zi < hz && hz - *zi <= hw.len() + 2)), format!("(z {z:?}, desktop {host:?})"));
                self.set_edit(true);
            }
            1 => {
                let hw = self.hwnds();
                check(&mut st, "edit mode drops no-activate so keys arrive", hw.iter().all(|h| win32::ex_style(*h) & 0x0800_0000 == 0 && win32::ex_style(*h) & 0x80 != 0), String::new());
            }
            2 => {
                let Some(i) = clock else { return self.selftest_abort(el, st, "no clock-1 instance") };
                let r0 = self.rect_now(i).unwrap();
                st.rect = Some(r0);
                self.begin_drag(i, Handle::Move, (1000, 500));
                self.drag_update(i, (900, 560));
                let mid = self.rect_now(i).unwrap();
                check(&mut st, "drag moves the window live, before release", (mid.x - r0.x + 100).abs() <= 10 && (mid.y - r0.y - 60).abs() <= 10, format!("(dx {}, dy {})", mid.x - r0.x, mid.y - r0.y));
                self.end_drag(i);
                next = 900;
            }
            3 => {
                let i = clock.unwrap();
                let saved = std::fs::read_to_string(Workspace::path(&self.opts.dir)).ok().and_then(|t| serde_json::from_str::<Workspace>(&t).ok());
                let on_disk = saved.and_then(|w| w.instances.into_iter().find(|c| c.id == "clock-1")).map(|c| (c.x, c.y));
                let mem = (self.ws.instances[i].x, self.ws.instances[i].y);
                let anchored = self.ws.instances[i].monitor.name.contains("DISPLAY");
                check(&mut st, "moved position is persisted, anchored to a monitor", on_disk == Some(mem) && anchored, format!("(disk {on_disk:?}, memory {mem:?})"));
                let r0 = self.rect_now(i).unwrap();
                self.begin_drag(i, Handle::SE, (0, 0));
                self.drag_update(i, (70, 50));
                self.end_drag(i);
                let r1 = self.rect_now(i).unwrap();
                check(&mut st, "SE handle resizes live, top-left fixed", r1.x == r0.x && r1.y == r0.y && r1.w >= r0.w + 60 && r1.h >= r0.h + 40, format!("({}x{} -> {}x{})", r0.w, r0.h, r1.w, r1.h));
                self.render(i);
                let sz = self.wins[i].window.as_ref().unwrap().inner_size();
                let (tw, th) = self.wins[i].target.as_ref().unwrap().size();
                check(&mut st, "render target covers the resized window", tw >= sz.width && th >= sz.height, format!("(swapchain {tw}x{th}, window {}x{})", sz.width, sz.height));
            }
            4 => {
                self.undo_last();
                self.undo_last();
                let i = clock.unwrap();
                let r = self.rect_now(i).unwrap();
                let want = st.rect;
                check(&mut st, "two undos restore the original rect", Some(r) == want, format!("({r:?} vs {want:?})"));
                self.set_edit(false);
            }
            5 => {
                let hw = self.hwnds();
                check(&mut st, "leaving edit mode restores no-activate", hw.iter().all(|h| win32::ex_style(*h) & both == both), String::new());
                let Some(i) = folder else { return self.selftest_abort(el, st, "no icon_folder-1 instance") };
                st.collapsed = self.rect_now(i);
                st.configures0 = self.gpu.as_ref().map_or(0, |g| g.configures.get());
                match self.first_hit_with(i, "toggle expanded") {
                    Some(p) => self.click_at(i, p),
                    None => check(&mut st, "folder tile is clickable", false, "(no toggle hit region)".into()),
                }
                next = 700;
            }
            6 => {
                let i = folder.unwrap();
                let c = st.collapsed.unwrap();
                let r = self.rect_now(i).unwrap();
                check(&mut st, "clicking the folder expands its window in place", r.w > c.w + 100 && r.h > c.h + 40, format!("({}x{} -> {}x{})", c.w, c.h, r.w, r.h));
                let n = self.gpu.as_ref().map_or(0, |g| g.configures.get()) - st.configures0;
                check(&mut st, "the expand animation reconfigures the swapchain at most once", n <= 1, format!("({n} reconfigures over ~12 frames)"));
                let m = self.monitor_of(&self.ws.instances[i]).unwrap().work;
                check(&mut st, "expanded window stays inside the work area", r.x >= m.0 && r.y >= m.1 && r.right() <= m.0 + m.2 as i32 && r.bottom() <= m.1 + m.3 as i32, format!("({r:?} in {m:?})"));
                let me = win32::hwnd_of(self.wins[i].window.as_ref().unwrap()).and_then(win32::z_index);
                let others: Vec<usize> = self.wins.iter().enumerate().filter(|(j, _)| *j != i).filter_map(|(_, w)| w.window.as_ref()).filter_map(|w| win32::hwnd_of(w)).filter_map(win32::z_index).collect();
                check(&mut st, "expanded folder is above sibling widgets", me.is_some_and(|m| others.iter().all(|o| m < *o)), format!("(z {me:?} vs {others:?})"));
                match self.first_hit_with(i, "toggle expanded") {
                    Some(p) => self.click_at(i, p),
                    None => check(&mut st, "folder has a close control", false, String::new()),
                }
                next = 700;
            }
            7 => {
                let i = folder.unwrap();
                let c = st.collapsed.unwrap();
                let r = self.rect_now(i).unwrap();
                check(&mut st, "closing the folder restores its exact size and place", r == c, format!("({r:?} vs {c:?})"));
                // hot reload: a broken user override must show an in-place error, never a fallback
                let wd = self.opts.dir.join("widgets");
                let _ = std::fs::create_dir_all(&wd);
                let _ = std::fs::write(wd.join("clock.toml"), "[root]\ntype='text'\ncolour='#fff'\n");
                next = 1100;
            }
            8 => {
                let i = clock.unwrap();
                let bad = self.reg.get("clock").is_some_and(|d| d.is_err());
                let shown = self.wins[i].widget_error.clone();
                check(&mut st, "editing a widget file hot-reloads it", bad, String::new());
                check(&mut st, "a broken definition shows an error card in place", shown.as_deref().is_some_and(|e| e.contains("colour")), format!("({:?})", shown.as_deref().map(|e| e.chars().take(60).collect::<String>())));
                let _ = std::fs::remove_file(self.opts.dir.join("widgets").join("clock.toml"));
                next = 1100;
            }
            9 => {
                let i = clock.unwrap();
                check(&mut st, "fixing the file heals the widget", self.reg.get("clock").is_some_and(|d| d.is_ok()) && self.wins[i].widget_error.is_none(), String::new());
                // settings-window commands: the same path the UI takes
                self.apply(el, Cmd::Param("clock-1".into(), "ticks".into(), Value::Bool(false)));
                self.apply(el, Cmd::Add("digital_clock".into()));
                self.apply(el, Cmd::Z("clock-1".into(), "normal".into()));
                self.apply(el, Cmd::Theme(crate::theme::Selection { palette: "Daylight".into(), ..Default::default() }));
                next = 500;
            }
            10 => {
                let i = clock.unwrap();
                check(&mut st, "a Param command reaches the instance's saved config", self.ws.instances[i].params.get("ticks") == Some(&serde_json::Value::Bool(false)), String::new());
                check(&mut st, "adding a widget creates a window for it", self.idx("digital_clock-2").is_some_and(|j| self.wins[j].window.is_some()) && self.hwnds().len() == 5, format!("({} windows)", self.hwnds().len()));
                let z = self.wins[i].window.as_ref().and_then(|w| win32::hwnd_of(w)).map(|h| win32::ex_style(h) & 0x8 != 0);
                check(&mut st, "a Layer command re-applies z-order (normal is not topmost)", z == Some(false), String::new());
                check(&mut st, "a Theme command re-themes every widget", self.ws.theme.palette == "Daylight" && self.theme.color("text").to_hex() == "#141a2a", String::new());
                self.apply(el, Cmd::Remove("digital_clock-2".into()));
                self.open_settings(el);
                next = 900;
            }
            11 => {
                check(&mut st, "removing a widget closes its window", self.idx("digital_clock-2").is_none() && self.hwnds().len() == 4, format!("({} windows)", self.hwnds().len()));
                check(&mut st, "the settings window opens", self.settings.is_some(), String::new());
                if let Some(s) = &self.settings {
                    s.window.request_redraw();
                }
                next = 700;
            }
            12 => {
                check(&mut st, "the settings window survives rendering frames", self.settings.is_some() && self.gpu_lost.is_none(), String::new());
                self.apply(el, Cmd::Close);
                next = 300;
            }
            13 => {
                check(&mut st, "the settings window closes", self.settings.is_none(), String::new());
                // ---- Show Desktop (ADR-002), without touching the real desktop ----
                let host = win32::desktop_icon_host();
                check(&mut st, "the desktop-icon host is found (Progman on 24H2+)", host.is_some(), format!("({host:?})"));
                if let Some(s) = self.sentinel.as_ref() {
                    check(&mut st, "normal state: the sentinel sits above the icon host", win32::desktop_state(s) == Some(false), format!("({:?})", win32::desktop_state(s)));
                    // detection against a fake host, raised and lowered around the sentinel
                    if let Some(fake) = win32::Sentinel::new() {
                        fake.show();
                        let f = fake.hwnd();
                        let (a, b) = (win32::desktop_state_with(f, s), { fake.raise(); win32::desktop_state_with(f, s) });
                        fake.sink();
                        let c = win32::desktop_state_with(f, s);
                        check(&mut st, "a host raised above the sentinel reads as Show Desktop, and back", (a, b, c) == (Some(false), Some(true), Some(false)), format!("({a:?} -> {b:?} -> {c:?})"));
                        // now play Explorer: the fake host goes to the top of the normal band, as in Show Desktop
                        fake.raise();
                        self.test_host = Some(f);
                        st.fake = Some(fake);
                    }
                } else {
                    check(&mut st, "the z-order sentinel exists", false, String::new());
                }
                self.apply(el, Cmd::Z("icon_list-1".into(), "bottom".into()));
                self.desktop_shown = true; // force the state: Explorer is not asked to hide anything
                self.apply_show_desktop();
                next = 350;
            }
            14 => {
                let topmost = |s: &Self, id: &str| s.idx(id).and_then(|i| s.wins[i].window.as_ref()).and_then(|w| win32::hwnd_of(w)).map(|h| win32::ex_style(h) & 0x8 != 0);
                check(&mut st, "Desktop-layer widgets float above the desktop while it is shown", topmost(self, "icon_folder-1") == Some(true) && topmost(self, "digital_clock-1") == Some(true), String::new());
                check(&mut st, "Bottom-layer widgets stay under it, hidden by design", topmost(self, "icon_list-1") == Some(false), String::new());
                let order = win32::z_order();
                let me_pid = std::process::id();
                let me = self.idx("icon_folder-1").and_then(|i| self.wins[i].window.as_ref()).and_then(|w| win32::hwnd_of(w)).and_then(win32::z_index);
                let fz = st.fake.as_ref().and_then(|f| win32::z_index(f.hwnd()));
                // What the walk-up guarantees, independent of the environment: the widget is above
                // the raised desktop, and no foreign topmost window is left between the two, so it
                // sits directly under the backmost foreign topmost window (the taskbar layer).
                // (The taskbar's own z-index is no yardstick: Windows demotes it below normal
                // windows while a fullscreen app is in front.)
                let between: Vec<String> = match (me, fz) {
                    (Some(m), Some(f)) => order.iter().enumerate().filter(|(i, e)| *i > m && *i < f && e.topmost && e.pid != me_pid).map(|(_, e)| e.class.clone()).collect(),
                    _ => vec![],
                };
                let detail = format!("(widget z {me:?}, raised desktop z {fz:?}, foreign topmost windows in between: {between:?})");
                check(&mut st, "floating widgets sit directly under the backmost topmost window, above the raised desktop", matches!((me, fz), (Some(m), Some(f)) if m < f) && between.is_empty(), detail);
                self.desktop_shown = false;
                self.apply_show_desktop();
                next = 350;
            }
            15 => {
                let hosts = win32::desktop_icon_host().and_then(win32::z_index);
                let f = self.idx("icon_folder-1").and_then(|i| self.wins[i].window.as_ref()).and_then(|w| win32::hwnd_of(w));
                let topmost = f.map(|h| win32::ex_style(h) & 0x8 != 0);
                let me = f.and_then(win32::z_index);
                check(&mut st, "leaving Show Desktop sinks widgets back above the desktop layer", topmost == Some(false) && matches!((me, hosts), (Some(m), Some(h)) if m < h), format!("(widget z {me:?}, host z {hosts:?}, topmost {topmost:?})"));
                if let Some(h) = f {
                    let before = win32::z_index(h);
                    win32::try_raise(h); // a foreign attempt to bring the widget to the front
                    let after = win32::z_index(h);
                    check(&mut st, "the z-order guard vetoes a foreign raise", before == after, format!("(z {before:?} -> {after:?})"));
                    win32::float_over_desktop(h, win32::desktop_icon_host().unwrap_or(h));
                    let ok = win32::ex_style(h) & 0x8 != 0;
                    win32::sink_to_desktop(h);
                    check(&mut st, "our own z-order calls pass the guard (NOSENDCHANGING)", ok && win32::ex_style(h) & 0x8 == 0, String::new());
                }
                self.apply(el, Cmd::Z("icon_list-1".into(), "desktop".into()));
                self.test_host = None;
                st.fake = None; // drops the fake host window
                next = 200;
            }
            16 => {
                let pass = st.checks.iter().filter(|c| c.1).count();
                let total = st.checks.len();
                let report: Vec<String> = st.checks.iter().map(|(n, ok)| format!("{} {n}", if *ok { "PASS" } else { "FAIL" })).collect();
                let _ = std::fs::write(self.opts.dir.join("selftest.txt"), format!("{pass}/{total}\n{}\n", report.join("\n")));
                self.log(format!("selftest: {pass}/{total} passed"));
                el.exit();
                return;
            }
            _ => {}
        }
        st.step += 1;
        st.at = now + Duration::from_millis(next);
        self.selftest = Some(st);
    }

    fn selftest_abort(&mut self, el: &ActiveEventLoop, st: SelfTest, why: &str) {
        self.log(format!("selftest aborted: {why}"));
        let lines: Vec<String> = st.checks.iter().map(|(n, ok)| format!("{} {n}", if *ok { "PASS" } else { "FAIL" })).collect();
        let _ = std::fs::write(self.opts.dir.join("selftest.txt"), format!("aborted: {why}\n{}", lines.join("\n")));
        el.exit();
    }

    fn undo_last(&mut self) {
        let Some(u) = self.undo.pop() else { return };
        if let Some(i) = self.ws.instances.iter().position(|c| c.id == u.id) {
            self.set_window_rect(i, u.before);
            self.commit_rect(i);
        }
    }

    // ---- commands from the settings window ---------------------------------------------

    fn default_size(&self, widget: &str) -> (f32, f32) {
        let g = 2.0 * self.gutter();
        match self.reg.get(widget) {
            Some(Ok(d)) => (d.size.0 + g, d.size.1 + g),
            _ => (200.0 + g, 120.0 + g),
        }
    }

    fn add_instance(&mut self, el: &ActiveEventLoop, widget: &str) {
        let (w, h) = self.default_size(widget);
        let mon = self.monitors.first().cloned();
        let n = self.ws.instances.len() as f32;
        let mut cfg = InstanceCfg {
            id: self.ws.next_id(widget),
            widget: widget.to_string(),
            monitor: mon.map(|m| m.reference()).unwrap_or_default(),
            x: 80.0 + 28.0 * (n % 8.0),
            y: 80.0 + 28.0 * (n % 8.0),
            w,
            h,
            ..Default::default()
        };
        if matches!(widget, "icon_list" | "icon_folder") {
            cfg.set_items(&default_shortcuts());
        }
        self.log(format!("added {}", cfg.id));
        self.ws.instances.push(cfg);
        self.sync_windows(el);
        self.sync_watchers();
        self.mark_save();
    }

    fn apply(&mut self, el: &ActiveEventLoop, cmd: Cmd) {
        let find = |s: &Self, id: &str| s.ws.instances.iter().position(|c| c.id == id);
        match cmd {
            Cmd::Add(w) => self.add_instance(el, &w),
            Cmd::Remove(id) => {
                if let Some(i) = find(self, &id) {
                    self.ws.instances.remove(i);
                    self.wins.remove(i);
                    self.sync_watchers();
                    self.mark_save();
                    self.log(format!("removed {id}"));
                }
            }
            Cmd::Param(id, name, v) => {
                if let Some(i) = find(self, &id) {
                    self.ws.instances[i].set_param(&name, &v);
                    if name == "folder" {
                        self.sync_watchers();
                    }
                    self.refresh_items(i);
                    self.wins[i].redraw = true;
                    self.mark_save();
                }
            }
            Cmd::Items(id, items) => {
                if let Some(i) = find(self, &id) {
                    self.ws.instances[i].set_items(&items);
                    self.refresh_items(i);
                    self.wins[i].redraw = true;
                    self.mark_save();
                }
            }
            Cmd::Z(id, z) => {
                if let Some(i) = find(self, &id) {
                    self.ws.instances[i].z = z.clone();
                    if let Some(h) = self.wins[i].window.as_ref().and_then(|w| win32::hwnd_of(w)) {
                        win32::set_zmode(h, ZMode::parse(&z).unwrap_or(ZMode::Desktop));
                    }
                    self.wins[i].raised = false;
                    self.guard_z(i);
                    if self.desktop_shown {
                        self.apply_show_desktop();
                    }
                    self.mark_save();
                }
            }
            Cmd::ClickThrough(id, on) => {
                if let Some(i) = find(self, &id) {
                    self.ws.instances[i].click_through = on;
                    if let Some(w) = &self.wins[i].window {
                        let _ = w.set_cursor_hittest(!on || self.edit);
                        win32::set_no_activate(w, !self.edit);
                    }
                    self.mark_save();
                }
            }
            Cmd::ResetPos(id) => {
                if let Some(i) = find(self, &id) {
                    let (w, h) = self.default_size(&self.ws.instances[i].widget.clone());
                    let cfg = &mut self.ws.instances[i];
                    (cfg.x, cfg.y, cfg.w, cfg.h) = (60.0, 60.0, w, h);
                    if let Some(m) = self.monitors.first() {
                        cfg.monitor = m.reference();
                    }
                    if let Some(p) = workspace::resolve(&self.ws.instances[i], &self.monitors) {
                        let s = self.scale_of(i);
                        self.set_window_rect(i, Rect::new(p.0, p.1, (w as f64 * s) as i32, (h as f64 * s) as i32));
                    }
                    self.sync_windows(el);
                    self.mark_save();
                }
            }
            Cmd::Theme(sel) => {
                let icon_changed = sel.icon_pack != self.ws.theme.icon_pack;
                self.ws.theme = sel;
                self.rebuild_theme();
                if icon_changed {
                    for i in 0..self.wins.len() {
                        self.refresh_items(i);
                    }
                }
                self.mark_save();
            }
            Cmd::Override(k, v) => {
                match v {
                    Some(v) => self.ws.overrides.insert(k, v),
                    None => self.ws.overrides.remove(&k),
                };
                self.rebuild_theme();
                self.mark_save();
            }
            Cmd::Gpu(g) => {
                self.ws.gpu = g;
                self.mark_save();
                self.log("GPU preference saved: restart Wayfinder to apply it");
            }
            Cmd::Autostart(on) => {
                self.ws.autostart = on;
                if let Err(e) = set_autostart(on) {
                    self.log(format!("autostart: {e}"));
                }
                self.mark_save();
            }
            Cmd::Grid(g) => {
                self.ws.grid = g;
                self.mark_save();
            }
            Cmd::Edit(on) => self.set_edit(on),
            Cmd::Reload => self.reload(),
            Cmd::OpenFolder => {
                let d = self.opts.dir.join("widgets");
                let _ = std::fs::create_dir_all(&d);
                win32::open(&d.to_string_lossy());
            }
            Cmd::Quit => el.exit(),
            Cmd::Close => self.settings = None,
            Cmd::Minimize => {} // handled by the settings window itself
        }
    }

    /// The driver reset (or a surface could not be recovered): rebuild the
    /// device and every window's target. Repeated losses degrade to the
    /// software renderer for this session rather than looping.
    fn recover_gpu(&mut self, el: &ActiveEventLoop, why: &str) {
        let now = Instant::now();
        self.gpu_recoveries.retain(|t| now.duration_since(*t) < Duration::from_secs(90));
        self.gpu_recoveries.push(now);
        if self.gpu_recoveries.len() >= 3 && !self.degraded {
            self.degraded = true;
            self.log("the GPU was lost 3 times in 90 s: switching to the software renderer for this session");
        }
        self.log(format!("GPU lost ({why}): rebuilding"));
        self.settings = None;
        self.gpu = None;
        self.icons.forget();
        let windows: Vec<(usize, Arc<Window>)> = self.wins.iter().enumerate().filter_map(|(i, w)| w.window.clone().map(|win| (i, win))).collect();
        for w in &mut self.wins {
            w.target = None;
        }
        let power = self.power();
        for (i, win) in windows {
            let target = match self.gpu.as_mut() {
                Some(g) => g.target_for(&win),
                None => Gpu::new(&win, power).map(|(g, t)| {
                    self.gpu = Some(g);
                    t
                }),
            };
            match target {
                Ok(t) => self.wins[i].target = Some(t),
                Err(e) => self.log(format!("could not rebuild window {i}: {e}")),
            }
            self.wins[i].redraw = true;
        }
        let _ = el;
        if let Some(g) = &self.gpu {
            let info = g.info.clone();
            self.log(format!("gpu: {info}"));
        }
    }

    fn reload(&mut self) {
        self.lib = Library::load(&self.opts.dir);
        self.reg = Registry::load(&self.opts.dir.join("widgets"));
        for e in self.lib.errors.clone().into_iter().chain(self.reg.errors()) {
            self.log(e);
        }
        self.rebuild_theme();
        for i in 0..self.wins.len() {
            self.refresh_items(i);
        }
        self.log("reloaded widget definitions, themes and folders");
    }

    // ---- file watching -------------------------------------------------------------------

    /// `only_assets`: the data-dir watcher reacts to definition/theme/font/icon
    /// files only. Anything else in there (our own log, workspace.json and its
    /// temp file) must never count, or saving would trigger a reload forever.
    fn watcher(&self, only_assets: bool) -> Option<RecommendedWatcher> {
        let proxy = self.proxy.clone();
        notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            if matches!(ev.kind, notify::EventKind::Access(_)) {
                return;
            }
            let relevant = ev.paths.iter().any(|p| {
                !only_assets || p.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "toml" | "png" | "ttf" | "otf" | "ttc"))
            });
            if relevant {
                let _ = proxy.send_event(UserEvent::Files);
            }
        })
        .ok()
    }

    fn sync_watchers(&mut self) {
        // one recursive watch on the data dir (widgets, themes, icon packs) plus
        // one per mirrored folder
        if self.watchers.is_empty() {
            let _ = std::fs::create_dir_all(&self.opts.dir);
            if let Some(mut w) = self.watcher(true) {
                if w.watch(&self.opts.dir, RecursiveMode::Recursive).is_ok() {
                    self.watchers.push(w);
                }
            }
        }
        let want: Vec<(String, String)> = self.ws.instances.iter().filter(|c| !c.folder().is_empty()).map(|c| (c.id.clone(), c.folder())).collect();
        if want != self.watched_folders {
            self.watchers.truncate(1);
            self.watched_folders = want.clone();
            for (_, dir) in want {
                if let Some(mut w) = self.watcher(false) {
                    if w.watch(std::path::Path::new(&dir), RecursiveMode::NonRecursive).is_ok() {
                        self.watchers.push(w);
                    }
                }
            }
        }
    }

    fn open_settings(&mut self, el: &ActiveEventLoop) {
        if let Some(s) = &self.settings {
            s.window.focus_window();
            return;
        }
        let power = self.power();
        match SettingsWin::open(el, &mut self.gpu, power) {
            Ok(s) => self.settings = Some(s),
            Err(e) => self.log(format!("settings: {e}")),
        }
    }

    fn init_tray(&mut self) {
        let menu = Menu::new();
        let items = [
            MenuItem::with_id("edit", "Edit layout   Ctrl+Alt+E", true, None),
            MenuItem::with_id("settings", "Settings...", true, None),
            MenuItem::with_id("reload", "Reload widgets and themes", true, None),
            MenuItem::with_id("folder", "Open widgets folder", true, None),
        ];
        for it in &items {
            let _ = menu.append(it);
        }
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&MenuItem::with_id("quit", "Quit Wayfinder", true, None));
        let p = self.proxy.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let _ = p.send_event(UserEvent::Menu(e.id.0.clone()));
        }));
        let p = self.proxy.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click { button: tray_icon::MouseButton::Left, button_state: tray_icon::MouseButtonState::Up, .. } = e {
                let _ = p.send_event(UserEvent::TrayClick);
            }
        }));
        match TrayIconBuilder::new().with_menu(Box::new(menu)).with_menu_on_left_click(false).with_tooltip("Wayfinder").with_icon(tray_icon_image()).build() {
            Ok(t) => self.tray = Some(t),
            Err(e) => self.log(format!("tray icon failed: {e}")),
        }
    }

    fn init_hotkey(&mut self) {
        match GlobalHotKeyManager::new() {
            Ok(m) => {
                let hk = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyE);
                if let Err(e) = m.register(hk) {
                    self.log(format!("hotkey Ctrl+Alt+E unavailable ({e}); use the tray menu for Edit Mode"));
                }
                let p = self.proxy.clone();
                GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
                    if e.state == HotKeyState::Pressed {
                        let _ = p.send_event(UserEvent::Hotkey);
                    }
                }));
                self._hotkeys = Some(m);
            }
            Err(e) => self.log(format!("global hotkeys unavailable: {e}")),
        }
    }
}

fn default_shortcuts() -> Vec<Shortcut> {
    let win = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    [("Notepad", format!("{win}\\notepad.exe")), ("Calculator", format!("{win}\\System32\\calc.exe")), ("Explorer", format!("{win}\\explorer.exe")), ("Terminal", format!("{win}\\System32\\cmd.exe"))]
        .into_iter()
        .map(|(n, t)| Shortcut { name: n.into(), target: t, icon: String::new() })
        .collect()
}

/// Start with Windows (HKCU Run key).
fn set_autostart(on: bool) -> Result<(), String> {
    use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW};
    use windows::core::w;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    unsafe {
        let mut key = HKEY::default();
        RegOpenKeyExW(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"), None, KEY_SET_VALUE, &mut key).ok().map_err(|e| e.to_string())?;
        let r = if on {
            let v: Vec<u16> = format!("\"{}\"", exe.display()).encode_utf16().chain([0]).collect();
            RegSetValueExW(key, w!("Wayfinder"), None, REG_SZ, Some(std::slice::from_raw_parts(v.as_ptr().cast(), v.len() * 2)))
        } else {
            RegDeleteValueW(key, w!("Wayfinder"))
        };
        let _ = RegCloseKey(key);
        if on { r.ok().map_err(|e| e.to_string()) } else { Ok(()) } // deleting a missing value is fine
    }
}

/// First-run arrangement: right-hand side of the primary monitor, clear of the desktop icons.
fn default_instances(monitors: &[MonitorInfo], reg: &Registry, gutter: f32) -> Vec<InstanceCfg> {
    let Some(m) = monitors.iter().find(|m| m.x == 0 && m.y == 0).or(monitors.first()) else { return vec![] };
    let logical_w = m.work.2 as f32 / m.scale as f32;
    let size = |id: &str| match reg.get(id) {
        Some(Ok(d)) => (d.size.0 + 2.0 * gutter, d.size.1 + 2.0 * gutter),
        _ => (240.0, 160.0),
    };
    let mut out = Vec::new();
    let mut col = |id: &str, widget: &str, x_from_right: f32, y: f32, items: bool| {
        let (w, h) = size(widget);
        let mut c = InstanceCfg { id: id.into(), widget: widget.into(), monitor: m.reference(), x: (logical_w - x_from_right - w).max(0.0), y, w, h, ..Default::default() };
        if items {
            c.set_items(&default_shortcuts());
        }
        out.push(c);
        (w, h)
    };
    let (cw, ch) = col("clock-1", "clock", 8.0, 8.0, false);
    let (_, dh) = col("digital_clock-1", "digital_clock", 8.0, 8.0 + ch - 20.0, false);
    let x2 = 8.0 + cw.max(340.0) - 20.0;
    let (lw, lh) = col("icon_list-1", "icon_list", x2, 8.0, true);
    col("icon_folder-1", "icon_folder", x2 + lw - 20.0, 8.0, true);
    let _ = (dh, lh);
    out
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.booted {
            return;
        }
        self.booted = true;
        self.monitors = win32::monitors(el);
        let lines: Vec<String> = self.monitors.iter().map(|m| format!("monitor {}: {}x{} @ {:.2}x, work area {:?}", m.name, m.w, m.h, m.scale, m.work)).collect();
        for l in lines {
            self.log(l);
        }
        self.sentinel = win32::Sentinel::new();
        if self.sentinel.is_none() {
            self.log("could not create the z-order sentinel window; Show Desktop handling is off");
        }
        self.init_tray();
        self.init_hotkey();
        if self.ws.instances.is_empty() {
            self.ws.instances = default_instances(&self.monitors, &self.reg, self.gutter());
            self.mark_save();
            self.log("first run: created the default widgets");
        }
        self.sync_windows(el);
        self.sync_watchers();
        let (p1, p2, p3) = (self.proxy.clone(), self.proxy.clone(), self.proxy.clone());
        let _ = p3;
        win32::watch_shell_events(move || {
            let _ = p1.send_event(UserEvent::Shell);
        });
        if let Some(w) = self.wins.iter().find_map(|w| w.window.clone()) {
            win32::watch_display_changes(&w, move || {
                let _ = p2.send_event(UserEvent::Display);
            });
        }
        self.check_show_desktop();
        if self.start_edit {
            self.set_edit(true);
        }
        self.log(format!("ready: {} instance(s), theme {} / {} / {}", self.ws.instances.len(), self.ws.theme.palette, self.ws.theme.fonts, self.ws.theme.glyphs));
    }

    fn user_event(&mut self, el: &ActiveEventLoop, ev: UserEvent) {
        match ev {
            UserEvent::Menu(id) => match id.as_str() {
                "edit" => self.set_edit(!self.edit),
                "settings" => self.open_settings(el),
                "reload" => self.reload(),
                "folder" => self.apply(el, Cmd::OpenFolder),
                "quit" => el.exit(),
                _ => {}
            },
            UserEvent::Hotkey => self.set_edit(!self.edit),
            UserEvent::TrayClick => self.open_settings(el),
            UserEvent::Files => self.reload_at = Some(Instant::now() + Duration::from_millis(250)),
            UserEvent::Shell => {
                // Explorer reorders a few ms after the event: check a few times, growing gaps
                let now = Instant::now();
                self.shell_due = [4u64, 20, 60, 140, 300, 700].iter().map(|ms| now + Duration::from_millis(*ms)).collect();
            }
            UserEvent::Display => self.display_at = Some(Instant::now() + Duration::from_millis(500)),
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, ev: WindowEvent) {
        if self.settings.as_ref().is_some_and(|s| s.window.id() == id) {
            let gpu_info = self.gpu.as_ref().map(|g| g.info.clone()).unwrap_or_else(|| "no GPU yet".into());
            let App { ws, reg, lib, theme, log, edit, settings, text, icons, gpu, families, wins, .. } = self;
            let parked: Vec<String> = ws.instances.iter().zip(wins.iter()).filter(|(_, w)| w.window.is_none()).map(|(c, _)| c.id.clone()).collect();
            let ctx = settings::Ctx { ws, reg, lib, theme, log, gpu_info: &gpu_info, fonts: families, edit: *edit, parked: &parked };
            let s = settings.as_mut().unwrap();
            let cmds = s.event(&ev, &ctx, text);
            if matches!(ev, WindowEvent::RedrawRequested) {
                if let Some(g) = gpu.as_mut() {
                    s.render(g, text, icons, &ctx);
                }
            }
            for c in cmds {
                self.apply(el, c);
            }
            return;
        }
        let Some(i) = self.index_of(id) else { return };
        match ev {
            WindowEvent::RedrawRequested => self.render(i),
            WindowEvent::CursorMoved { position, .. } => self.on_cursor(i, position),
            WindowEvent::CursorLeft { .. } => {
                if self.wins[i].hover.take().is_some() {
                    self.wins[i].redraw = true;
                }
                self.wins[i].mouse = (-1.0, -1.0);
            }
            WindowEvent::MouseInput { state, button, .. } => self.on_mouse(i, state, button),
            WindowEvent::MouseWheel { delta, .. } => self.on_wheel(i, delta),
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::KeyboardInput { event, .. } => self.on_key(i, &event.logical_key, event.state),
            WindowEvent::Resized(s) => {
                if let (Some(g), Some(t)) = (self.gpu.as_ref(), self.wins[i].target.as_mut()) {
                    g.fit(t, s.width, s.height);
                }
                self.wins[i].redraw = true;
            }
            WindowEvent::ScaleFactorChanged { .. } => self.wins[i].redraw = true,
            WindowEvent::CloseRequested => {} // widgets are closed from Settings, not by Alt+F4
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        let now = Instant::now();
        if let Some(t) = self.opts.exit_after {
            if now.duration_since(self.started).as_secs_f32() >= t {
                if self.save_at.is_some() {
                    let _ = self.ws.save(&self.opts.dir);
                }
                el.exit();
                return;
            }
        }
        while self.shell_due.first().is_some_and(|t| *t <= now) {
            self.shell_due.remove(0);
            self.check_show_desktop();
        }
        if self.display_at.is_some_and(|t| t <= now) {
            self.display_at = None;
            self.monitors = win32::monitors(el);
            self.log(format!("displays changed: {} monitor(s)", self.monitors.len()));
            self.sync_windows(el);
            // reposition windows whose monitor moved or changed scale
            for i in 0..self.wins.len() {
                if let (Some(p), Some(w)) = (workspace::resolve(&self.ws.instances[i], &self.monitors), self.wins[i].window.clone()) {
                    let s = self.scale_of(i);
                    let cfg = &self.ws.instances[i];
                    if let Some(h) = win32::hwnd_of(&w) {
                        win32::set_rect(h, p.0, p.1, (cfg.w as f64 * s).round() as i32, (cfg.h as f64 * s).round() as i32);
                    }
                    self.wins[i].want = None;
                }
            }
            self.redraw_all();
        }
        if self.reload_at.is_some_and(|t| t <= now) {
            self.reload_at = None;
            self.reload();
        }
        if self.save_at.is_some_and(|t| t <= now) {
            self.save_at = None;
            if let Err(e) = self.ws.save(&self.opts.dir) {
                self.log(format!("could not save workspace: {e}"));
            }
        }

        self.selftest_tick(el, now);
        if let Some(why) = self.gpu_lost.take() {
            self.recover_gpu(el, &why);
        } else if self.gpu.as_ref().is_some_and(|g| g.is_lost()) {
            self.recover_gpu(el, "device lost callback");
        }
        let interval = Duration::from_micros(16_667);
        let mut wake: Option<Instant> = [self.save_at, self.reload_at, self.shell_due.first().copied(), self.display_at, self.selftest.as_ref().map(|t| t.at), self.opts.exit_after.map(|t| self.started + Duration::from_secs_f32(t))]
            .into_iter()
            .flatten()
            .min();
        let soonest = |t: Instant, wake: &mut Option<Instant>| *wake = Some(wake.map_or(t, |w| w.min(t)));
        for iw in &mut self.wins {
            let Some(w) = &iw.window else { continue };
            let due = iw.redraw || iw.next_tick.is_some_and(|t| t <= now) || (iw.animating && now.duration_since(iw.last_render) >= interval);
            if due {
                if !iw.requested {
                    iw.requested = true;
                    w.request_redraw();
                }
                continue;
            }
            if iw.animating {
                soonest(iw.last_render + interval, &mut wake);
            } else if let Some(t) = iw.next_tick {
                soonest(t, &mut wake);
            }
        }
        if let Some(s) = &self.settings {
            match s.next_frame(now) {
                Some(t) if t <= now => s.window.request_redraw(),
                Some(t) => soonest(t, &mut wake),
                None => {}
            }
        }
        el.set_control_flow(match wake {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
    }

    fn exiting(&mut self, _el: &ActiveEventLoop) {
        if let Err(e) = self.ws.save(&self.opts.dir) {
            eprintln!("wayfinder: could not save workspace on exit: {e}");
        }
        let _ = DrawList::default();
    }
}
