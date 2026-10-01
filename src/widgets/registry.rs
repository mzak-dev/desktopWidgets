use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{Drawer, TomlWidget, Widget};
use crate::format::{Base, WidgetDef};

const BUILTIN: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/builtin_widgets.rs"));

type WrapDefinition = fn(TomlWidget) -> Arc<dyn Widget>;

/// Wrapping whichever definition loaded (built-in or user copy) keeps the behaviour when a user restyles it.
const RUST_WIDGETS_ON_DEFINITIONS: &[(&str, WrapDefinition)] = &[("drawer", Drawer::wrap)];

/// A broken user file is an error card, never a fallback to the built-in (decision 14).
pub type Def = Result<Arc<dyn Widget>, String>;

#[derive(Default, Clone)]
pub struct Registry {
    pub defs: BTreeMap<String, Def>,
}

fn adapt(id: &str, def: WidgetDef) -> Arc<dyn Widget> {
    let toml = TomlWidget::new(def);
    match RUST_WIDGETS_ON_DEFINITIONS.iter().find(|(w, _)| *w == id) {
        Some((_, wrap)) => wrap(toml),
        None => Arc::new(toml),
    }
}

/// `*.toml` files directly in `dir`, sorted, with their Widget ids (file stems).
pub fn widget_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut paths: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "toml")).collect();
    paths.sort();
    paths.into_iter().filter_map(|p| Some((p.file_stem()?.to_str()?.to_string(), p))).collect()
}

impl Registry {
    pub fn builtin() -> Registry {
        let defs = BUILTIN.iter().map(|(id, src)| (id.to_string(), WidgetDef::parse(id, src).map(|d| adapt(id, d)))).collect();
        Registry { defs }
    }

    /// Built-ins, then the user's folder.
    pub fn load(user_dir: &Path) -> Registry {
        let mut r = Registry::builtin();
        r.load_dir(user_dir);
        r
    }

    /// Every Widget file in `dir` replaces the one with its id, even when it is broken
    /// (decision 14). Returns the ids it read. `./` paths in them stay inside `dir`'s parent,
    /// the content root.
    pub fn load_dir(&mut self, dir: &Path) -> Vec<String> {
        let base = Base { dir: dir.to_path_buf(), root: dir.parent().unwrap_or(dir).to_path_buf() };
        let mut ids = Vec::new();
        for (id, p) in widget_files(dir) {
            let def = std::fs::read_to_string(&p)
                .map_err(|e| e.to_string())
                .and_then(|s| WidgetDef::parse(&id, &s))
                .map(|d| adapt(&id, WidgetDef { base: Some(base.clone()), ..d }))
                .map_err(|e| format!("{}: {e}", p.display()));
            self.defs.insert(id.clone(), def);
            ids.push(id);
        }
        ids
    }

