//! A Plugin's saved data (ADR-0008): string keys and values, up to 1 MB, in
//! `<data>/plugin-data/<id>.json`. Outside the Plugin's folder, so upgrades keep it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const STORE_CAP: usize = 1 << 20;

#[derive(Default)]
struct Inner {
    map: BTreeMap<String, String>,
    bytes: usize,
    dirty: bool,
    purged: bool,
}

pub struct KvStore {
    path: PathBuf,
    inner: Mutex<Inner>,
}

impl KvStore {
    /// A missing file is empty; a corrupt one is kept aside as `.json.broken`.
    pub fn open(path: &Path) -> KvStore {
        let map: BTreeMap<String, String> = match std::fs::read_to_string(path) {
            Err(_) => BTreeMap::new(),
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|_| {
                let _ = std::fs::rename(path, path.with_extension("json.broken"));
                BTreeMap::new()
            }),
        };
        let bytes = map.iter().map(|(k, v)| k.len() + v.len()).sum();
        KvStore { path: path.to_path_buf(), inner: Mutex::new(Inner { map, bytes, ..Default::default() }) }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.inner.lock().unwrap().map.get(key).cloned()
    }

    /// `None` deletes. Past the cap it is refused and the old value kept.
    pub fn set(&self, key: &str, value: Option<String>) -> Result<(), String> {
        let mut g = self.inner.lock().unwrap();
        let old = g.map.get(key).map_or(0, |v| key.len() + v.len());
        let new = value.as_ref().map_or(0, |v| key.len() + v.len());
        if g.bytes - old + new > STORE_CAP {
            return Err(format!("the plugin's saved data would pass {} KB", STORE_CAP >> 10));
        }
        match value {
            Some(v) => g.map.insert(key.to_string(), v),
            None => g.map.remove(key),
        };
        g.bytes = g.bytes - old + new;
        g.dirty = true;
        Ok(())
    }

    /// Writes (atomically) if anything changed since the last flush.
    pub fn flush(&self) -> Result<(), String> {
        let mut g = self.inner.lock().unwrap();
        if !g.dirty || g.purged {
            return Ok(());
        }
        let text = serde_json::to_string(&g.map).map_err(|e| e.to_string())?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &self.path)).map_err(|e| format!("{}: {e}", self.path.display()))?;
        g.dirty = false;
        Ok(())
    }

    /// The Plugin was removed: delete the file, and never write it again.
    pub fn purge(&self) {
        let mut g = self.inner.lock().unwrap();
        g.purged = true;
        g.map.clear();
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wf-kv-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("weather.json")
    }

    #[test]
    fn values_survive_a_reopen() {
        let p = tmp("reopen");
        let s = KvStore::open(&p);
        s.set("todo", Some("[\"milk\"]".into())).unwrap();
        s.set("gone", Some("x".into())).unwrap();
        s.set("gone", None).unwrap();
        s.flush().unwrap();
        let again = KvStore::open(&p);
        assert_eq!((again.get("todo").as_deref(), again.get("gone")), (Some("[\"milk\"]"), None));
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[test]
    fn a_set_past_1_mb_is_refused_and_the_old_value_kept() {
        let s = KvStore::open(&tmp("cap"));
        s.set("a", Some("x".repeat(STORE_CAP - 10))).unwrap();
        assert!(s.set("b", Some("y".repeat(20))).is_err());
        assert_eq!(s.get("b"), None);
        s.set("a", Some("small".into())).unwrap();
        s.set("b", Some("y".repeat(20))).unwrap();
    }

    #[test]
    fn flush_writes_only_when_dirty() {
        let p = tmp("dirty");
        let s = KvStore::open(&p);
        s.flush().unwrap();
        assert!(!p.exists(), "nothing to write");
        s.set("k", Some("v".into())).unwrap();
        s.flush().unwrap();
        std::fs::write(&p, "{\"k\":\"edited\"}").unwrap();
        s.flush().unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{\"k\":\"edited\"}", "clean: not rewritten");
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[test]
    fn a_purged_store_never_writes() {
        let p = tmp("purge");
        let s = KvStore::open(&p);
        s.set("k", Some("v".into())).unwrap();
        s.flush().unwrap();
        s.purge();
        assert!(!p.exists());
        s.set("k", Some("again".into())).unwrap();
        s.flush().unwrap();
        assert!(!p.exists(), "a dying worker's last flush must not bring it back");
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[test]
    fn a_corrupt_file_is_kept_aside() {
        let p = tmp("corrupt");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "not json").unwrap();
        let s = KvStore::open(&p);
        assert_eq!(s.get("k"), None);
        assert!(p.with_extension("json.broken").exists());
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }
}
