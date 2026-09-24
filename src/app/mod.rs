//! One OS window per Instance, redrawn only on a changed binding, an
//! animation or input (decision 16). Each concern is an `impl App` in a submodule.

mod commands;
mod desktop;
mod edit_mode;
mod explorer;
mod first_run;
mod host;
mod input;
mod instance;
mod render;
mod selftest;
mod tray;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
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
use crate::code::runtime::Limits;
use crate::code::store::KvStore;
use crate::code::{Deps, WasmSource};
use crate::content::{Catalog, Root};
use crate::data::{self, DataSources};
use crate::draw::DrawList;
use crate::edit::{self, Handle, Rect, Snap};
use crate::gfx::{Gpu, Power, RenderError, Target};
use crate::icons::IconService;
use crate::net::Fetch;
use crate::platform::win32::{self, ZMode};
use crate::plugins::{self, Plugin, PluginRow, PluginStore};
use crate::settings::{self, Cmd, Scope, SettingsWin};
use crate::text::TextEngine;
use crate::theme::{Library, Theme};
use crate::ui::{self, Env, Frame};
use crate::value::Value;
use crate::widgets::{self, ActionCx, Def, ExpandInfo, Host, Registry, Services, View};
use crate::workspace::{self, InstanceCfg, MonitorInfo, Workspace};

pub use self::explorer::install_from_explorer;

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
    /// A folder a Data Source watches changed.
    WatchedChanged(Vec<PathBuf>),
    ForegroundChanged,
    DisplaysChanged,
    /// A Data Source has new values, logs or status.
    SourceNews,
}

/// How to run Wayfinder. `Options::from_args()` reads the command line; an app built on
/// Wayfinder adds its own Data Sources to `extra_sources` and calls `run`.
pub struct Options {
    pub dir: PathBuf,
    pub selftest: bool,
    pub gpu_override: Option<String>,
    pub exit_after_secs: Option<f32>,
    /// Point `.wfplugin` files at this exe (not for throwaway `--data` runs).
    pub register_file_type: bool,
    /// Start in Edit Mode.
    pub edit: bool,
    /// Install this `.wfplugin` first (what double-clicking one runs).
    pub install: Option<PathBuf>,
    /// Data Sources beyond the built-ins, registered at startup.
    pub extra_sources: Vec<Box<dyn data::DataSource>>,
}

impl Default for Options {
    fn default() -> Self {
        Options { dir: workspace::data_dir(), selftest: false, gpu_override: None, exit_after_secs: None, register_file_type: true, edit: false, install: None, extra_sources: Vec::new() }
    }
}

impl Options {
    pub fn from_args() -> Options {
        Options::parse(&std::env::args().skip(1).collect::<Vec<_>>())
    }

    /// `--data <dir> --exit-after <sec> --gpu <mode> --edit --selftest --install <file>`.
    pub fn parse(args: &[String]) -> Options {
        let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned());
        let flag = |name: &str| args.iter().any(|a| a == name);
        let selftest = flag("--selftest");
        Options {
            dir: value("--data").map(PathBuf::from).unwrap_or_else(workspace::data_dir),
            selftest,
            gpu_override: value("--gpu"),
            exit_after_secs: value("--exit-after").and_then(|s| s.parse().ok()),
            register_file_type: value("--data").is_none() && !selftest,
            edit: flag("--edit"),
            install: value("--install").map(PathBuf::from),
            extra_sources: Vec::new(),
        }
    }
}

/// Runs Wayfinder until it quits: one copy per data folder, `--install` first, then the
/// tray, the widgets and the event loop.
pub fn run(mut opts: Options) {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::HSTRING;

    // One running copy per data directory: a second launch exits quietly.
    let key = format!("Wayfinder-{:x}", opts.dir.to_string_lossy().bytes().fold(5381u64, |h, b| h.wrapping_mul(33) ^ b as u64));
    let _mutex = unsafe { CreateMutexW(None, true, &HSTRING::from(key)) };
    let running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let installing = opts.install.take();
    if let Some(file) = &installing {
        let installed = install_from_explorer(&opts.dir, file, running);
        if running || !installed {
            return; // a running copy reloads by itself
        }
    } else if running {
        eprintln!("wayfinder: already running (data dir {})", opts.dir.display());
        return;
    }
    // COM for the file pickers; S_FALSE (already initialised) is fine.
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
    }
    let event_loop = winit::event_loop::EventLoop::<UserEvent>::with_user_event().build().expect("event loop");
    let proxy = event_loop.create_proxy();
    let edit = opts.edit;
    let mut app = App::new(proxy, opts);
    if edit {
        app.request_edit_on_start();
    }
    if installing.is_some() {
        app.request_settings_on_start("plugins");
    }
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("wayfinder: event loop error: {e}");
    }
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
    /// A Settings page to open once started (after `--install`).
    start_page: Option<String>,
    selftest: Option<SelfTest>,
    gpu_lost_reason: Option<String>,
    gpu_recoveries: Vec<Instant>,
    forced_software: bool,
    families: Vec<String>,
    plugins: Vec<Plugin>,
    plugin_rows: Vec<PluginRow>,
    /// The last install's outcome, shown on the Plugins page.
    plugin_note: String,
    /// Each code Plugin's saved data, shared by every generation of its Code Source.
    stores: BTreeMap<String, Arc<KvStore>>,
    /// The network for plugin code, opened the first time a Plugin lists hosts.
    fetch: Option<Arc<dyn Fetch>>,
}

