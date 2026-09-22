//! The `shortcuts` Data Source: an Instance's app shortcuts, either the
//! explicit list in its `items` param or the contents of the folder in its
//! `folder` param, mirrored live.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::{Cadence, DataSource, SourceCx};
use crate::value::Value;
use crate::workspace::InstanceCfg;

#[derive(Clone, Debug, PartialEq)]
pub struct Shortcut {
    pub name: String,
    pub target: String,
    /// Optional explicit icon image path.
    pub icon: String,
}

impl Shortcut {
    pub fn from_value(v: &Value) -> Option<Shortcut> {
        let s = |k: &str| v.get(k).map(|x| x.to_string()).unwrap_or_default();
        let target = s("target");
        if target.is_empty() {
            return None;
        }
        let name = if s("name").is_empty() { file_stem(&target) } else { s("name") };
        Some(Shortcut { name, target, icon: s("icon") })
    }

    pub fn to_value(&self) -> Value {
        Value::obj([("name", self.name.as_str().into()), ("target", self.target.as_str().into()), ("icon", self.icon.as_str().into())])
    }
}

pub fn file_stem(target: &str) -> String {
    let t = target.trim_end_matches(['\\', '/']);
    Path::new(t).file_stem().and_then(|s| s.to_str()).unwrap_or(t).to_string()
}

/// Entries of a folder as shortcuts (`.lnk`, `.exe`, `.url`, files), capped.
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

/// A few apps every Windows machine has, for a new list to start from.
pub fn starter_apps() -> Vec<Shortcut> {
    let win = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    [("Notepad", format!("{win}\\notepad.exe")), ("Calculator", format!("{win}\\System32\\calc.exe")), ("Explorer", format!("{win}\\explorer.exe")), ("Terminal", format!("{win}\\System32\\cmd.exe"))]
        .into_iter()
        .map(|(n, t)| Shortcut { name: n.into(), target: t, icon: String::new() })
        .collect()
}

/// Separates the parts of an image id; not a legal path character.
pub const ID_SEP: char = '\u{1f}';

/// `icon:<pack>SEP<target>SEP<explicit icon path>`: what the icon service resolves.
pub fn icon_id(pack: &str, s: &Shortcut) -> String {
    format!("icon:{pack}{ID_SEP}{}{ID_SEP}{}", s.target, s.icon)
}

/// The `shortcuts` value for a list of shortcuts.
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

/// Most entries a mirrored folder shows.
const FOLDER_CAP: usize = 96;

#[derive(Default)]
pub struct Shortcuts {
    /// Listings of mirrored folders, until a watched folder changes.
    folders: Mutex<HashMap<String, Vec<Shortcut>>>,
}

impl Shortcuts {
    /// The shortcuts an Instance shows: its mirrored folder, or else its own list.
    pub fn items_of(&self, cfg: &InstanceCfg) -> Vec<Shortcut> {
        let folder = cfg.folder();
        if folder.is_empty() {
            return cfg.items();
        }
        let mut cache = self.folders.lock().unwrap_or_else(|e| e.into_inner());
        cache.entry(folder).or_insert_with_key(|f| folder_items(f, FOLDER_CAP)).clone()
    }
}

impl DataSource for Shortcuts {
    fn name(&self) -> &'static str {
        "shortcuts"
    }

    fn value(&self, cx: &SourceCx) -> Value {
        shortcuts_value(&self.items_of(cx.cfg), cx.icon_pack)
    }

    /// Changes only on events (a param edit, a watched folder), never with time.
    fn cadence(&self, _field: &str) -> Option<Cadence> {
        None
    }

    fn watch(&self, cfg: &InstanceCfg) -> Vec<PathBuf> {
        let folder = cfg.folder();
        if folder.is_empty() { vec![] } else { vec![PathBuf::from(folder)] }
    }

    fn invalidate(&self) {
        self.folders.lock().unwrap_or_else(|e| e.into_inner()).clear();
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
        assert!(src.watch(&cfg).is_empty());

        cfg.params.insert("folder".into(), serde_json::Value::String(dir.to_string_lossy().into_owned()));
        assert_eq!(src.items_of(&cfg).iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["b"]);
        assert_eq!(src.watch(&cfg), vec![dir.clone()]);
        std::fs::write(dir.join("a.txt"), "").unwrap();
        assert_eq!(src.items_of(&cfg).len(), 1, "cached until the watcher says otherwise");
        src.invalidate();
        assert_eq!(src.items_of(&cfg).iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
