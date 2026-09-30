//! A Widget is either a TOML Widget (a definition file) or a Rust Widget
//! (code); both implement `Widget`. Rust Widgets register in `registry.rs`.

mod drawer;
#[cfg(test)]
mod fits;
mod meta;
mod registry;
#[cfg(test)]
pub(crate) mod safety_net;
mod toml_widget;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::anim::Anim;
use crate::card::Card;
use crate::data::{DataSources, SourceCx, Tm};
use crate::format::{self, Arrange};
use crate::icons::ImageStore;
use crate::text::TextEngine;
use crate::theme::Theme;
use crate::ui::{self, Env, Frame};
use crate::value::Value;
use crate::workspace::InstanceCfg;

pub use crate::format::{Built, Inputs};
pub use crate::ui::ExpandInfo;
pub use drawer::Drawer;
pub use crate::meta::{Choice, ModuleMeta, ParamDef, ParamType, Seed, TierMeta, WidgetMeta, needs_message};
pub use meta::{apply_seed, migrate, seed_params};
pub use registry::{Def, Registry, widget_files};
pub use toml_widget::TomlWidget;

pub trait Widget: Send + Sync {
    fn meta(&self) -> &WidgetMeta;

    /// `deps` must list every Data Source path read (`expr::Scope::read`
    /// records them), or the Instance never wakes when they change.
    fn build(&self, inp: &Inputs, theme: &Theme, image_size: &dyn Fn(&str) -> Option<(f32, f32)>) -> Result<Built, String>;

    fn on_instance_added(&self, _cfg: &mut InstanceCfg, _host: &mut dyn Host) {}

    /// Returning false lets the engine's own verbs (`launch`, `toggle`, `set`...) handle it.
    fn handle_action(&self, _verb: &str, _arg: &str, _cx: &mut ActionCx) -> bool {
        false
    }
}

pub trait Host {
    fn data_dir(&self) -> &Path;
    fn pick_file(&mut self) -> Option<PathBuf>;
    fn pick_folder(&mut self) -> Option<PathBuf> {
        None
    }
    fn create_shortcut(&mut self, dir: &Path, target: &Path) -> bool;
    fn log(&mut self, msg: String);
}

pub struct ActionCx<'a> {
    pub cfg: &'a InstanceCfg,
    pub state: &'a mut BTreeMap<String, Value>,
    pub host: &'a mut dyn Host,
    pub sources: &'a DataSources,
    pub wants_redraw: bool,
}

pub fn set_up_instance(w: &dyn Widget, cfg: &mut InstanceCfg, host: &mut dyn Host) {
    seed_params(w.meta(), cfg);
    w.on_instance_added(cfg, host);
}

pub struct Prepared {
    pub frame: Frame,
    pub deps: BTreeSet<String>,
    pub warnings: Vec<String>,
    pub expand: Option<ExpandInfo>,
    pub error: Option<String>,
    /// A new image size became known: the queued uploads are in `Services.images`.
    pub images_changed: bool,
}

pub struct Services<'a> {
    pub images: &'a mut ImageStore,
    pub text: &'a mut TextEngine,
    pub anim: &'a mut Anim,
    pub sources: &'a DataSources,
}

pub struct View<'a> {
    pub cfg: &'a InstanceCfg,
    pub state: &'a BTreeMap<String, Value>,
    /// Logical px.
    pub window_size: (f32, f32),
    pub theme: &'a Theme,
    pub icon_pack: &'a str,
    pub tm: Tm,
    pub hover: Option<&'a str>,
    pub scale: f32,
    pub now: Instant,
    pub card: Card,
}