    pub fn register(&mut self, w: Arc<dyn Widget>) {
        self.defs.insert(w.meta().id.clone(), Ok(w));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::Sys;
    use crate::theme::{Library, Selection, Theme};
    use crate::ui::{Kind, Node};
    use crate::value::Value;
    use crate::widgets::Inputs;

    #[test]
    fn every_file_in_assets_widgets_is_built_in_and_parses() {
        let files = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/widgets")).unwrap().filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|x| x == "toml")).count();
        let r = Registry::load(Path::new("no-such-dir"));
        assert_eq!((BUILTIN.len(), r.ids().len()), (files, files));
        assert!(r.errors().is_empty(), "built-in widget failed to parse: {:?}", r.errors());
    }

    #[test]
    fn system_monitor_builds_with_live_data_and_a_layout_hides_what_it_leaves_out() {
        let r = Registry::load(Path::new("no-such-dir"));
        let Some(Ok(w)) = r.get("system_monitor") else { panic!("system_monitor") };
        let theme = Theme::compose(&Library::load(Path::new("nope")), &Selection::default(), &[]);
        let sys = Sys::default();
        let arcs = |layout: crate::workspace::Layout| {
            let (params, st) = (BTreeMap::new(), BTreeMap::new());
            let read = |n: &str| (n == "sys").then(|| sys.sample());
            let arrange = Some(crate::modules::Arrange { layout: &layout, tier: None, preview: false });
            let inp = Inputs { params: &params, state: &st, card_size: (700.0, 200.0), key_prefix: "t", read_source: &read, arrange };
            let b = w.build(&inp, &theme, &|_| None).unwrap();
            assert!(b.warnings.is_empty(), "{:?}", b.warnings);
            assert!(b.deps.contains("sys.gauges_all"));
            fn count(n: &Node) -> usize {
                usize::from(matches!(&n.kind, Kind::Shape(s) if s.name() == "arc")) + n.children.iter().map(count).sum::<usize>()
            }
            (count(&b.root), b.arrangement.unwrap())
        };
        let (all, a) = arcs(Default::default());
        assert!(all >= 3, "{all} gauges");
        assert_eq!(a.tier, "normal");
        let ids: Vec<String> = a.slots.iter().find(|s| s.name == "gauges").unwrap().modules.iter().map(|m| m.id.clone()).collect();
        assert!(ids.starts_with(&["gauge:cpu".into(), "gauge:ram".into()]), "{ids:?}");
        let rest = ids.iter().filter(|i| *i != "gauge:cpu").cloned().collect();
        let layout = crate::workspace::Layout::from([("normal".to_string(), BTreeMap::from([("gauges".to_string(), rest)]))]);
        let (fewer, a) = arcs(layout);
        assert_eq!(fewer, all - 1);
        assert!(a.hidden.iter().any(|m| m.id == "gauge:cpu"), "the tray offers it back");
    }

    #[test]
    fn every_when_in_a_builtin_is_an_expression() {
        // `when = "self.w > 9"` without braces is a non-empty string, so always true
        for (id, src) in BUILTIN {
            for (n, line) in src.lines().enumerate() {
                let t = line.trim_start();
                if let Some(v) = t.strip_prefix("when = \"") {
                    assert!(v.starts_with('{') && v.trim_end().ends_with("}\""), "{id}.toml:{}: `{t}` is not an expression", n + 1);
                }
            }
        }
    }

    #[test]
    fn a_photo_frame_wakes_for_its_next_slide_and_a_gallery_does_not() {
        let dir = std::env::temp_dir().join(format!("wf-frame-wake-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for n in ["a.jpg", "b.jpg"] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        let r = Registry::load(Path::new("no-such-dir"));
        let theme = Theme::compose(&Library::load(Path::new("nope")), &Selection::default(), &[]);
        let sources = crate::data::DataSources::builtin();
        let wake = |id: &str| {
            let Some(Ok(w)) = r.get(id) else { panic!("{id}") };
            let mut cfg = crate::workspace::InstanceCfg { id: format!("{id}-1"), widget: id.into(), ..Default::default() };
            cfg.params.insert("folder".into(), serde_json::Value::String(dir.to_string_lossy().into_owned()));
            let params = w.meta().effective_params(&cfg.params_map());
            let cx = crate::data::SourceCx { cfg: &cfg, params: &params, tm: crate::data::Tm { year: 2026, month: 10, day: 1, dow: 4, hour: 9, minute: 0, second: 30, ms: 0 }, icon_pack: "Default" };
            let read = |n: &str| sources.value(n, &cx);
            let st = BTreeMap::new();
            let inp = Inputs { params: &params, state: &st, card_size: w.meta().default_card_size, key_prefix: "t", read_source: &read, arrange: None };
            let b = w.build(&inp, &theme, &|_| None).unwrap();
            (b.deps.clone(), sources.next_wake(&b.deps, &cx))
        };
        let (deps, next) = wake("photo_frame");
        assert!(deps.contains("gallery.current.path"), "{deps:?}");
        assert_eq!(next, Some(std::time::Duration::from_millis(30_002)), "every 5 minutes by default: woken at the next minute to look");
        assert_eq!(wake("photo_gallery").1, None, "a gallery waits for its folder to change");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn media_photo_and_agent_widgets_build_at_every_size_full_or_empty() {
        let r = Registry::load(Path::new("no-such-dir"));
        let theme = Theme::compose(&Library::load(Path::new("nope")), &Selection::default(), &[]);
        let pic = |n: &str| Value::obj([("name", n.into()), ("path", format!("C:\\Photos\\{n}.jpg").into()), ("ext", "jpg".into()), ("size", 2_400_000.0.into()), ("modified_ms", 0.0.into()), ("date", "3 Mar 2024".into())]);
        let list = |v: &[f64]| Value::List(v.iter().map(|x| Value::Num(*x)).collect());
        let full = |name: &str| -> Option<Value> {
            Some(match name {
                "media" => Value::obj([("active", true.into()), ("title", "Song".into()), ("artist", "Band".into()), ("album", "LP".into()), ("source", "Spotify".into()), ("playing", true.into()), ("can_seek", true.into()), ("art", "file:C:\\cover.png".into()), ("position", 30.0.into()), ("duration", 200.0.into()), ("progress", 0.15.into()), ("clock", "0:30".into()), ("length", "3:20".into())]),
                "audio" => Value::obj([("bands", list(&[0.2, 1.0, 0.5, 0.0])), ("peaks", list(&[0.4, 1.0, 0.6, 0.1])), ("level", 0.7.into()), ("bass", 0.9.into()), ("wave", list(&[0.0, 1.0, -1.0, 0.3])), ("history", Value::List(vec![list(&[0.1, 0.5, 0.2, 0.0]), list(&[0.2, 1.0, 0.5, 0.0])])), ("active", true.into())]),
                "gallery" => Value::obj([("count", 2.into()), ("index", 1.into()), ("current", pic("b")), ("items", Value::List(vec![pic("a"), pic("b")])), ("folder", "C:\\Photos".into()), ("folder_name", "Photos".into()), ("truncated", true.into()), ("error", "".into())]),
                "calendar" => crate::data::calendar_value(&crate::data::Tm { year: 2026, month: 8, day: 31, dow: 1, hour: 9, minute: 0, second: 0, ms: 0 }, 6, &["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"].map(String::from)),
                "agents" => Value::obj([("items", Value::List(vec![Value::obj([("name", "app".into()), ("cwd", "C:\\dev\\app".into()), ("tool", "Claude Code".into()), ("state", "done".into()), ("label", "Done".into()), ("working", false.into()), ("done", true.into()), ("age", "5s".into())])])), ("count", 1.into()), ("working", 0.into()), ("provider", "all".into()), ("note", "".into())]),
                _ => return None,
            })
        };
        let empty = |name: &str| -> Option<Value> {
            Some(match name {
                "media" => Value::obj([("active", false.into()), ("title", "".into()), ("artist", "".into()), ("album", "".into()), ("source", "".into()), ("playing", false.into()), ("can_seek", false.into()), ("art", "".into()), ("position", 0.0.into()), ("duration", 0.0.into()), ("progress", 0.0.into()), ("clock", "".into()), ("length", "".into())]),
                "audio" => Value::obj([("bands", Value::List(vec![])), ("peaks", Value::List(vec![])), ("level", 0.0.into()), ("bass", 0.0.into()), ("wave", Value::List(vec![])), ("history", Value::List(vec![])), ("active", false.into())]),
                "gallery" => Value::obj([("count", 0.into()), ("index", (-1).into()), ("current", Value::Nil), ("items", Value::List(vec![])), ("folder", "".into()), ("folder_name", "".into()), ("truncated", false.into()), ("error", "".into())]),
                "agents" => Value::obj([("items", Value::List(vec![])), ("count", 0.into()), ("working", 0.into()), ("provider", "claude".into()), ("note", "".into())]),
                "calendar" => crate::data::calendar_value(&crate::data::Tm { year: 2021, month: 2, day: 1, dow: 1, hour: 9, minute: 0, second: 0, ms: 0 }, 0, &["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"].map(String::from)),
                _ => return None,
            })
        };
        let variants: &[(&str, &[(&str, Value)])] = &[
            ("media_controller", &[]),
            ("media_controller", &[("card", false.into()), ("show_art", false.into())]),
            ("audio_visualizer", &[]),
            ("audio_visualizer", &[("mode", "mirror".into())]),
            ("audio_visualizer", &[("mode", "line".into())]),
            ("audio_visualizer", &[("mode", "area".into())]),
            ("audio_visualizer", &[("mode", "bars3d".into()), ("depth", 16.into())]),
            ("audio_visualizer", &[("mode", "scope".into())]),
            ("audio_visualizer", &[("mode", "waterfall".into()), ("background", false.into())]),
            ("audio_visualizer", &[("mode", "level".into()), ("background", false.into())]),
            ("photo_gallery", &[]),
            ("photo_frame", &[("show_name", true.into()), ("card", true.into())]),
            ("gif_player", &[("folder", "C:\\Gifs".into())]),
            ("agent_status", &[("provider", "all".into())]),
            ("calendar", &[]),
            ("calendar", &[("week_numbers", false.into()), ("other_days", false.into())]),
        ];
        for (id, set) in variants {
            let Some(Ok(w)) = r.get(id) else { panic!("{id} is not built in") };
            let m = w.meta();
            let max = m.max_card_size.unwrap_or(m.default_card_size);
            // the corners, and either side of each size family's edge (100 and 260 px)
            let edges = [(170.0, 170.0), (340.0, 340.0), (259.0, 150.0), (261.0, 150.0), (200.0, 259.0), (200.0, 261.0), (300.0, 99.0), (300.0, 101.0), (240.0, 129.0), (600.0, 300.0)];
            let fit = |(w, h): (f32, f32)| (w.clamp(m.min_card_size.0, max.0), h.clamp(m.min_card_size.1, max.1));
            let sizes: Vec<(f32, f32)> = [m.min_card_size, m.default_card_size, (m.min_card_size.0, max.1), max].into_iter().chain(edges.into_iter().map(fit)).collect();
            let params: BTreeMap<String, Value> = set.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
            for read in [&full as &dyn Fn(&str) -> Option<Value>, &empty] {
                for states in [BTreeMap::new(), BTreeMap::from([("index".to_string(), Value::Num(1.0)), ("sliding".to_string(), Value::Bool(true))])] {
                    for &size in &sizes {
                        let inp = Inputs { params: &params, state: &states, card_size: size, key_prefix: "t", read_source: read, arrange: None };
                        let b = w.build(&inp, &theme, &|_| None).unwrap_or_else(|e| panic!("{id} {params:?} at {size:?}: {e}"));
                        assert!(b.warnings.is_empty(), "{id} {params:?} at {size:?}: {:?}", b.warnings);
                    }
                }
            }
        }
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