impl App {
    pub fn new(proxy: EventLoopProxy<UserEvent>, mut opts: Options) -> App {
        let dir = opts.dir.clone();
        let extra_sources = std::mem::take(&mut opts.extra_sources);
        let mut sources = DataSources::builtin();
        let waker = Mutex::new(proxy.clone());
        sources.set_waker(Arc::new(move || {
            let _ = waker.lock().unwrap().send_event(UserEvent::SourceNews);
        }));
        let mut source_errors = Vec::new();
        for s in extra_sources {
            if let Err(e) = sources.register(s) {
                source_errors.push(e);
            }
        }
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::remove_file(dir.join("wayfinder.log")); // one log per run
        let guide_errors = write_missing_guides(&dir);
        PluginStore::new(&dir).sweep();
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
            sources,
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
            start_page: None,
            selftest: None,
            gpu_lost_reason: None,
            gpu_recoveries: Vec::new(),
            forced_software: false,
            families: Vec::new(),
            plugins: Vec::new(),
            plugin_rows: Vec::new(),
            plugin_note: String::new(),
            stores: BTreeMap::new(),
            fetch: None,
        };
        app.load_content();
        app.rebuild_theme();
        app.selftest = app.opts.selftest.then(|| SelfTest { step: 0, at: Instant::now() + Duration::from_millis(2200), checks: Vec::new(), rect: None, collapsed: None, configures0: 0, fake: None });
        if let Some(e) = ws_err {
            app.log(e);
        }
        for e in guide_errors.into_iter().chain(source_errors) {
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

    pub fn request_settings_on_start(&mut self, page: &str) {
        self.start_page = Some(page.to_string());
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
        self.retain_code();
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

    /// What Instance `i`'s sources see: its params with the Widget's defaults, the time and its icon pack.
    fn with_source_cx<R>(&self, i: usize, f: impl FnOnce(&data::SourceCx) -> R) -> R {
        let cfg = &self.ws.instances[i];
        let params = match self.reg.get(&cfg.widget) {
            Some(Ok(w)) => w.meta().effective_params(&cfg.params_map()),
            _ => cfg.params_map(),
        };
        let icon_pack = cfg.theme.resolve(&self.ws.theme).icon_pack;
        f(&data::SourceCx { cfg, params: &params, tm: data::now_local(), icon_pack: &icon_pack })
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
        self.plugin_rows = plugins::rows(&self.plugins, &self.ws.disabled_plugins, &cat);
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
        self.sync_code();
    }

    /// Starts the Code Sources of enabled Plugins and stops the rest; unchanged ones keep running.
    fn sync_code(&mut self) {
        let (specs, _) = plugins::code_specs(&self.plugins, &self.ws.disabled_plugins);
        if self.fetch.is_none() && specs.iter().any(|(_, s)| !s.hosts.is_empty()) {
            match crate::platform::winhttp::WinHttp::new() {
                Ok(w) => self.fetch = Some(Arc::new(w)),
                Err(e) => self.log(format!("plugins cannot use the network: {e}")),
            }
        }
        for (_, spec) in &specs {
            let path = plugins::data_file(&self.opts.dir, &spec.plugin);
            self.stores.entry(spec.plugin.clone()).or_insert_with(|| Arc::new(KvStore::open(&path)));
        }
        let proxy = Mutex::new(self.proxy.clone());
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = proxy.lock().unwrap().send_event(UserEvent::SourceNews);
        });
        let (fetch, stores) = (self.fetch.clone(), &self.stores);
        self.sources.sync_code(specs, |spec| {
            let deps = Deps { fetch: fetch.clone(), store: stores.get(&spec.plugin).cloned(), notify: notify.clone(), limits: Limits::default() };
            WasmSource::start(spec, deps)
        });
        self.refresh_code_status();
    }

    /// Copies each Code Source's status onto its Plugins page row.
    fn refresh_code_status(&mut self) {
        for (name, status) in self.sources.code_status() {
            for row in self.plugin_rows.iter_mut().filter(|r| r.code_source.as_deref() == Some(name.as_str())) {
                row.status = status.line();
            }
        }
    }