/// The one frame path: the desktop, `--render-widget` and the examples all build a Widget's
/// frame here. Sources are read at `v.now` (the caller's clock, real in the app, virtual in a
/// headless render) and animations run at the theme's `anim-speed`.
pub fn prepare(def: &Def, v: &View, sv: &mut Services) -> Prepared {
    sv.anim.duration_factor = crate::anim::duration_factor(&v.theme.str("anim-speed"));
    let card_size = v.card.card_size(v.window_size);
    let params = match def {
        Ok(w) => w.meta().effective_params(&v.cfg.params_map()),
        Err(_) => v.cfg.params_map(),
    };
    let cx = SourceCx::new(v.cfg.instance(), &params, v.tm, v.icon_pack).with_now(v.now);
    let sources = sv.sources;
    let read = |name: &str| sources.value(name, &cx);
    let mut images_changed = false;
    let mut result = None;
    for _ in 0..2 {
        let outcome = match def {
            Err(e) => Err(format!("{}: {e}", v.cfg.widget)),
            Ok(w) => {
                let unmet = w.meta().unmet(|n| sources.get(n).is_some());
                if unmet.is_empty() {
                    let inp = Inputs { params: &params, state: v.state, card_size, key_prefix: &v.cfg.id, read_source: &read, arrange: Some(Arrange { layout: &v.cfg.layout, tier: None, preview: false }) };
                    let images = &*sv.images;
                    w.build(&inp, v.theme, &|id| images.size(id).map(|(w, h)| (w as f32, h as f32)))
                } else {
                    Err(format!("{}: {}", w.meta().name, needs_message(&unmet)))
                }
            }
        };
        match outcome {
            Ok(b) => {
                let mut fresh = false;
                for id in &b.image_ids {
                    fresh |= sv.images.ensure(id);
                }
                images_changed |= fresh;
                result = Some(Ok(b));
                if !fresh {
                    break;
                }
            }
            Err(e) => {
                result = Some(Err(e));
                break;
            }
        }
    }
    let (root, deps, warnings, expand, error) = match result.expect("at least one build ran") {
        Ok(b) => {
            let root = v.card.window_node(&v.cfg.id, v.window_size, b.root);
            (root, b.deps, b.warnings, b.expand.map(|e| v.card.expand_in_window_units(e)), None)
        }
        Err(e) => (format::error_card(&e, v.window_size, v.theme), BTreeSet::new(), vec![], None, Some(e)),
    };
    let mut env = Env { text: sv.text, anim: sv.anim, hover: v.hover, now: v.now, scale: v.scale };
    let frame = ui::layout(&root, v.window_size, &mut env);
    Prepared { frame, deps, warnings, expand, error, images_changed }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;
    use crate::images::ImageOp;
    use crate::ui::Node;
    use crate::expr::Scope;
    use crate::theme::{Library, Selection};
    use std::sync::Arc;

    fn theme() -> Theme {
        Theme::compose(&Library::load(Path::new("nope")), &Selection::default(), &[])
    }

    struct Badge(WidgetMeta);

    impl Widget for Badge {
        fn meta(&self) -> &WidgetMeta {
            &self.0
        }

        fn build(&self, inp: &Inputs, theme: &Theme, _: &dyn Fn(&str) -> Option<(f32, f32)>) -> Result<Built, String> {
            let scope = Scope::with_provider(inp.read_source);
            let minute = scope.read("clock.minute")?;
            let root = Node::new(inp.key_prefix).wh(inp.card_size.0, inp.card_size.1).fill(theme.color("surface")).border(1.0, Color([1.0; 4])).child(Node::text(format!("{}/t", inp.key_prefix), format!("{minute}"), 12.0, theme.color("text")));
            Ok(Built { root, deps: scope.deps(), image_ids: BTreeSet::new(), warnings: vec![], expand: Some(ExpandInfo { active: true, width: Some(300.0), height: None }), arrangement: None })
        }
    }

    #[test]
    fn a_rust_widget_builds_through_the_same_seam_and_gets_a_window() {
        let meta = WidgetMeta { id: "badge".into(), name: "Badge".into(), description: String::new(), category: String::new(), icon: String::new(), default_card_size: (100.0, 40.0), min_card_size: (48.0, 48.0), max_card_size: None, params: vec![], initial_state: BTreeMap::new(), needs: vec![], tiers: vec![], slots: vec![], modules: vec![] };
        let def: Def = Ok(Arc::new(Badge(meta)));
        let t = theme();
        let src = |n: &str| (n == "clock").then(|| Value::obj([("minute", 7.into())]));
        let Ok(w) = &def else { unreachable!() };
        let (st, params) = (BTreeMap::new(), BTreeMap::new());
        let inp = Inputs { params: &params, state: &st, card_size: (100.0, 40.0), key_prefix: "b-1", read_source: &src, arrange: None };
        let b = w.build(&inp, &t, &|_| None).unwrap();
        assert!(b.deps.contains("clock.minute"), "{:?}", b.deps);

        let flat = BTreeMap::from([("outlines".to_string(), Value::Bool(false))]);
        let card = Card::new(&Theme::compose(&Library::load(Path::new("nope")), &Selection::default(), &[&flat]));
        let window = card.window_node("b-1", card.window_size((100.0, 40.0)), b.root);
        assert_eq!((window.key.as_str(), window.children[0].look.border), ("b-1~", 0.0), "outlines off reaches Rust Widgets too");
        assert_eq!(b.expand.map(|e| card.expand_in_window_units(e).width), Some(Some(340.0)));
    }

    /// A Widget with one `image` element, at the picture's real size once it is known.
    struct Picture {
        meta: WidgetMeta,
        id: String,
        /// What `image_size` said at each build.
        seen: std::sync::Mutex<Vec<Option<(f32, f32)>>>,
    }

    impl Widget for Picture {
        fn meta(&self) -> &WidgetMeta {
            &self.meta
        }

        fn build(&self, inp: &Inputs, _: &Theme, image_size: &dyn Fn(&str) -> Option<(f32, f32)>) -> Result<Built, String> {
            let size = image_size(&self.id);
            self.seen.lock().unwrap().push(size);
            let (w, h) = size.unwrap_or((32.0, 32.0));
            let mut img = Node::new(format!("{}/img", inp.key_prefix));
            img.kind = ui::Kind::Image(ui::ImageSpec { id: self.id.clone(), w, h, tint: None, play: true, frame: None, fit: ui::Fit::Contain, feather: 0.0, fade: 0, ready: size.is_some() });
            let root = Node::new(inp.key_prefix).wh(inp.card_size.0, inp.card_size.1).child(img);
            Ok(Built { root, deps: BTreeSet::new(), image_ids: BTreeSet::from([self.id.clone()]), warnings: vec![], expand: None, arrangement: None })
        }
    }

    fn picture(id: &str) -> (Arc<Picture>, Def) {
        let meta = WidgetMeta { id: "pic".into(), name: "Pic".into(), description: String::new(), category: String::new(), icon: String::new(), default_card_size: (200.0, 100.0), min_card_size: (48.0, 48.0), max_card_size: None, params: vec![], initial_state: BTreeMap::new(), needs: vec![], tiers: vec![], slots: vec![], modules: vec![] };
        let w = Arc::new(Picture { meta, id: id.into(), seen: Default::default() });
        (w.clone(), Ok(w))
    }

    /// Runs `prepare` with no `Gpu` anywhere: an `ImageStore`, a `TextEngine` and empty sources.
    fn prepare_pic(def: &Def, images: &mut ImageStore) -> Prepared {
        let (theme, cfg, state) = (theme(), InstanceCfg { id: "pic-1".into(), widget: "pic".into(), w: 200.0, h: 100.0, ..Default::default() }, BTreeMap::new());
        let card = Card::new(&theme);
        let v = View { cfg: &cfg, state: &state, window_size: (200.0, 100.0), theme: &theme, icon_pack: "Default", tm: Tm { year: 2026, month: 9, day: 23, dow: 3, hour: 10, minute: 8, second: 0, ms: 0 }, hover: None, scale: 1.0, now: Instant::now(), card };
        let (mut text, mut anim, sources) = (TextEngine::new(), Anim::default(), DataSources::new(vec![]));
        let mut sv = Services { images, text: &mut text, anim: &mut anim, sources: &sources };
        prepare(def, &v, &mut sv)
    }

    fn png(name: &str, w: u32, h: u32) -> String {
        let dir = std::env::temp_dir().join(format!("wf-prepare-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        image::RgbaImage::new(w, h).save(&path).unwrap();
        format!("file:{}", path.display())
    }

    #[test]
    fn prepare_builds_and_lays_out_a_widget_with_no_gpu() {
        let id = png("lay.png", 40, 30);
        let (w, def) = picture(&id);
        let mut images = ImageStore::default();
        let p = prepare_pic(&def, &mut images);
        assert!(p.error.is_none(), "{:?}", p.error);
        assert!(p.images_changed);
        let rect = p.frame.rects.iter().find(|(k, _)| k == "pic-1/img").map(|(_, r)| *r).expect("the image was laid out");
        assert_eq!(rect[2] * 30.0, rect[3] * 40.0, "at the picture's 4:3, not the 32x32 stand-in the first build used: {rect:?}");
        assert_eq!(p.frame.list.image_ids().collect::<Vec<_>>(), [id.as_str()]);
        assert_eq!(images.size(&id), Some((40, 30)));
        assert_eq!(w.seen.lock().unwrap().len(), 2);
    }

    #[test]
    fn a_widget_builds_twice_only_when_a_new_size_became_known() {
        let id = png("twice.png", 16, 8);
        let (w, def) = picture(&id);
        let mut images = ImageStore::default();
        prepare_pic(&def, &mut images);
        assert_eq!(*w.seen.lock().unwrap(), [None, Some((16.0, 8.0))], "the first pass has no size, ensure records it, the second lays out with it");
        let again = prepare_pic(&def, &mut images);
        assert!(!again.images_changed);
        assert_eq!(w.seen.lock().unwrap().len(), 3, "known already: one build");
        let ops = images.drain();
        assert!(matches!(ops.as_slice(), [ImageOp::Upload(i, d)] if *i == id && (d.w, d.h) == (16, 8)), "one upload, however many frames");
    }

    #[test]
    fn a_missing_picture_lays_out_as_the_generic_icon() {
        let (_, def) = picture("file:/no/such/picture.png");
        let mut images = ImageStore::default();
        let p = prepare_pic(&def, &mut images);
        assert_eq!(images.size("file:/no/such/picture.png"), Some((48, 48)));
        let rect = p.frame.rects.iter().find(|(k, _)| k == "pic-1/img").unwrap().1;
        assert!(rect[2] == rect[3] && rect[2] != 32.0, "{rect:?}");
    }

    /// A widget that notes what its `clock` source says, and a `clock` that says how far
    /// past `base` the caller's clock is.
    struct Probe(WidgetMeta, std::sync::Mutex<Vec<Value>>);

    impl Widget for Probe {
        fn meta(&self) -> &WidgetMeta {
            &self.0
        }

        fn build(&self, inp: &Inputs, _: &Theme, _: &dyn Fn(&str) -> Option<(f32, f32)>) -> Result<Built, String> {
            self.1.lock().unwrap().push((inp.read_source)("clock").unwrap_or_default());
            Ok(Built { root: Node::new(inp.key_prefix).wh(inp.card_size.0, inp.card_size.1), deps: BTreeSet::new(), image_ids: BTreeSet::new(), warnings: vec![], expand: None, arrangement: None })
        }
    }

    struct Since(Instant);

    impl crate::data::DataSource for Since {
        fn name(&self) -> &str {
            "clock"
        }

        fn value(&self, cx: &SourceCx) -> Value {
            Value::Num(cx.now().saturating_duration_since(self.0).as_secs_f64())
        }

        fn cadence(&self, _: &str, _: &SourceCx) -> Option<crate::data::Cadence> {
            None
        }
    }

    #[test]
    fn prepare_reads_sources_at_the_views_clock_and_animates_at_the_themes_speed() {
        let meta = WidgetMeta { id: "probe".into(), name: "Probe".into(), description: String::new(), category: String::new(), icon: String::new(), default_card_size: (100.0, 40.0), min_card_size: (48.0, 48.0), max_card_size: None, params: vec![], initial_state: BTreeMap::new(), needs: vec![], tiers: vec![], slots: vec![], modules: vec![] };
        let probe = Arc::new(Probe(meta, Default::default()));
        let def: Def = Ok(probe.clone());
        let base = Instant::now();
        let mut sources = DataSources::new(vec![]);
        sources.register(Box::new(Since(base))).unwrap();
        let cfg = InstanceCfg { id: "probe-1".into(), widget: "probe".into(), w: 100.0, h: 40.0, ..Default::default() };
        let (state, mut text, mut images) = (BTreeMap::new(), TextEngine::new(), ImageStore::default());
        let mut run = |speed: Option<&str>, at: Instant| {
            let layer: BTreeMap<String, Value> = speed.iter().map(|s| ("anim-speed".to_string(), Value::Str(s.to_string()))).collect();
            let theme = Theme::compose(&Library::load(Path::new("nope")), &Selection::default(), &[&layer]);
            let card = Card::new(&theme);
            let v = View { cfg: &cfg, state: &state, window_size: (100.0, 40.0), theme: &theme, icon_pack: "Default", tm: Tm { year: 2026, month: 1, day: 15, dow: 4, hour: 10, minute: 10, second: 30, ms: 0 }, hover: None, scale: 1.0, now: at, card };
            let mut anim = Anim::default();
            prepare(&def, &v, &mut Services { images: &mut images, text: &mut text, anim: &mut anim, sources: &sources });
            anim.duration_factor
        };
        assert_eq!(run(None, base + std::time::Duration::from_secs(7)), 1.0);
        assert_eq!(probe.1.lock().unwrap().last(), Some(&Value::Num(7.0)), "a source sees the clock the caller gave the frame");
        assert_eq!((run(Some("off"), base), run(Some("fast"), base), run(Some("relaxed"), base)), (0.0, 0.6, 1.6), "every caller of prepare honours the theme's anim-speed");
    }

    #[test]
    fn set_up_instance_seeds_params_before_the_widget_sets_up() {
        struct Nobody;
        impl Host for Nobody {
            fn data_dir(&self) -> &Path {
                Path::new("nope")
            }
            fn pick_file(&mut self) -> Option<PathBuf> {
                None
            }
            fn create_shortcut(&mut self, _: &Path, _: &Path) -> bool {
                false
            }
            fn log(&mut self, _: String) {}
        }
        let reg = Registry::load(Path::new("no-such-dir"));
        let Some(Ok(list)) = reg.get("icon_list") else { panic!("icon_list") };
        let mut cfg = InstanceCfg { id: "icon_list-1".into(), widget: "icon_list".into(), ..Default::default() };
        set_up_instance(&**list, &mut cfg, &mut Nobody);
        assert_eq!(cfg.items(), crate::data::starter_apps(), "`seed = \"starter-apps\"` fills the list once");
    }
}
