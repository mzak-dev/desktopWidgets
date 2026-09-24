//! One OS window per Instance, redrawn only on a changed binding, an
//! animation or input (decision 16). Each concern is an `impl App` in a submodule.

mod commands;
mod desktop;
mod edit_mode;
mod first_run;
mod host;
mod input;
mod instance;
mod render;
mod selftest;
mod tray;

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

use crate::anim::{self, Anim, Ease};
use crate::card::Card;
use crate::content::{Catalog, Root};
use crate::data::{self, DataSources};
use crate::draw::DrawList;
use crate::edit::{self, Handle, Rect, Snap};
use crate::gfx::{Gpu, Power, RenderError, Target};
use crate::icons::IconService;
use crate::platform::win32::{self, ZMode};
use crate::plugins::{self, Plugin, PluginStore};
use crate::settings::{self, Cmd, Scope, SettingsWin};
use crate::text::TextEngine;
use crate::theme::{Library, Theme};
use crate::ui::{self, Env, Frame};
use crate::value::Value;
use crate::widgets::{self, ActionCx, Def, ExpandInfo, Host, Registry, Services, View};
use crate::workspace::{self, InstanceCfg, MonitorInfo, Workspace};

use self::edit_mode::UndoEntry;
use self::first_run::{default_instances, write_missing_guides};
use self::host::AppHost;
use self::instance::{Drag, EXPAND_SECS, GLIDE_SECS, Instance, VerbOutcome, SizeTween, base_size_after_edit, engine_action, expand_target, scrolled_offset, snap_offset};
use self::selftest::SelfTest;

#[derive(Debug)]
pub enum UserEvent {
    Menu(String),
    Hotkey,
    TrayClick,
    FilesChanged,
    ForegroundChanged,
    DisplaysChanged,
}

pub struct Options {
    pub dir: PathBuf,
    pub selftest: bool,
    pub gpu_override: Option<String>,
    pub exit_after_secs: Option<f32>,
}

pub struct App {
    proxy: EventLoopProxy<UserEvent>,
    opts: Options,
    ws: Workspace,
    lib: Library,
    theme: Theme,
    reg: Registry,
    sources: DataSources,
    gpu: Option<Gpu>,
    text: TextEngine,
    icons: IconService,
    wins: Vec<Instance>,
    settings: Option<SettingsWin>,
    edit: bool,
    /// The Instance whose Edit Mode remove button was clicked once and now asks "Remove?".
    remove_armed: Option<String>,
    undo: Vec<UndoEntry>,
    mods: ModifiersState,
    monitors: Vec<MonitorInfo>,
    tray: Option<TrayIcon>,
    _hotkeys: Option<GlobalHotKeyManager>,
    watchers: Vec<RecommendedWatcher>,
    watched_paths: Vec<(String, PathBuf)>,
    log: Vec<String>,
    save_at: Option<Instant>,
    reload_at: Option<Instant>,
    show_desktop_checks: Vec<Instant>,
    sentinel: Option<win32::Sentinel>,
    fake_icon_host: Option<windows::Win32::Foundation::HWND>,
    display_at: Option<Instant>,
    desktop_shown: bool,
    started: Instant,
    booted: bool,
    start_edit: bool,
    selftest: Option<SelfTest>,
    gpu_lost_reason: Option<String>,
    gpu_recoveries: Vec<Instant>,
    forced_software: bool,
    families: Vec<String>,
    plugins: Vec<Plugin>,
}

impl App {
    pub fn new(proxy: EventLoopProxy<UserEvent>, opts: Options) -> App {
        let dir = opts.dir.clone();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::remove_file(dir.join("wayfinder.log")); // one log per run
        let guide_errors = write_missing_guides(&dir);
        let (ws, ws_err) = Workspace::load(&dir);
        let theme = Theme::default(); // composed by `rebuild_theme` below
        let text = TextEngine::new();
        let mut app = App {
            proxy,
            icons: IconService::default(),
            opts,
            ws,
            lib: Library::default(), // filled by `load_content` below
            theme,
            reg: Registry::default(),
            sources: DataSources::builtin(),
            gpu: None,
            text,
            wins: Vec::new(),
            settings: None,
            edit: false,
            remove_armed: None,
            undo: Vec::new(),
            mods: ModifiersState::empty(),
            monitors: Vec::new(),
            tray: None,
            _hotkeys: None,
            watchers: Vec::new(),
            watched_paths: Vec::new(),
            log: Vec::new(),
            save_at: None,
            reload_at: None,
            show_desktop_checks: Vec::new(),
            sentinel: None,
            fake_icon_host: None,
            display_at: None,
            desktop_shown: false,
            started: Instant::now(),
            booted: false,
            start_edit: false,
            selftest: None,
            gpu_lost_reason: None,
            gpu_recoveries: Vec::new(),
            forced_software: false,
            families: Vec::new(),
            plugins: Vec::new(),
        };
        app.load_content();
        app.rebuild_theme();
        app.selftest = app.opts.selftest.then(|| SelfTest { step: 0, at: Instant::now() + Duration::from_millis(2200), checks: Vec::new(), rect: None, collapsed: None, configures0: 0, fake: None });
        if let Some(e) = ws_err {
            app.log(e);
        }
        for e in guide_errors {
            app.log(e);
        }
        app
    }

