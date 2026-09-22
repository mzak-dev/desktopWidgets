//! The widget registry and the build-and-layout pipeline shared by the running
//! app and the offscreen renderer.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use crate::anim::Anim;
use crate::card::Card;
use crate::data::{self, Shortcut, Tm};
use crate::format::{self, ExpandInfo, Inputs, WidgetDef};
use crate::gfx::Gpu;
use crate::icons::IconService;
use crate::text::TextEngine;
use crate::theme::Theme;
use crate::ui::{self, Env, Frame};
use crate::value::Value;
use crate::workspace::InstanceCfg;

const BUILTIN: &[(&str, &str)] = &[
    ("clock", include_str!("../assets/widgets/clock.toml")),
    ("digital_clock", include_str!("../assets/widgets/digital_clock.toml")),
    ("icon_list", include_str!("../assets/widgets/icon_list.toml")),
    ("icon_folder", include_str!("../assets/widgets/icon_folder.toml")),
    ("drawer", include_str!("../assets/widgets/drawer.toml")),
    ("system_monitor", include_str!("../assets/widgets/system_monitor.toml")),
];

/// A definition or the reason it failed to load. A broken user file replacing a
/// built-in shows an error card; it never silently falls back (decision 14).
pub type Def = Result<Arc<WidgetDef>, String>;

#[derive(Default, Clone)]
pub struct Registry {
    pub defs: BTreeMap<String, Def>,
}

impl Registry {
    pub fn load(user_dir: &Path) -> Registry {
        let mut defs = BTreeMap::new();
        for (id, src) in BUILTIN {
            defs.insert(id.to_string(), WidgetDef::parse(id, src).map(Arc::new));
        }
        if let Ok(rd) = std::fs::read_dir(user_dir) {
            let mut paths: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "toml")).collect();
            paths.sort();
            for p in paths {
                let id = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                let def = std::fs::read_to_string(&p)
                    .map_err(|e| e.to_string())
                    .and_then(|s| WidgetDef::parse(&id, &s))
                    .map(Arc::new)
                    .map_err(|e| format!("{}: {e}", p.display()));
                defs.insert(id, def);
            }
        }
        Registry { defs }
    }

    pub fn get(&self, id: &str) -> Option<&Def> {
        self.defs.get(id)
    }

    pub fn ids(&self) -> Vec<String> {
        self.defs.keys().cloned().collect()
    }

    pub fn errors(&self) -> Vec<String> {
        self.defs.values().filter_map(|d| d.as_ref().err().cloned()).collect()
    }
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
}

/// Everything about one Instance at one moment.
pub struct View<'a> {
    pub cfg: &'a InstanceCfg,
    pub state: &'a BTreeMap<String, Value>,
    pub items: &'a [Shortcut],
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
    let mut uploaded = false;
    let mut result = None;
    for _ in 0..2 {
        let outcome = match def {
            Err(e) => Err(format!("{}: {e}", v.cfg.widget)),
            Ok(d) => {
                let mut params = v.cfg.params_map();
                if v.card.blur {
                    // a widget that styles blur itself sees the effective switch
                    params.insert("blur".into(), true.into());
                }
                let inp = Inputs {
                    params: &params,
                    state: v.state,
                    size: card_size,
                    key: &v.cfg.id,
                    clock: data::clock_value(&v.tm),
                    sys: data::sys_value(),
                    shortcuts: data::shortcuts_value(v.items, v.pack),
                };
                let gpu = &*sv.gpu;
                format::build(d, &inp, v.theme, &|id| gpu.image_size(id).map(|(w, h)| (w as f32, h as f32)))
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
            let styles_blur = matches!(def, Ok(d) if d.params.iter().any(|p| p.name == "blur"));
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

    #[test]
    fn all_builtin_widgets_parse() {
        let r = Registry::load(Path::new("no-such-dir"));
        assert_eq!(r.ids().len(), 6);
        assert!(r.errors().is_empty(), "built-in widget failed to parse: {:?}", r.errors());
    }


    #[test]
    fn system_monitor_builds_with_live_data_and_hides_what_is_off() {
        let def = WidgetDef::parse("system_monitor", include_str!("../assets/widgets/system_monitor.toml")).unwrap();
        let theme = Theme::compose(&crate::theme::Library::load(Path::new("nope")), &crate::theme::Selection::default(), &BTreeMap::new());
        let arcs = |params: BTreeMap<String, Value>| {
            let st = BTreeMap::new();
            let inp = Inputs { params: &params, state: &st, size: (300.0, 150.0), key: "t", clock: Value::default(), sys: data::sys_value(), shortcuts: Value::default() };
            let b = format::build(&def, &inp, &theme, &|_| None).unwrap();
            assert!(b.warnings.is_empty(), "{:?}", b.warnings);
            assert!(b.deps.contains("sys.gauges"));
            fn count(n: &crate::ui::Node) -> usize {
                usize::from(matches!(n.kind, crate::ui::Kind::Arc(_))) + n.children.iter().map(count).sum::<usize>()
            }
            count(&b.root)
        };
        let all = arcs(BTreeMap::new());
        assert!(all >= 3, "{all} gauges");
        let off = BTreeMap::from([("show_cpu".to_string(), Value::Bool(false))]);
        assert_eq!(arcs(off), all - 1);
    }
    #[test]
    fn a_broken_user_file_replaces_the_builtin_with_an_error_not_a_fallback() {
        let dir = std::env::temp_dir().join(format!("wf-widgets-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("clock.toml"), "[root]\ntype='text'\ncolour='#fff'").unwrap();
        let r = Registry::load(&dir);
        let e = r.get("clock").unwrap().as_ref().err().expect("must be an error");
        assert!(e.contains("colour") && e.contains("clock.toml"), "{e}");
        assert!(r.get("digital_clock").unwrap().is_ok(), "other widgets unaffected");
        std::fs::remove_dir_all(&dir).ok();
    }
}