    fn take_source_news(&mut self) {
        let mut status = false;
        for (name, news) in self.sources.take_news() {
            for l in news.logs {
                self.log(format!("{name}: {l}"));
            }
            status |= news.status_changed;
            if news.all {
                for iw in self.wins.iter_mut().filter(|w| DataSources::reads(&w.deps, &name)) {
                    iw.redraw = true;
                }
            }
            for id in news.changed {
                if let Some(iw) = self.ws.instances.iter().position(|c| c.id == id).and_then(|i| self.wins.get_mut(i)) {
                    iw.redraw = true;
                }
            }
        }
        if status {
            self.refresh_code_status();
            let failing: Vec<String> = self.plugin_rows.iter().filter(|r| r.status.starts_with("Error") || r.status.starts_with("Cannot")).map(|r| format!("plugin {}: {}", r.id, r.status)).collect();
            for l in failing {
                self.log(l);
            }
            if let Some(s) = &mut self.settings {
                s.invalidate();
            }
        }
    }

    /// Code Sources keep values only for Instances on screen.
    fn retain_code(&self) {
        let live: BTreeSet<String> = self.ws.instances.iter().zip(&self.wins).filter(|(_, w)| w.window.is_some()).map(|(c, _)| c.id.clone()).collect();
        self.sources.retain(&live);
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
            if !only_assets {
                // a Data Source's own folder: only that source looks again
                let _ = proxy.send_event(UserEvent::WatchedChanged(ev.paths));
            } else if ev.paths.iter().any(|p| plugins::is_content_change(&data, p)) {
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
        let want: Vec<(String, PathBuf)> = (0..self.ws.instances.len()).flat_map(|i| self.with_source_cx(i, |cx| self.sources.watched_paths(cx)).into_iter().map(move |p| (i, p))).map(|(i, p)| (self.ws.instances[i].id.clone(), p)).collect();
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
        if let Some(page) = self.start_page.take() {
            self.open_settings(el);
            if let Some(s) = &mut self.settings {
                s.show_page(&page);
            }
        }
        if self.opts.register_file_type {
            match std::env::current_exe().map_err(|e| e.to_string()).and_then(|exe| win32::register_file_type(&exe)) {
                Ok(true) => self.log("double-clicking a .wfplugin file now installs it"),
                Ok(false) => {}
                Err(e) => self.log(format!("could not register .wfplugin files: {e}")),
            }
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
            UserEvent::WatchedChanged(paths) => {
                for p in &paths {
                    self.sources.path_changed(p);
                }
                self.redraw_all();
            }
            UserEvent::ForegroundChanged => {
                // Explorer reorders a few ms after the event: check a few times, growing gaps
                let now = Instant::now();
                self.show_desktop_checks = [4u64, 20, 60, 140, 300, 700].iter().map(|ms| now + Duration::from_millis(*ms)).collect();
            }
            UserEvent::DisplaysChanged => self.display_at = Some(Instant::now() + Duration::from_millis(500)),
            UserEvent::SourceNews => self.take_source_news(),
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, ev: WindowEvent) {
        if self.settings.as_ref().is_some_and(|s| s.window.id() == id) {
            let gpu_info = self.gpu.as_ref().map(|g| g.info.clone()).unwrap_or_else(|| "no GPU yet".into());
            let App { ws, reg, lib, theme, log, edit, settings, text, icons, gpu, families, wins, plugins: installed, plugin_rows, plugin_note, .. } = self;
            let off = plugins::hidden_instances(ws, reg, installed);
            let hidden: Vec<(String, settings::Hidden)> = ws.instances.iter().zip(wins.iter()).filter(|(_, w)| w.window.is_none()).map(|(c, _)| (c.id.clone(), off.get(&c.id).map_or(settings::Hidden::Parked, |p| settings::Hidden::PluginOff(p.clone())))).collect();
            let ctx = settings::Ctx { ws, reg, lib, theme, log, gpu_info: &gpu_info, fonts: families, edit: *edit, hidden: &hidden, plugins: plugin_rows, plugin_note };
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

#[cfg(test)]
mod options_tests {
    use super::*;

    #[test]
    fn options_come_from_the_command_line() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let o = Options::parse(&args(&["--data", "D:\\wf", "--gpu", "software", "--edit", "--install", "x.wfplugin", "--exit-after", "2.5"]));
        assert_eq!((o.dir, o.gpu_override.as_deref(), o.edit, o.install, o.exit_after_secs), (PathBuf::from("D:\\wf"), Some("software"), true, Some(PathBuf::from("x.wfplugin")), Some(2.5)));
        assert!(!o.register_file_type, "a throwaway --data run leaves the file association alone");
        let plain = Options::parse(&[]);
        assert!(plain.register_file_type && plain.extra_sources.is_empty() && !plain.selftest);
    }
}
