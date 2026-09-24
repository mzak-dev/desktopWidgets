//! A Widget is either a TOML Widget (a definition file) or a Rust Widget
//! (code); both implement `Widget`. Rust Widgets register in `registry.rs`.

mod drawer;
#[cfg(test)]
mod fits;
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
pub use meta::{Choice, ParamDef, ParamType, Seed, WidgetMeta, needs_message};
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
    w.meta().seed_params(cfg);
    w.on_instance_added(cfg, host);
}

pub struct Inputs<'a> {
    pub params: &'a BTreeMap<String, Value>,
    pub state: &'a BTreeMap<String, Value>,
    /// Logical px.
    pub card_size: (f32, f32),
    /// Unique per Instance, so text, hover and animation state never collide.
    pub key_prefix: &'a str,
    pub read_source: &'a dyn Fn(&str) -> Option<Value>,
}

#[derive(Debug)]
pub struct Built {
    pub root: Node,
    pub deps: BTreeSet<String>,
    pub image_ids: BTreeSet<String>,
    pub warnings: Vec<String>,
    pub expand: Option<ExpandInfo>,
}

/// In card units from a Widget, window units after `Card::expand_in_window_units`.
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
    pub icons_uploaded: bool,
}

pub struct Services<'a> {
    pub gpu: &'a mut Gpu,
    pub icons: &'a mut IconService,
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

pub fn prepare(def: &Def, v: &View, sv: &mut Services) -> Prepared {
    let card_size = v.card.card_size(v.window_size);
    let params = match def {
        Ok(w) => w.meta().effective_params(&v.cfg.params_map()),
        Err(_) => v.cfg.params_map(),
    };
    let cx = SourceCx { cfg: v.cfg, params: &params, tm: v.tm, icon_pack: v.icon_pack };
    let sources = sv.sources;
    let read = |name: &str| sources.value(name, &cx);
    let mut icons_uploaded = false;
    let mut result = None;
    for _ in 0..2 {
        let outcome = match def {
            Err(e) => Err(format!("{}: {e}", v.cfg.widget)),
            Ok(w) => {
                let unmet = w.meta().unmet(|n| sources.get(n).is_some());
                if unmet.is_empty() {
                    let inp = Inputs { params: &params, state: v.state, card_size, key_prefix: &v.cfg.id, read_source: &read };
                    let gpu = &*sv.gpu;
                    w.build(&inp, v.theme, &|id| gpu.image_size(id).map(|(w, h)| (w as f32, h as f32)))
                } else {
                    Err(format!("{}: {}", w.meta().name, needs_message(&unmet)))
                }
            }
        };
        match outcome {
            Ok(b) => {
                let mut fresh = false;
                for id in &b.image_ids {
                    fresh |= sv.icons.ensure(sv.gpu, id);
                }
                icons_uploaded |= fresh;
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
    Prepared { frame, deps, warnings, expand, error, icons_uploaded }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;
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
            Ok(Built { root, deps: scope.deps(), image_ids: BTreeSet::new(), warnings: vec![], expand: Some(ExpandInfo { active: true, width: Some(300.0), height: None }) })
        }
    }

    #[test]
    fn a_rust_widget_builds_through_the_same_seam_and_gets_a_window() {
        let meta = WidgetMeta { id: "badge".into(), name: "Badge".into(), description: String::new(), default_card_size: (100.0, 40.0), min_card_size: (48.0, 48.0), max_card_size: None, params: vec![], initial_state: BTreeMap::new(), needs: vec![] };
        let def: Def = Ok(Arc::new(Badge(meta)));
        let t = theme();
        let src = |n: &str| (n == "clock").then(|| Value::obj([("minute", 7.into())]));
        let Ok(w) = &def else { unreachable!() };
        let (st, params) = (BTreeMap::new(), BTreeMap::new());
        let inp = Inputs { params: &params, state: &st, card_size: (100.0, 40.0), key_prefix: "b-1", read_source: &src };
        let b = w.build(&inp, &t, &|_| None).unwrap();
        assert!(b.deps.contains("clock.minute"), "{:?}", b.deps);

        let flat = BTreeMap::from([("outlines".to_string(), Value::Bool(false))]);
        let card = Card::new(&Theme::compose(&Library::load(Path::new("nope")), &Selection::default(), &[&flat]));
        let window = card.window_node("b-1", card.window_size((100.0, 40.0)), b.root);
        assert_eq!((window.key.as_str(), window.children[0].look.border), ("b-1~", 0.0), "outlines off reaches Rust Widgets too");
        assert_eq!(b.expand.map(|e| card.expand_in_window_units(e).width), Some(Some(340.0)));
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
