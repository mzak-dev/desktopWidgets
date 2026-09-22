//! The Widget seam (CONTEXT.md: Widget), the registry, and the
//! build-and-layout pipeline shared by the running app and the offscreen
//! renderer.
//!
//! A Widget comes in two kinds, both adapters of the `Widget` trait:
//! - a **TOML Widget** (`TomlWidget`): a definition file, for simple Widgets.
//!   Adding a built-in one is dropping a file into `assets/widgets/`.
//! - a **Rust Widget**: code, for full Widgets that need behaviour a file
//!   cannot declare. It may draw its tree from a definition (the Drawer does)
//!   or build it with the `ui::Node` builder. Adding one is a module here and
//!   a line in `registry.rs`.

mod drawer;
mod meta;
mod registry;
mod toml_widget;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::anim::Anim;
use crate::card::Card;
use crate::data::{DataSources, SourceCx, Tm};
use crate::format;
use crate::gfx::Gpu;
use crate::icons::IconService;
use crate::text::TextEngine;
use crate::theme::Theme;
use crate::ui::{self, Env, Frame, Node};
use crate::value::Value;
use crate::workspace::InstanceCfg;

pub use drawer::Drawer;
pub use meta::{ParamDef, ParamType, Seed, WidgetMeta};
pub use registry::{Def, Registry};
pub use toml_widget::TomlWidget;

/// A kind of thing that can be placed. Implemented by `TomlWidget` and by Rust Widgets.
pub trait Widget: Send + Sync {
    fn meta(&self) -> &WidgetMeta;

    /// The card tree of one Instance at one moment. The engine dresses it in
    /// its window (`card::Card::dress`), lays it out and draws it. `deps` must
    /// list every Data Source path it read (`expr::Scope::read` records them),
    /// or the Instance will not wake when they change.
    fn build(&self, inp: &Inputs, theme: &Theme, image_size: &dyn Fn(&str) -> Option<(f32, f32)>) -> Result<Built, String>;

    /// Once, when an Instance of this Widget is added, after its params are seeded.
    fn on_add(&self, _cfg: &mut InstanceCfg, _host: &mut dyn Host) {}

    /// A click action (`on_click = "verb arg"`). Return true when handled;
    /// otherwise the engine's own verbs (`launch`, `toggle`, `set`...) apply.
    fn action(&self, _verb: &str, _arg: &str, _cx: &mut ActionCx) -> bool {
        false
    }
}

/// What a Widget may ask of the engine outside a build.
pub trait Host {
    /// The data directory (`%APPDATA%\Wayfinder`).
    fn data_dir(&self) -> &Path;
    /// Ask the user for a file; `None` if they cancel.
    fn pick_file(&mut self) -> Option<PathBuf>;
    /// Create a `.lnk` in `dir` pointing at `target`.
    fn create_shortcut(&mut self, dir: &Path, target: &Path) -> bool;
    /// A line for the log (and the Settings log page).
    fn log(&mut self, msg: String);
}

/// Everything a Widget's `action` may read or change.
pub struct ActionCx<'a> {
    pub cfg: &'a InstanceCfg,
    pub state: &'a mut BTreeMap<String, Value>,
    pub host: &'a mut dyn Host,
    pub sources: &'a DataSources,
    /// Set to redraw the Instance afterwards.
    pub redraw: bool,
}

/// Prepare a new Instance of `w`: seed its params, then let the Widget set up.
pub fn setup(w: &dyn Widget, cfg: &mut InstanceCfg, host: &mut dyn Host) {
    w.meta().seed(cfg);
    w.on_add(cfg, host);
}

/// Everything a build reads besides the Widget itself.
pub struct Inputs<'a> {
    pub params: &'a BTreeMap<String, Value>,
    pub state: &'a BTreeMap<String, Value>,
    /// Card size, logical px (the window minus its gutter: see `card`).
    pub size: (f32, f32),
    /// Unique per Instance: prefixes every node key so text, hover and animation state never collide.
    pub key: &'a str,
    /// Data Sources by name, read lazily: only the ones a binding reads are evaluated.
    pub sources: &'a dyn Fn(&str) -> Option<Value>,
}

#[derive(Debug)]
pub struct Built {
    /// The card, sized to `Inputs::size`.
    pub root: Node,
    /// Dotted paths the build read: the scheduler's dependency set.
    pub deps: BTreeSet<String>,
    /// Image ids the tree uses, so the app can make sure they are uploaded.
    pub images: BTreeSet<String>,
    pub warnings: Vec<String>,
    pub expand: Option<ExpandInfo>,
}

/// A size the Instance asks its window to grow to (an open folder), in card
/// units from a Widget and window units once `Card::expand_window` adds the gutter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExpandInfo {
    pub active: bool,
    pub width: Option<f32>,
    pub height: Option<f32>,
}

pub struct Prepared {
    pub frame: Frame,
    pub deps: BTreeSet<String>,
    pub warnings: Vec<String>,
    pub expand: Option<ExpandInfo>,
    pub error: Option<String>,
    /// Icons were uploaded during this call (callers may want a second look).
    pub uploaded: bool,
}

/// Services one build needs; grouped so call sites stay readable.
pub struct Services<'a> {
    pub gpu: &'a mut Gpu,
    pub icons: &'a mut IconService,
    pub text: &'a mut TextEngine,
    pub anim: &'a mut Anim,
    pub sources: &'a DataSources,
}

