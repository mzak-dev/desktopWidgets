//! The headless render behind `--render-widget`: a Widget's frame built by the same path as
//! the desktop's (`widgets::prepare`), in a pinned environment.
//!
//! Hermetic by default: a fixed Ambient (research/18, `Pins`), no installed content, an empty
//! temporary data folder, no network, the software adapter. A `real` seam, `--installed` or a
//! hardware adapter opt out of one piece each, and the record beside the PNG
//! (`<png>.env.json`, see `env`) says which. The text is measured with the machine's fonts
//! (there are no bundled ones), so two renders are byte-identical on one machine and font set,
//! not across machines.
//!
//! Time is virtual: the first frame is drawn at T0 and the last at T0 + settle, and every
//! source reads that clock (`SourceCx::now`), so a playing track or a fading animation is
//! where the clock says, not where the machine's speed left it. In between the render blocks,
//! and never draws a half-loaded state: each Code Source the widget reads must have answered,
//! and the Image Store must have nothing pending. A timeout is an error.

mod env;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crate::ambient::{Ambient, Backdrop, FetchMode, IconMode, Pins, Seam, Tm};
use crate::anim::Anim;
use crate::card::Card;
use crate::code::runtime::Limits;
use crate::code::{CodeSources, Deps, WasmSource};
use crate::content::{Catalog, Root};
use crate::data::{DataSources, SourceCx};
use crate::gfx::{Gpu, Power};
use crate::icons::ImageStore;
use crate::plugins::{self, PluginStore};
use crate::text::TextEngine;
use crate::theme::{Selection, Theme};
use crate::value::Value;
use crate::widgets::{Services, View, prepare, widget_files};
use crate::workspace::{self, InstanceCfg};

/// One `--render-widget` run.
#[derive(Debug, PartialEq)]
pub struct Request {
    /// A widget id (built-in; with `installed`, a plugin's or the user's too) or a widget file.
    pub widget: String,
    pub png: PathBuf,
    /// The card's size in logical px; the widget's default size without it.
    pub size: Option<(f32, f32)>,
    pub params: Vec<(String, serde_json::Value)>,
    pub state: Vec<(String, serde_json::Value)>,
    /// `--time`: the time of day, on the pinned date (or today's with a real clock).
    pub time: Option<(u32, u32)>,
    /// How long to wait for each plugin code source to answer, in seconds.
    pub wait: f32,
    /// `--data`: read only with `installed`; otherwise a render uses an empty temporary folder.
    pub data: Option<PathBuf>,
    /// Look widgets up among the installed plugins and the user's own (not hermetic).
    pub installed: bool,
    pub gpu: String,
    /// The environment: `--env`, `--palette`, `--scale`, `--transparent`, `--now`, `--real`.
    pub pins: Pins,
}

impl Request {
    pub fn new(widget: String) -> Self {
        Self { widget, png: PathBuf::new(), size: None, params: vec![], state: vec![], time: None, wait: 5.0, data: None, installed: false, gpu: "software".into(), pins: Pins::default() }
    }
}

/// An empty folder made for one render and removed after it.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Scratch, String> {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!("wayfinder-render-{}-{nanos}", std::process::id()));
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
fn unanswered(sources: &DataSources, names: &[String], deps: &BTreeSet<String>, cx: &SourceCx) -> Vec<String> {
    let used: BTreeSet<&str> = deps.iter().map(|d| d.split('.').next().unwrap_or(d)).collect();
    let loading = |n: &String| matches!(sources.value(n, cx), Some(Value::Obj(o)) if matches!(o.get("loading"), Some(Value::Bool(true))));
    names.iter().filter(|n| used.contains(n.as_str())).filter(|n| loading(n)).cloned().collect()
}