    fn power(&self) -> Power {
        if self.forced_software {
            return Power::Software;
        }
        Power::parse(self.opts.gpu_override.as_deref().unwrap_or(&self.ws.gpu))
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
        self.theme = self.ws.global_theme(&self.lib);
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

    /// ponytail: composed per call (a few map merges, never while idle); cache on Instance if drags ever profile slow.
    fn theme_of(&self, i: usize) -> Theme {
        self.ws.theme_for(&self.lib, &self.ws.instances[i])
    }

    /// Applies a style change, then refits every window whose blur (and so gutter) changed.
    fn restyle(&mut self, change: impl FnOnce(&mut Workspace)) {
        let was: Vec<Card> = (0..self.wins.len()).map(|i| self.card(i)).collect();
        change(&mut self.ws);
        for (i, w) in was.into_iter().enumerate() {
            if self.card(i).blur != w.blur {
                self.refit_window_around_card(i, w);
            }
        }
        self.rebuild_theme();
        self.mark_save();
    }

    fn card(&self, i: usize) -> Card {
        Card::new(&self.theme_of(i))
    }

    fn new_card(&self) -> Card {
        Card::new(&self.theme)
    }

    fn monitor_of(&self, cfg: &InstanceCfg) -> Option<&MonitorInfo> {
        self.monitors.iter().find(|m| m.name == cfg.monitor.name)
    }

    fn sync_windows(&mut self, el: &ActiveEventLoop) {
        while self.wins.len() < self.ws.instances.len() {
            self.wins.push(Instance::new());
        }
        self.wins.truncate(self.ws.instances.len());
        let hidden = plugins::hidden_instances(&self.ws, &self.reg, &self.plugins);
        for i in 0..self.ws.instances.len() {
            let id = &self.ws.instances[i].id;
            let pos = if hidden.contains_key(id) { None } else { workspace::resolve(&self.ws.instances[i], &self.monitors) };
            match (pos, self.wins[i].window.is_some()) {
                (Some(p), false) => {
                    if let Err(e) = self.create_window(el, i, p) {
                        let id = self.ws.instances[i].id.clone();
                        self.log(format!("could not create window for {id}: {e}"));
                    }
                }
                (None, true) => {
                    let id = self.ws.instances[i].id.clone();
                    match hidden.get(&id) {
                        Some(p) => self.log(format!("{id} is hidden while the plugin {p} is off")),
                        None => self.log(format!("monitor for {id} is gone: parked (kept in place for when it returns)")),
                    }
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
        self.render(i);
        window.set_visible(true);
        // winit rebuilds WS_EX_* from its own flags on every state change, so our
        // styles go on last, and again after anything that touches them
        win32::set_no_activate(&window, !self.edit);
        self.update_z_guard(i);
        if self.desktop_shown {
            self.apply_show_desktop();
        }
        Ok(())
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

    /// Three losses in 90 s switch to the software renderer instead of looping (ADR-006).
    fn recover_gpu(&mut self, el: &ActiveEventLoop, why: &str) {
        let now = Instant::now();
        self.gpu_recoveries.retain(|t| now.duration_since(*t) < Duration::from_secs(90));
        self.gpu_recoveries.push(now);
        if self.gpu_recoveries.len() >= 3 && !self.forced_software {
            self.forced_software = true;
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

    /// The folders content is read from, after the built-ins; later ones win.
    fn content_roots(&self) -> Vec<Root> {
        let mut roots = plugins::roots(&self.plugins, &self.ws.disabled_plugins);
        roots.push(Root::user(&self.opts.dir));
        roots
    }

    /// Plugins, then Widgets, themes and Icon Packs from every content root, with their
    /// errors logged.
    fn load_content(&mut self) {
        self.plugins = PluginStore::new(&self.opts.dir).list();
        let broken: Vec<String> = self.plugins.iter().filter_map(|p| p.manifest.as_ref().err().map(|e| format!("plugin {}: {e}", p.id))).collect();
        for e in broken {
            self.log(e);
        }
        let cat = Catalog::load(&self.content_roots());
        self.reg = cat.registry;
        self.lib = cat.library;
        self.icons.set_packs(cat.icon_packs);
        if let Some(g) = self.gpu.as_mut() {
            self.icons.flush_files(g);
        }
        let font_problems = self.text.sync_fonts(&cat.font_files);
        self.families = self.text.family_names();
        for e in self.lib.errors.clone().into_iter().chain(self.reg.errors()).chain(font_problems) {
            self.log(e);
        }
    }

    fn reload(&mut self, el: &ActiveEventLoop) {
        self.load_content();
        self.sync_windows(el);
        self.rebuild_theme();
        self.sources.invalidate();
        if let Some(s) = &mut self.settings {
            s.invalidate();
        }
        self.log("reloaded widget definitions, themes and folders");
    }

    /// With `only_assets`, our own log and workspace.json don't count, or saving would reload forever.
    fn watcher(&self, only_assets: bool) -> Option<RecommendedWatcher> {
        let proxy = self.proxy.clone();
        let data = self.opts.dir.clone();
        notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            if matches!(ev.kind, notify::EventKind::Access(_)) {
                return;
            }
            let relevant = ev.paths.iter().any(|p| !only_assets || plugins::is_content_change(&data, p));
            if relevant {
                let _ = proxy.send_event(UserEvent::FilesChanged);
            }
        })
        .ok()
    }

    fn sync_watchers(&mut self) {
        if self.watchers.is_empty() {
            let _ = std::fs::create_dir_all(&self.opts.dir);
            if let Some(mut w) = self.watcher(true) {
                if w.watch(&self.opts.dir, RecursiveMode::Recursive).is_ok() {
                    self.watchers.push(w);
                }
            }
        }
        let want: Vec<(String, PathBuf)> = self.ws.instances.iter().flat_map(|c| self.sources.watched_paths(c).into_iter().map(|p| (c.id.clone(), p))).collect();
        if want != self.watched_paths {
            self.watchers.truncate(1);
            self.watched_paths = want.clone();
            for (_, dir) in want {
                if let Some(mut w) = self.watcher(false) {
                    if w.watch(&dir, RecursiveMode::NonRecursive).is_ok() {
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
            let card = self.new_card();
            let mut host = AppHost::new(&self.opts.dir, None);
            self.ws.instances = default_instances(&self.monitors, &self.reg, card, &mut host);
            for l in host.into_logs() {
                self.log(l);
            }
            self.mark_save();
            self.log("first run: created the default widgets");
        }
        self.sync_windows(el);
        self.sync_watchers();
        let (p1, p2, p3) = (self.proxy.clone(), self.proxy.clone(), self.proxy.clone());
        let _ = p3;
        win32::watch_shell_events(move || {
            let _ = p1.send_event(UserEvent::ForegroundChanged);
        });
        if let Some(w) = self.wins.iter().find_map(|w| w.window.clone()) {
            win32::watch_display_changes(&w, move || {
                let _ = p2.send_event(UserEvent::DisplaysChanged);
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
                "reload" => self.reload(el),
                "folder" => self.apply(el, Cmd::OpenFolder),
                "quit" => el.exit(),
                _ => {}
            },
            UserEvent::Hotkey => self.set_edit(!self.edit),
            UserEvent::TrayClick => self.open_settings(el),
            UserEvent::FilesChanged => self.reload_at = Some(Instant::now() + Duration::from_millis(250)),
            UserEvent::ForegroundChanged => {
                // Explorer reorders a few ms after the event: check a few times, growing gaps
                let now = Instant::now();
                self.show_desktop_checks = [4u64, 20, 60, 140, 300, 700].iter().map(|ms| now + Duration::from_millis(*ms)).collect();
            }
            UserEvent::DisplaysChanged => self.display_at = Some(Instant::now() + Duration::from_millis(500)),
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
        if let Some(t) = self.opts.exit_after_secs {
            if now.duration_since(self.started).as_secs_f32() >= t {
                if self.save_at.is_some() {
                    let _ = self.ws.save(&self.opts.dir);
                }
                el.exit();
                return;
            }
        }
        while self.show_desktop_checks.first().is_some_and(|t| *t <= now) {
            self.show_desktop_checks.remove(0);
            self.check_show_desktop();
        }
        if self.display_at.is_some_and(|t| t <= now) {
            self.display_at = None;
            self.monitors = win32::monitors(el);
            self.log(format!("displays changed: {} monitor(s)", self.monitors.len()));
            self.sync_windows(el);
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
            self.reload(el);
        }
        if self.save_at.is_some_and(|t| t <= now) {
            self.save_at = None;
            if let Err(e) = self.ws.save(&self.opts.dir) {
                self.log(format!("could not save workspace: {e}"));
            }
        }

        self.selftest_tick(el, now);
        if let Some(why) = self.gpu_lost_reason.take() {
            self.recover_gpu(el, &why);
        } else if self.gpu.as_ref().is_some_and(|g| g.is_lost()) {
            self.recover_gpu(el, "device lost callback");
        }
        let mut wake: Option<Instant> = [self.save_at, self.reload_at, self.show_desktop_checks.first().copied(), self.display_at, self.selftest.as_ref().map(|t| t.at), self.opts.exit_after_secs.map(|t| self.started + Duration::from_secs_f32(t))]
            .into_iter()
            .flatten()
            .min();
        let soonest = |t: Instant, wake: &mut Option<Instant>| *wake = Some(wake.map_or(t, |w| w.min(t)));
        for iw in &mut self.wins {
            let Some(w) = &iw.window else { continue };
            if iw.needs_frame(now) {
                if !iw.requested {
                    iw.requested = true;
                    w.request_redraw();
                }
                continue;
            }
            if let Some(t) = iw.next_wake() {
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
