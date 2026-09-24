use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{Drawer, TomlWidget, Widget};
use crate::format::WidgetDef;

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
    /// (decision 14). Returns the ids it read.
    pub fn load_dir(&mut self, dir: &Path) -> Vec<String> {
        let mut ids = Vec::new();
        for (id, p) in widget_files(dir) {
            let def = std::fs::read_to_string(&p)
                .map_err(|e| e.to_string())
                .and_then(|s| WidgetDef::parse(&id, &s))
                .map(|d| adapt(&id, d))
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
    fn system_monitor_builds_with_live_data_and_hides_what_is_off() {
        let r = Registry::load(Path::new("no-such-dir"));
        let Some(Ok(w)) = r.get("system_monitor") else { panic!("system_monitor") };
        let theme = Theme::compose(&Library::load(Path::new("nope")), &Selection::default(), &[]);
        let arcs = |params: BTreeMap<String, Value>| {
            let st = BTreeMap::new();
            let sys = Sys::default();
            let read = |n: &str| (n == "sys").then(|| sys.sample());
            let inp = Inputs { params: &params, state: &st, card_size: (300.0, 150.0), key_prefix: "t", read_source: &read };
            let b = w.build(&inp, &theme, &|_| None).unwrap();
            assert!(b.warnings.is_empty(), "{:?}", b.warnings);
            assert!(b.deps.contains("sys.gauges"));
            fn count(n: &Node) -> usize {
                usize::from(matches!(&n.kind, Kind::Shape(s) if s.name() == "arc")) + n.children.iter().map(count).sum::<usize>()
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