/// The seams this render takes from the machine: the `real` Pins plus the modes that imply one.
fn real_seams(pins: &Pins, power: Power) -> BTreeSet<Seam> {
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
fn ambient_for(pins: &Pins, real: &BTreeSet<Seam>, t0: Instant, data: &Path) -> Result<Ambient, String> {
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

/// Renders one widget as the desktop would, in the pinned environment, and writes a PNG and
/// its `.env.json`. Returns whether the widget showed an error.
pub fn render(r: &Request) -> Result<bool, String> {
    let scratch = Scratch::new()?;
    let power = Power::parse(&r.gpu).map_err(|e| format!("--gpu: {e}"))?;
    let real = real_seams(&r.pins, power);
    let mut pins = r.pins.clone();
    let data: PathBuf = if r.installed { r.data.clone().unwrap_or_else(workspace::data_dir) } else { scratch.0.clone() };
    if !r.installed && r.data.is_some() {
        println!("notice: --data is not read by a hermetic render (installed widgets and plugins are left out, and an empty temporary folder is used); add --installed to read them from it");
    }
    if r.time.is_some() && !real.contains(&Seam::Clock) {
        if pins.now == Pins::default().now {
            println!("notice: --time sets the time of day on the pinned date (2026-01-15), not today's; use --now <ISO> for another date or --real clock for the machine's");
        }
        if let Some((hour, minute)) = r.time {
            pins.now = Tm { hour, minute, second: 0, ms: 0, ..pins.now };
        }
    }

    // content: built-in, the folder of a named widget file, and with `installed` the machine's
    let plugin_list = if r.installed { PluginStore::new(&data).list() } else { Vec::new() };
    let mut roots = plugins::roots(&plugin_list, &Default::default());
    if r.installed {
        roots.push(Root::user(&data));
    }
    let mut cat = Catalog::load(&roots);
    let file = Path::new(&r.widget);
    let mut content: Vec<env::ContentRoot> = roots.iter().map(|root| env::ContentRoot { kind: "installed", path: std::path::absolute(&root.dir).unwrap_or_else(|_| root.dir.clone()), hash: env::hash_files(&root.dir, &env::files_under(&root.dir)) }).collect();
    let id = if r.widget.ends_with(".toml") {
        let dir = file.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        cat.registry.load_dir(dir);
        let files: Vec<PathBuf> = widget_files(dir).into_iter().map(|(_, p)| p).collect();
        content.push(env::ContentRoot { kind: "widget-folder", path: std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf()), hash: env::hash_files(dir, &files) });
        file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    } else {
        r.widget.clone()
    };
    let missing = if r.installed { String::new() } else { " (installed widgets and plugins are read only with --installed)".into() };
    let def = cat.registry.get(&id).ok_or_else(|| format!("no widget `{id}`{missing}"))?.clone();
    for e in cat.registry.errors().into_iter().filter(|e| e.contains(&id)) {
        println!("warning: {e}");
    }

    let mut gpu = Gpu::new_headless(power)?;
    let adapter = gpu.adapter_report();
    if !adapter.software && !real.contains(&Seam::Gpu) {
        return Err(format!("refusing to render on {} ({}): a hermetic render uses the software adapter", adapter.name, adapter.device_type));
    }

    let t0 = Instant::now();
    let mut ambient = ambient_for(&pins, &real, t0, &data)?;
    let mut text = TextEngine::with_fonts((ambient.fonts)());
    for e in text.sync_fonts(&cat.font_files) {
        println!("warning: {e}");
    }
    let mut images = ImageStore::new(ambient.icons.clone(), cat.icon_packs.clone());
    images.set_cache(data.join(".cache").join("thumbs"));
    let sel = Selection { palette: pins.palette.clone(), ..Default::default() };
    let speed: BTreeMap<String, Value> = pins.anim.iter().map(|a| ("anim-speed".to_string(), Value::Str(a.clone()))).collect();
    let theme = Theme::compose(&cat.library, &sel, &[&speed]);
    let card = Card::new(&theme);

    let mut sources = DataSources::from(&ambient);
    let (news_tx, news) = mpsc::channel();
    let news_tx = Mutex::new(news_tx);
    let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        let _ = news_tx.lock().unwrap().send(());
    });
    sources.set_waker(notify.clone());
    let (specs, _) = plugins::code_specs(&plugin_list, &Default::default());
    let fetch = ambient.fetch.take();
    let home = if r.installed { std::env::var_os("USERPROFILE").map(PathBuf::from) } else { None };
    let places = crate::code::fs::Places { home, private: vec![data.clone()] };
    let mut code = CodeSources::default();
    code.sync(&mut sources, specs, |spec| WasmSource::start(spec, Deps { fetch: fetch.clone(), store: None, notify: notify.clone(), limits: Limits::default(), places: places.clone(), calendar: ambient.calendar.clone() }));
    let code_names: Vec<String> = code.status().into_iter().map(|(n, _)| n).collect();

    let meta = match &def {
        Ok(w) => Some(w.meta().clone()),
        Err(_) => None,
    };
    let card_size = r.size.or(meta.as_ref().map(|m| m.default_card_size)).unwrap_or((200.0, 120.0));
    let size = card.window_size(card_size);
    let mut cfg = InstanceCfg { id: format!("{id}-1"), widget: id.clone(), w: size.0, h: size.1, ..Default::default() };
    if let Some(m) = &meta {
        crate::widgets::seed_params(m, &mut cfg);
    }
    for (k, v) in &r.params {
        cfg.params.insert(k.clone(), v.clone());
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

    let settle = pins.settle;
    let mut anim = Anim::default();
    let frame = |at: Instant, text: &mut TextEngine, images: &mut ImageStore, anim: &mut Anim| {
        let v = View { cfg: &cfg, state: &state, window_size: size, theme: &theme, icon_pack: "Default", tm, hover: None, scale: pins.scale, now: at, card };
        let mut sv = Services { images, text, anim, sources: &sources };
        prepare(&def, &v, &mut sv)
    };
    let waiting = |at: Instant, deps: &BTreeSet<String>| {
        let cx = SourceCx::new(cfg.instance(), &params, tm, "Default").with_now(at);
        unanswered(&sources, &code_names, deps, &cx)
    };

    // enter animations start transparent: a frame at T0 starts them, the last one is at T0 + settle
    let mut p = frame(t0, &mut text, &mut images, &mut anim);
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
                return Err(format!("code source {} did not answer within {} s (--wait raises it)", names.join(", "), r.wait));
            };
            let _ = news.recv_timeout(left.min(Duration::from_millis(50)));
            sources.take_news();
            code.take_news();
        }
        // small copies of pictures are made off-thread
        let deadline = Instant::now() + Duration::from_secs(20);
        while images.pending() {
            if Instant::now() >= deadline {
                return Err("pictures were still being prepared after 20 s".into());
            }
            std::thread::sleep(Duration::from_millis(20));
            images.take_ready();
        }
        images.take_ready();
        p = frame(t0 + settle, &mut text, &mut images, &mut anim);
        if !images.pending() && waiting(t0 + settle, &p.deps).is_empty() {
            break;
        }
        if rounds == 3 {
            return Err("the widget did not settle: it was still loading after 3 rounds".into());
        }
    }
    for w in &p.warnings {
        println!("warning: {w}");
    }
    if let Some(e) = &p.error {
        println!("error: {e}");
    }

    let (pw, ph) = ((size.0 * pins.scale).round() as u32, (size.1 * pins.scale).round() as u32);
    gpu.apply(images.drain());
    let mut px = gpu.render_offscreen(pw, ph, &p.frame.list, &mut text, settle)?;
    // premultiplied: over a backdrop, or back to straight alpha for a transparent PNG
    for (i, c) in px.chunks_exact_mut(4).enumerate() {
        let a = c[3] as f32 / 255.0;
        if pins.transparent {
            if a > 0.0 {
                for k in 0..3 {
                    c[k] = (c[k] as f32 / a).min(255.0) as u8;
                }
            }
        } else {
            let (x, y) = ((i as u32 % pw) as f32 / pw as f32, (i as u32 / pw) as f32 / ph as f32);
            let bg = match pins.backdrop {
                Backdrop::Gradient => [30.0 + 70.0 * x, 60.0 + 50.0 * (1.0 - y), 120.0 + 60.0 * y],
                Backdrop::Solid(rgb) => rgb.map(f32::from),
            };
            for k in 0..3 {
                c[k] = (c[k] as f32 + bg[k] * (1.0 - a)).clamp(0.0, 255.0) as u8;
            }
            c[3] = 255;
        }
    }
    let img = image::RgbaImage::from_raw(pw, ph, px).ok_or("the renderer returned the wrong size")?;
    img.save(&r.png).map_err(|e| format!("{}: {e}", r.png.display()))?;

    let faces = text.font_faces();
    let facts = env::Facts { widget: &id, size: (pw, ph), pins: &pins, real: &real, installed: r.installed, roots: &content, faces: &faces, adapter: &adapter, deps: &p.deps, code_sources: &code_names, rounds };
    let because = env::leaks(&facts);
    let side = env::sidecar_path(&r.png);
    let json = serde_json::to_string_pretty(&env::sidecar(&facts)).map_err(|e| e.to_string())?;
    std::fs::write(&side, json + "\n").map_err(|e| format!("{}: {e}", side.display()))?;
    println!("rendered {id} at {:.0}x{:.0} to {}", card_size.0, card_size.1, r.png.display());
    if because.is_empty() {
        println!("hermetic render; environment recorded in {}", side.display());
    } else {
        println!("not hermetic ({}); environment recorded in {}", because.join("; "), side.display());
    }
    Ok(p.error.is_some())
}

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
