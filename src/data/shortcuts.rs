//! An Instance's `items` param, or the live contents of its `folder` param.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use super::{Cadence, DataSource, SourceCx};
use crate::shortcut::{Shortcut, file_stem, icon_id};
use crate::value::Value;
use crate::workspace::InstanceCfg;

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
    pub fn items_of(&self, cfg: &InstanceCfg) -> Vec<Shortcut> {
        let folder = cfg.folder();
        if folder.is_empty() {
            return cfg.items();
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
        shortcuts_value(&self.items_of(cx.cfg), cx.icon_pack)
    }

    fn cadence(&self, _field: &str, _cx: &SourceCx) -> Option<Cadence> {
        None
    }

    fn watched_paths(&self, cx: &super::SourceCx) -> Vec<PathBuf> {
        let folder = cx.cfg.folder();
        if folder.is_empty() { vec![] } else { vec![PathBuf::from(folder)] }
    }

    fn invalidate(&self) {
        self.folder_listings.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut cfg = InstanceCfg::default();
        cfg.set_items(&starter_apps());
        assert_eq!(src.items_of(&cfg).len(), 4, "no folder: the explicit list");
        let watched = |cfg: &InstanceCfg| {
            let params = cfg.params_map();
            src.watched_paths(&SourceCx { cfg, params: &params, tm: crate::data::Tm { year: 2026, month: 1, day: 1, dow: 4, hour: 0, minute: 0, second: 0, ms: 0 }, icon_pack: "Default" })
        };
        assert!(watched(&cfg).is_empty());

        cfg.params.insert("folder".into(), serde_json::Value::String(dir.to_string_lossy().into_owned()));
        assert_eq!(src.items_of(&cfg).iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["b"]);
        assert_eq!(watched(&cfg), vec![dir.clone()]);
        std::fs::write(dir.join("a.txt"), "").unwrap();
        assert_eq!(src.items_of(&cfg).len(), 1, "cached until the watcher says otherwise");
        src.invalidate();
        assert_eq!(src.items_of(&cfg).iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