/// Everything about one Instance at one moment.
pub struct View<'a> {
    pub cfg: &'a InstanceCfg,
    pub state: &'a BTreeMap<String, Value>,
    /// Window size, logical px.
    pub size: (f32, f32),
    pub theme: &'a Theme,
    pub pack: &'a str,
    pub tm: Tm,
    pub hover: Option<&'a str>,
    pub scale: f32,
    pub now: Instant,
    /// This Instance's card within its window (gutter, blur, outlines).
    pub card: Card,
}

pub fn prepare(def: &Def, v: &View, sv: &mut Services) -> Prepared {
    let card_size = v.card.card_size(v.size);
    let cx = SourceCx { cfg: v.cfg, tm: v.tm, icon_pack: v.pack };
    let sources = sv.sources;
    let read = |name: &str| sources.value(name, &cx);
    let mut uploaded = false;
    let mut result = None;
    for _ in 0..2 {
        let outcome = match def {
            Err(e) => Err(format!("{}: {e}", v.cfg.widget)),
            Ok(w) => {
                let mut params = v.cfg.params_map();
                if v.card.blur {
                    // a widget that styles blur itself sees the effective switch
                    params.insert("blur".into(), true.into());
                }
                let inp = Inputs { params: &params, state: v.state, size: card_size, key: &v.cfg.id, sources: &read };
                let gpu = &*sv.gpu;
                w.build(&inp, v.theme, &|id| gpu.image_size(id).map(|(w, h)| (w as f32, h as f32)))
            }
        };
        match outcome {
            Ok(b) => {
                let mut fresh = false;
                for id in &b.images {
                    fresh |= sv.icons.ensure(sv.gpu, id);
                }
                uploaded |= fresh;
                result = Some(Ok(b));
                if !fresh {
                    break; // sizes are known; no rebuild needed
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
            let styles_blur = def.as_ref().is_ok_and(|w| w.meta().styles_blur());
            let root = v.card.dress(&v.cfg.id, v.size, b.root, styles_blur);
            (root, b.deps, b.warnings, b.expand.map(|e| v.card.expand_window(e)), None)
        }
        Err(e) => (format::error_card(&e, v.size, v.theme), BTreeSet::new(), vec![], None, Some(e)),
    };
    let mut env = Env { text: sv.text, anim: sv.anim, hover: v.hover, now: v.now, scale: v.scale };
    let frame = ui::layout(&root, v.size, &mut env);
    Prepared { frame, deps, warnings, expand, error, uploaded }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;
    use crate::expr::Scope;
    use crate::theme::{Library, Selection};
    use std::sync::Arc;

    fn theme() -> Theme {
        Theme::compose(&Library::load(Path::new("nope")), &Selection::default(), &BTreeMap::new())
    }

    /// A pure Rust Widget: its tree from the `Node` builder, its deps recorded
    /// through a `Scope` over the Data Sources. Proves the Rust path of the seam.
    struct Badge(WidgetMeta);

    impl Widget for Badge {
        fn meta(&self) -> &WidgetMeta {
            &self.0
        }

        fn build(&self, inp: &Inputs, theme: &Theme, _: &dyn Fn(&str) -> Option<(f32, f32)>) -> Result<Built, String> {
            let scope = Scope::with_provider(inp.sources);
            let minute = scope.read("clock.minute")?;
            let root = Node::new(inp.key).wh(inp.size.0, inp.size.1).fill(theme.color("surface")).border(1.0, Color([1.0; 4])).child(Node::text(format!("{}/t", inp.key), format!("{minute}"), 12.0, theme.color("text")));
            Ok(Built { root, deps: scope.deps(), images: BTreeSet::new(), warnings: vec![], expand: Some(ExpandInfo { active: true, width: Some(300.0), height: None }) })
        }
    }

    #[test]
    fn a_rust_widget_builds_through_the_same_seam_and_gets_dressed() {
        let meta = WidgetMeta { id: "badge".into(), name: "Badge".into(), description: String::new(), size: (100.0, 40.0), min_size: (48.0, 48.0), params: vec![], state: BTreeMap::new() };
        let def: Def = Ok(Arc::new(Badge(meta)));
        let t = theme();
        let src = |n: &str| (n == "clock").then(|| Value::obj([("minute", 7.into())]));
        let Ok(w) = &def else { unreachable!() };
        let (st, params) = (BTreeMap::new(), BTreeMap::new());
        let inp = Inputs { params: &params, state: &st, size: (100.0, 40.0), key: "b-1", sources: &src };
        let b = w.build(&inp, &t, &|_| None).unwrap();
        assert!(b.deps.contains("clock.minute"), "{:?}", b.deps);

        // what `prepare` does around any Widget: dress the card, grow the expand size by the gutter
        let card = Card::new(&t, false, false);
        let window = card.dress("b-1", card.window_size((100.0, 40.0)), b.root, w.meta().styles_blur());
        assert_eq!((window.key.as_str(), window.children[0].look.border), ("b-1~", 0.0), "outlines off reaches Rust Widgets too");
        assert_eq!(b.expand.map(|e| card.expand_window(e).width), Some(Some(340.0)));
    }

    #[test]
    fn setup_seeds_params_before_the_widget_sets_up() {
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
        setup(&**list, &mut cfg, &mut Nobody);
        assert_eq!(cfg.items(), crate::data::starter_apps(), "`seed = \"starter-apps\"` fills the list once");
    }
}
