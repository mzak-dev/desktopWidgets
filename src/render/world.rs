//! The half of a render that needs no GPU: the content a run reads, the fixed Ambient, and
//! the settled frame (first frame at T0, blocking waits, last frame at T0 + settle). `render`
//! draws it; the scene runner dumps it. Nothing here creates a device.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use super::{Request, env};
use crate::ambient::{Ambient, FetchMode, IconMode, Pins, Seam, Tm};
use crate::anim::Anim;
use crate::card::Card;
use crate::code::runtime::Limits;
use crate::code::{CodeSources, Deps, WasmSource};
use crate::content::{Catalog, Root};
use crate::data::{DataSources, SourceCx};
use crate::gfx::Power;
use crate::icons::ImageStore;
use crate::meta::ParamType;
use crate::plugins::{self, Plugin, PluginStore};
use crate::text::TextEngine;
use crate::theme::{Selection, Theme};
use crate::value::Value;
use crate::widgets::{Def, Services, View, prepare_with, widget_files};
use crate::workspace::{self, InstanceCfg};

/// An empty folder made for one render and removed after it.
pub(super) struct Scratch(pub PathBuf);

impl Scratch {
    pub(super) fn new() -> Result<Scratch, String> {
        static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        // runs may share a process and a clock tick (a scene set on several threads)
        let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("wayfinder-render-{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(Scratch(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The Code Sources among `names` that `deps` reads and that have not answered for this
/// Instance yet (their value still says `loading`).
pub(super) fn unanswered(sources: &DataSources, names: &[String], deps: &BTreeSet<String>, cx: &SourceCx) -> Vec<String> {
    let used: BTreeSet<&str> = deps.iter().map(|d| d.split('.').next().unwrap_or(d)).collect();
    let loading = |n: &String| matches!(sources.value(n, cx), Some(Value::Obj(o)) if matches!(o.get("loading"), Some(Value::Bool(true))));
    names.iter().filter(|n| used.contains(n.as_str())).filter(|n| loading(n)).cloned().collect()
}

/// The seams this render takes from the machine: the `real` Pins plus the modes that imply one.
pub(super) fn real_seams(pins: &Pins, power: Power) -> BTreeSet<Seam> {
    let mut real = pins.real.clone();
    if pins.icons == IconMode::System {
        real.insert(Seam::Icons);
    }
    if pins.fetch == FetchMode::Real {
        real.insert(Seam::Fetch);
    }
    if power != Power::Software {
        real.insert(Seam::Gpu);
    }
    real
}

/// The fixed Ambient of `pins`, with the real machine's part for each seam in `real`. A
/// playing track starts at `t0`.
pub(super) fn ambient_for(pins: &Pins, real: &BTreeSet<Seam>, t0: Instant, data: &Path) -> Result<Ambient, String> {
    let mut a = Ambient::fixed_at(pins, t0);
    let machine = real.iter().any(|s| matches!(s, Seam::Clock | Seam::Sys | Seam::Media | Seam::Audio | Seam::Icons)).then(|| Ambient::windows(data));
    if let Some(w) = machine {
        for s in real {
            match s {
                Seam::Clock => a.calendar = w.calendar.clone(),
                Seam::Sys => a.sys = w.sys.clone(),
                Seam::Media => a.media = w.media.clone(),
                Seam::Audio => a.capture = w.capture.clone(),
                Seam::Icons => a.icons = w.icons.clone(),
                Seam::Fonts | Seam::Fetch | Seam::Gpu => {}
            }
        }
    }
    if real.contains(&Seam::Fetch) {
        let http = crate::platform::winhttp::WinHttp::new().map_err(|e| format!("the real network is not available: {e}"))?;
        a.fetch = Some(Arc::new(http));
    }
    Ok(a)
}

/// What a run reads: the widget it names and the content around it, with nothing installed
/// unless the request says so.
pub struct Content {
    /// The widget's id.
    pub id: String,
    pub def: Def,
    cat: Catalog,
    plugins: Vec<Plugin>,
    /// Folders read, each with a hash of its files (for the record beside the output).
    pub roots: Vec<env::ContentRoot>,
    /// Lines a run prints before its result: notices and warnings about the content.
    pub notes: Vec<String>,
    /// The machine's plugins and the user's content were read (`--installed`).
    pub installed: bool,
    data: PathBuf,
    _scratch: Option<Scratch>,
}

impl Content {
    /// Finds the widget of `r`: a built-in, one in the `plugin` folder or the folder of a
    /// named file, and with `installed` the machine's plugins and the user's own.
    pub fn load(r: &Request) -> Result<Content, String> {
        let scratch = if r.installed { None } else { Some(Scratch::new()?) };
        let data: PathBuf = match &scratch {
            Some(s) => s.0.clone(),
            None => r.data.clone().unwrap_or_else(workspace::data_dir),
        };
        let mut notes = Vec::new();
        if !r.installed && r.data.is_some() {
            notes.push("notice: --data is not read by a hermetic render (installed widgets and plugins are left out, and an empty temporary folder is used); add --installed to read them from it".to_string());
        }
        let mut plugin_list = if r.installed { PluginStore::new(&data).list() } else { Vec::new() };
        if let Some(dir) = &r.plugin {
            plugin_list.push(plugins::load_folder(dir)?);
        }
        let mut roots = plugins::roots(&plugin_list, &Default::default());
        if r.installed {
            roots.push(Root::user(&data));
        }
        let mut cat = Catalog::load(&roots);
        let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        let mut content: Vec<env::ContentRoot> = roots
            .iter()
            .map(|root| {
                let kind = if r.plugin.as_deref().is_some_and(|d| abs(d) == abs(&root.dir)) { "plugin-folder" } else { "installed" };
                env::ContentRoot { kind, path: abs(&root.dir), hash: env::hash_files(&root.dir, &env::files_under(&root.dir)) }
            })
            .collect();
        let file = Path::new(&r.widget);
        let id = if r.widget.ends_with(".toml") {
            let dir = file.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
            cat.registry.load_dir(dir);
            let files: Vec<PathBuf> = widget_files(dir).into_iter().map(|(_, p)| p).collect();
            content.push(env::ContentRoot { kind: "widget-folder", path: abs(dir), hash: env::hash_files(dir, &files) });
            file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
        } else {
            r.widget.clone()
        };
        let missing = if r.installed { String::new() } else { " (installed widgets and plugins are read only with --installed)".into() };
        let def = cat.registry.get(&id).ok_or_else(|| format!("no widget `{id}`{missing}"))?.clone();
        for e in cat.registry.errors().into_iter().filter(|e| e.contains(&id)) {
            notes.push(format!("warning: {e}"));
        }
        Ok(Content { id, def, cat, plugins: plugin_list, roots: content, notes, installed: r.installed, data, _scratch: scratch })
    }

    /// The widget's metadata; `None` when its definition does not load.
    pub fn meta(&self) -> Option<&crate::widgets::WidgetMeta> {
        self.def.as_ref().ok().map(|w| w.meta())
    }

    /// The card size of `r`: its `size`, else its named tier's, else the widget's default.
    pub fn card_size(&self, r: &Request) -> Result<(f32, f32), String> {
        if let Some(s) = r.size {
            return Ok(s);
        }
        if let (Some(name), Some(m)) = (&r.tier, self.meta()) {
            let t = m.tiers.iter().find(|t| &t.name == name).ok_or_else(|| {
                let known: Vec<&str> = m.tiers.iter().map(|t| t.name.as_str()).collect();
                format!("widget `{}` has no tier `{name}`{}", self.id, if known.is_empty() { " (it has none)".to_string() } else { format!(" (its tiers are {})", known.join(", ")) })
            })?;
            return Ok(t.size);
        }
        Ok(self.meta().map_or((200.0, 120.0), |m| m.default_card_size))
    }
}

/// Why a run gave no frame: the request is wrong (no such widget, tier or Module), or the run
/// went wrong (a code source never answered, pictures never finished).
#[derive(Debug, PartialEq)]
pub enum Failure {
    Bad(String),
    Run(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (Failure::Bad(e) | Failure::Run(e)) = self;
        f.write_str(e)
    }
}

impl From<Failure> for String {
    fn from(f: Failure) -> String {
        f.to_string()
    }
}

/// A widget's frame after it settled, in the pinned environment: everything a render needs
/// to draw it and a dump to describe it.
pub struct Settled {
    pub content: Content,
    pub card_size: (f32, f32),
    /// Logical px, the card and its gutter.
    pub window: (f32, f32),
    /// The effective Pins (`--time` applied).
    pub pins: Pins,
    pub real: BTreeSet<Seam>,
    /// `param=value` for each file or folder param the widget was given (`folder`, `path`):
    /// what it reads there is the machine's.
    pub paths: Vec<String>,
    pub prepared: crate::widgets::Prepared,
    pub text: TextEngine,
    pub images: ImageStore,
    pub code_sources: Vec<String>,
    /// Frames drawn after the first to settle.
    pub rounds: u32,
    /// The virtual time of the last frame, after T0.
    pub at: Duration,
    /// Notices and warnings, in the order a run prints them.
    pub notes: Vec<String>,
    /// The theme the frame was built with.
    pub theme: Theme,
}

/// Builds the widget's frame and waits for it to be whole: each Code Source it reads has
/// answered and the Image Store has nothing pending. `trace` records a `Placed` per node.
pub fn settle(r: &Request, power: Power, trace: bool) -> Result<Settled, Failure> {
    let content = Content::load(r).map_err(Failure::Bad)?;
    let real = real_seams(&r.pins, power);
    let mut pins = r.pins.clone();
    let mut notes = content.notes.clone();
    if r.time.is_some() && !real.contains(&Seam::Clock) {
        if pins.now == Pins::default().now {
            notes.push("notice: --time sets the time of day on the pinned date (2026-01-15), not today's; use --now <ISO> for another date or --real clock for the machine's".into());
        }
        if let Some((hour, minute)) = r.time {
            pins.now = Tm { hour, minute, second: 0, ms: 0, ..pins.now };
        }
    }
    let id = content.id.clone();
    let data = content.data.clone();

    let t0 = Instant::now();
    let mut ambient = ambient_for(&pins, &real, t0, &data).map_err(Failure::Run)?;
    // the render's own empty folder stands in for the profile, so no agent session of the machine shows
    if !r.installed {
        ambient.home = Some(data.clone());
    }
    let mut text = TextEngine::with_fonts((ambient.fonts)());
    for e in text.sync_fonts(&content.cat.font_files) {
        notes.push(format!("warning: {e}"));
    }
    let mut images = ImageStore::new(ambient.icons.clone(), content.cat.icon_packs.clone());
    images.set_cache(data.join(".cache").join("thumbs"));
    let sel = Selection { palette: pins.palette.clone(), ..Default::default() };
    let speed: BTreeMap<String, Value> = pins.anim.iter().map(|a| ("anim-speed".to_string(), Value::Str(a.clone()))).collect();
    let theme = Theme::compose(&content.cat.library, &sel, &[&speed]);
    let card = Card::new(&theme);

    let mut sources = DataSources::from(&ambient);
    let (news_tx, news) = mpsc::channel();
    let news_tx = Mutex::new(news_tx);
    let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        let _ = news_tx.lock().unwrap().send(());
    });
    sources.set_waker(notify.clone());
    let (specs, _) = plugins::code_specs(&content.plugins, &Default::default());
    let fetch = ambient.fetch.take();
    let home = if r.installed { std::env::var_os("USERPROFILE").map(PathBuf::from) } else { None };
    let places = crate::code::fs::Places { home, private: vec![data.clone()] };
    let mut code = CodeSources::default();
    code.sync(&mut sources, specs, |spec| WasmSource::start(spec, Deps { fetch: fetch.clone(), store: None, notify: notify.clone(), limits: Limits::default(), places: places.clone(), calendar: ambient.calendar.clone() }));
    let code_names: Vec<String> = code.status().into_iter().map(|(n, _)| n).collect();

    let def = content.def.clone();
    let meta = content.meta().cloned();
    let card_size = content.card_size(r).map_err(Failure::Bad)?;
    let window = card.window_size(card_size);
    let mut cfg = InstanceCfg { id: format!("{id}-1"), widget: id.clone(), w: window.0, h: window.1, ..Default::default() };
    if let Some(m) = &meta {
        crate::widgets::seed_params(m, &mut cfg);
    }
    for (k, v) in &r.params {
        cfg.params.insert(k.clone(), v.clone());
    }
    if let Some(items) = &r.items {
        cfg.set_items(items);
    }
    let mut state: BTreeMap<String, Value> = meta.as_ref().map(|m| m.initial_state.clone()).unwrap_or_default();
    state.extend(r.state.iter().map(|(k, v)| (k.clone(), Value::from(v))));
    let params = match &def {
        Ok(w) => w.meta().effective_params(&cfg.params_map()),
        Err(_) => cfg.params_map(),
    };
    // with a real clock `--time` edits the machine's time of day, as it always did
    let now_tm = ambient.calendar.now();
    let tm = match r.time {
        Some((hour, minute)) if real.contains(&Seam::Clock) => Tm { hour, minute, second: 0, ms: 0, ..now_tm },
        _ => now_tm,
    };

    // the files and folders the widget was pointed at: they are the machine's, not the run's
    let mut paths: Vec<String> = meta.iter().flat_map(|m| &m.params).filter(|p| matches!(p.ty, ParamType::Path | ParamType::File)).filter_map(|p| params.get(&p.name).map(|v| (p.name.clone(), v.to_string()))).filter(|(_, v)| !v.is_empty()).map(|(k, v)| format!("{k}={v}")).collect();
    if let Some(f) = Some(cfg.instance().folder()).filter(|f| !f.is_empty()).map(|f| format!("folder={f}")).filter(|f| !paths.contains(f)) {
        paths.push(f);
    }

    let settle = pins.settle;
    let mut anim = Anim::default();

    // Modules put away: take them out of the tier the card size picks, as a user would
    if !r.hide.is_empty() {
        let (mut spare_images, mut spare_anim) = (ImageStore::default(), Anim::default());
        let v = View { cfg: &cfg, state: &state, window_size: window, theme: &theme, icon_pack: "Default", tm, hover: None, scale: pins.scale, now: t0, card };
        let probe = prepare_with(&def, &v, &mut Services { images: &mut spare_images, text: &mut text, anim: &mut spare_anim, sources: &sources }, false);
        let arrangement = probe.arrangement.ok_or_else(|| Failure::Bad(format!("hide: widget `{id}` has no Modules")))?;
        let known: Vec<String> = arrangement.slots.iter().flat_map(|s| s.modules.iter().chain(&s.cut)).chain(&arrangement.hidden).map(|m| m.id.clone()).collect();
        if let Some(bad) = r.hide.iter().find(|h| !known.contains(h)) {
            let pool: Vec<&str> = known.iter().map(String::as_str).collect();
            return Err(Failure::Bad(format!("hide: `{bad}` is not a Module of `{id}` at tier `{}`{}", arrangement.tier, crate::suggest::suggest(bad, &[&pool]))));
        }
        let mut layout = arrangement.as_layout();
        layout.values_mut().for_each(|ids| ids.retain(|m| !r.hide.contains(m)));
        cfg.layout.insert(arrangement.tier, layout);
    }

    let frame = |at: Instant, hover: Option<&str>, text: &mut TextEngine, images: &mut ImageStore, anim: &mut Anim| {
        let v = View { cfg: &cfg, state: &state, window_size: window, theme: &theme, icon_pack: "Default", tm, hover, scale: pins.scale, now: at, card };
        let mut sv = Services { images, text, anim, sources: &sources };
        prepare_with(&def, &v, &mut sv, trace)
    };
    let waiting = |at: Instant, deps: &BTreeSet<String>| {
        let cx = SourceCx::new(cfg.instance(), &params, tm, "Default").with_now(at);
        unanswered(&sources, &code_names, deps, &cx)
    };

    // enter animations start transparent: a frame at T0 starts them, the last one is at T0 + settle
    let mut p = frame(t0, r.hover.as_deref(), &mut text, &mut images, &mut anim);
    let mut rounds = 0;
    loop {
        rounds += 1;
        let deadline = Instant::now() + Duration::from_secs_f32(r.wait.max(0.0));
        loop {
            let names = waiting(t0 + settle, &p.deps);
            if names.is_empty() {
                break;
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Err(Failure::Run(format!("code source {} did not answer within {} s (--wait raises it)", names.join(", "), r.wait)));
            };
            let _ = news.recv_timeout(left.min(Duration::from_millis(50)));
            sources.take_news();
            code.take_news();
        }
        // small copies of pictures are made off-thread
        let deadline = Instant::now() + Duration::from_secs(20);
        while images.pending() {
            if Instant::now() >= deadline {
                return Err(Failure::Run("pictures were still being prepared after 20 s".into()));
            }
            std::thread::sleep(Duration::from_millis(20));
            images.take_ready();
        }
        images.take_ready();
        p = frame(t0 + settle, r.hover.as_deref(), &mut text, &mut images, &mut anim);
        if !images.pending() && waiting(t0 + settle, &p.deps).is_empty() {
            break;
        }
        if rounds == 3 {
            return Err(Failure::Run("the widget did not settle: it was still loading after 3 rounds".into()));
        }
    }
    let mut at = settle;
    // a point on the settled frame is hovered: the key under it, held until its transition is done
    if let (Some((x, y)), None) = (r.hover_at, &r.hover) {
        let key = p.frame.hit_at(x, y).map(|h| h.key.clone()).ok_or_else(|| Failure::Bad(format!("hover_at {x},{y}: nothing there takes a pointer (the settled frame has no hit region at that point)")))?;
        frame(t0 + settle, Some(&key), &mut text, &mut images, &mut anim);
        at = settle + HOVER_HOLD;
        p = frame(t0 + at, Some(&key), &mut text, &mut images, &mut anim);
    }
    for w in &p.warnings {
        notes.push(format!("warning: {w}"));
    }
    if let Some(e) = &p.error {
        notes.push(format!("error: {e}"));
    }
    Ok(Settled { content, card_size, window, pins, real, paths, prepared: p, text, images, code_sources: code_names, rounds, at, notes, theme })
}

/// How long a hovered node is held before the frame a scene describes, so its colour
/// transition has finished.
pub const HOVER_HOLD: Duration = Duration::from_secs(1);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{DataSource, InstanceRef};

