//! An Instance's `items` param, or the live contents of its `folder` param.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use super::{Cadence, DataSource, SourceCx};
use crate::shortcut::{Shortcut, file_stem, icon_id};
use crate::value::Value;
use super::InstanceRef;

/// Dock actions never accept a target invented by a network/plugin source.
/// Only an exact member of this instance's explicit, user-pinned list is trusted.
pub fn dock_target(inst: InstanceRef, verb: &str, arg: &str) -> Option<String> {
    match verb {
        "shortcuts.open" => inst.items().into_iter().find(|s| s.target == arg && !arg.is_empty()).map(|s| s.target),
        "shortcuts.recycle_bin" if arg.is_empty() => Some("shell:RecycleBinFolder".into()),
        _ => None,
    }
}

pub fn folder_items(dir: &str, cap: usize) -> Vec<Shortcut> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<Shortcut> = rd
        .filter_map(|e| e.ok())
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.') && e.file_name().to_string_lossy() != "desktop.ini")
        .map(|e| {
            let p = e.path();
            Shortcut { name: file_stem(&p.to_string_lossy()), target: p.to_string_lossy().into_owned(), icon: String::new() }
        })
        .collect();
    v.sort_by_key(|s| s.name.to_lowercase());
    v.truncate(cap);
    v
}

pub fn starter_apps() -> Vec<Shortcut> {
    let win = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    [("Notepad", format!("{win}\\notepad.exe")), ("Calculator", format!("{win}\\System32\\calc.exe")), ("Explorer", format!("{win}\\explorer.exe")), ("Terminal", format!("{win}\\System32\\cmd.exe"))]
        .into_iter()
        .map(|(n, t)| Shortcut { name: n.into(), target: t, icon: String::new() })
        .collect()
}

pub fn shortcuts_value(items: &[Shortcut], pack: &str) -> Value {
    let list = items
        .iter()
        .map(|s| {
            let Value::Obj(mut m) = s.to_value() else { unreachable!() };
            m.insert("icon_id".into(), Value::Str(icon_id(pack, s)));
            Value::Obj(m)
        })
        .collect();
    Value::obj([("items", Value::List(list)), ("count", (items.len() as i32).into())])
}

const MAX_FOLDER_ENTRIES: usize = 96;

#[derive(Default)]
pub struct Shortcuts {
    folder_listings: Mutex<HashMap<String, Vec<Shortcut>>>,
}

impl Shortcuts {
    pub fn items_of(&self, inst: InstanceRef) -> Vec<Shortcut> {
        let folder = inst.folder();
        if folder.is_empty() {
            return inst.items();
        }
        let mut cache = self.folder_listings.lock().unwrap_or_else(|e| e.into_inner());
        cache.entry(folder).or_insert_with_key(|f| folder_items(f, MAX_FOLDER_ENTRIES)).clone()
    }
}

impl DataSource for Shortcuts {
    fn name(&self) -> &str {
        "shortcuts"
    }

    fn value(&self, cx: &SourceCx) -> Value {
        shortcuts_value(&self.items_of(cx.instance()), cx.icon_pack())
    }

    fn cadence(&self, _field: &str, _cx: &SourceCx) -> Option<Cadence> {
        None
    }

    fn watched_paths(&self, cx: &super::SourceCx) -> Vec<PathBuf> {
        let folder = cx.instance().folder();
        if folder.is_empty() { vec![] } else { vec![PathBuf::from(folder)] }
    }

    fn invalidate(&self) {
        self.folder_listings.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::InstanceCfg;

    #[test]
    fn dock_actions_only_open_explicit_pins_or_the_fixed_recycle_bin() {
        let mut cfg = InstanceCfg::default();
        cfg.set_items(&[Shortcut { name: "Editor".into(), target: "C:\\Apps\\Editor.exe".into(), icon: "".into() }]);
        assert_eq!(dock_target(cfg.instance(), "shortcuts.open", "C:\\Apps\\Editor.exe"), Some("C:\\Apps\\Editor.exe".into()));
        for arg in ["", "C:\\Apps\\Other.exe", "C:\\Apps\\Editor.exe --flag", "shell:AppsFolder", "https://untrusted.example"] {
            assert_eq!(dock_target(cfg.instance(), "shortcuts.open", arg), None);
        }
        assert_eq!(dock_target(cfg.instance(), "shortcuts.recycle_bin", ""), Some("shell:RecycleBinFolder".into()));
        assert_eq!(dock_target(cfg.instance(), "shortcuts.recycle_bin", "anything"), None);
        assert_eq!(dock_target(cfg.instance(), "shortcuts.empty_bin", ""), None);
    }

    #[test]
    fn shortcut_round_trip_and_name_fallback() {
        let s = Shortcut { name: "".into(), target: "C:\\Apps\\Foo Bar.exe".into(), icon: "".into() };
        let back = Shortcut::from_value(&s.to_value()).unwrap();
        assert_eq!(back.name, "Foo Bar");
        assert!(Shortcut::from_value(&Value::obj([("name", "x".into())])).is_none()); // no target
    }

    #[test]
    fn a_mirrored_folder_wins_is_watched_and_is_listed_again_after_invalidate() {
        let dir = std::env::temp_dir().join(format!("wf-shortcuts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("b.txt"), "").unwrap();
        let src = Shortcuts::default();
        let mut saved = std::collections::BTreeMap::new();
        saved.insert("items".to_string(), serde_json::Value::Array(starter_apps().iter().map(|s| serde_json::Value::from(&s.to_value())).collect()));
        let items = |saved: &std::collections::BTreeMap<String, serde_json::Value>| src.items_of(InstanceRef::new("", saved));
        assert_eq!(items(&saved).len(), 4, "no folder: the explicit list");
        let watched = |saved: &std::collections::BTreeMap<String, serde_json::Value>| {
            let params = std::collections::BTreeMap::new();
            src.watched_paths(&SourceCx::new(InstanceRef::new("", saved), &params, crate::data::Tm::new(2026, 1, 1, 4, 0, 0, 0, 0), "Default"))
        };
        assert!(watched(&saved).is_empty());

        saved.insert("folder".into(), serde_json::Value::String(dir.to_string_lossy().into_owned()));
        assert_eq!(items(&saved).iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["b"]);
        assert_eq!(watched(&saved), vec![dir.clone()]);
        std::fs::write(dir.join("a.txt"), "").unwrap();
        assert_eq!(items(&saved).len(), 1, "cached until the watcher says otherwise");
        src.invalidate();
        assert_eq!(items(&saved).iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