    /// A source whose value says whether it has answered, as a Code Source's does.
    struct Fake(&'static str, bool);

    impl DataSource for Fake {
        fn name(&self) -> &str {
            self.0
        }

        fn value(&self, _: &SourceCx) -> Value {
            Value::obj([("loading", Value::Bool(self.1))])
        }

        fn cadence(&self, _: &str, _: &SourceCx) -> Option<crate::data::Cadence> {
            None
        }
    }

    #[test]
    fn a_code_source_is_waited_for_only_while_the_widget_reads_it_and_it_says_loading() {
        let mut sources = DataSources::new(vec![]);
        sources.register(Box::new(Fake("weather", true))).unwrap();
        sources.register(Box::new(Fake("quotes", false))).unwrap();
        sources.register(Box::new(Fake("unused", true))).unwrap();
        let (saved, params) = (BTreeMap::new(), BTreeMap::new());
        let cx = SourceCx::new(InstanceRef::new("w-1", &saved), &params, Pins::default().now, "Default");
        let names: Vec<String> = ["weather", "quotes", "unused"].map(String::from).to_vec();
        let deps = BTreeSet::from(["weather.temp".to_string(), "quotes.text".to_string(), "clock.minute".to_string()]);
        assert_eq!(unanswered(&sources, &names, &deps, &cx), ["weather"]);
        assert!(unanswered(&sources, &names, &BTreeSet::from(["clock.minute".to_string()]), &cx).is_empty());
    }

    #[test]
    fn the_seams_taken_from_the_machine_are_the_real_pins_and_the_modes_that_imply_one() {
        let mut p = Pins::default();
        assert!(real_seams(&p, Power::Software).is_empty());
        p.set("icons", &serde_json::json!("system")).unwrap();
        p.set("fetch", &serde_json::json!("real")).unwrap();
        p.set("real", &serde_json::json!("clock")).unwrap();
        assert_eq!(real_seams(&p, Power::Low), BTreeSet::from([Seam::Clock, Seam::Icons, Seam::Fetch, Seam::Gpu]));
    }

    #[test]
    fn the_fixed_ambient_takes_a_real_seam_only_where_asked() {
        let pins = Pins::default();
        let t0 = Instant::now();
        let a = ambient_for(&pins, &BTreeSet::new(), t0, Path::new("unused")).unwrap();
        assert_eq!(a.calendar.now(), pins.now);
        let w = ambient_for(&pins, &BTreeSet::from([Seam::Clock]), t0, Path::new("unused")).unwrap();
        assert_ne!(w.calendar.now(), pins.now, "the machine's clock is not the pinned one");
        assert_eq!(w.media.track().title, a.media.track().title, "the rest stays pinned");
    }
}
