//! The Workspace: the saved arrangement of Instances (decision 24). App-written
//! JSON at `%APPDATA%\Wayfinder\workspace.json`; widget definitions are
//! read-only TOML the app never writes. Positions are anchored to a monitor
//! and stored relative to its work area (decision 14), never as virtual-desktop
//! pixels; a missing monitor parks its Instances instead of relocating them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::data::Shortcut;
use crate::theme::Selection;
use crate::value::Value;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MonitorRef {
    pub name: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct InstanceCfg {
    pub id: String,
    pub widget: String,
    pub monitor: MonitorRef,
    /// Offset from the monitor work area's top-left, logical px.
    pub x: f32,
    pub y: f32,
    /// Collapsed size, logical px.
    pub w: f32,
    pub h: f32,
    /// "desktop" | "bottom" | "normal" | "topmost"
    pub z: String,
    pub click_through: bool,
    pub params: BTreeMap<String, serde_json::Value>,
}

impl Default for InstanceCfg {
    fn default() -> Self {
        Self {
            id: String::new(),
            widget: String::new(),
            monitor: MonitorRef::default(),
            x: 60.0,
            y: 60.0,
            w: 200.0,
            h: 120.0,
            z: "desktop".into(),
            click_through: false,
            params: BTreeMap::new(),
        }
    }
}

impl InstanceCfg {
    pub fn params_map(&self) -> BTreeMap<String, Value> {
        self.params.iter().map(|(k, v)| (k.clone(), Value::from(v))).collect()
    }

    pub fn set_param(&mut self, name: &str, v: &Value) {
        self.params.insert(name.to_string(), v.into());
    }

    pub fn items(&self) -> Vec<Shortcut> {
        match self.params.get("items") {
            Some(serde_json::Value::Array(a)) => a.iter().filter_map(|v| Shortcut::from_value(&Value::from(v))).collect(),
            _ => Vec::new(),
        }
    }

    pub fn set_items(&mut self, items: &[Shortcut]) {
        let list = items.iter().map(|s| serde_json::Value::from(&s.to_value())).collect();
        self.params.insert("items".into(), serde_json::Value::Array(list));
    }

    pub fn folder(&self) -> String {
        self.params.get("folder").and_then(|v| v.as_str()).unwrap_or("").to_string()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    pub version: u32,
    /// "low" (default) | "high": see ADR-005. `high` selects the dedicated GPU but pins a CPU core on some AMD drivers.
    pub gpu: String,
    pub theme: Selection,
    /// Token overrides applied over the whole theme.
    pub overrides: BTreeMap<String, String>,
    /// Snap grid in logical px (0 = off).
    pub grid: f32,
    pub autostart: bool,
    /// Blur the desktop behind every widget.
    pub blur: bool,
    /// Draw widget borders (off = flat, outline-free look).
    pub outlines: bool,
    /// Drag the top strip of a widget to move it, outside Edit Mode.
    pub header_drag: bool,
    pub instances: Vec<InstanceCfg>,
}

impl Default for Workspace {
    fn default() -> Self {
        Self { version: 1, gpu: "low".into(), theme: Selection::default(), overrides: BTreeMap::new(), grid: 8.0, autostart: false, blur: false, outlines: true, header_drag: false, instances: Vec::new() }
    }
}

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Wayfinder")
}

impl Workspace {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("workspace.json")
    }

    /// A missing file is a normal first run; a corrupt one is kept aside, never overwritten.
    pub fn load(dir: &Path) -> (Workspace, Option<String>) {
        let p = Self::path(dir);
        let Ok(text) = std::fs::read_to_string(&p) else { return (Workspace::default(), None) };
        match serde_json::from_str::<Workspace>(&text) {
            Ok(w) => (w, None),
            Err(e) => {
                let bak = p.with_extension("json.broken");
                let _ = std::fs::rename(&p, &bak);
                (Workspace::default(), Some(format!("workspace.json was unreadable ({e}); moved to {}", bak.display())))
            }
        }
    }

    /// Write atomically: a crash mid-save never leaves half a file.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let p = Self::path(dir);
        let tmp = p.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
    }

    pub fn next_id(&self, widget: &str) -> String {
        (1..).map(|n| format!("{widget}-{n}")).find(|id| !self.instances.iter().any(|i| &i.id == id)).unwrap()
    }
}

// ---- monitors and anchoring ---------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct MonitorInfo {
    pub name: String,
    /// Physical px.
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub scale: f64,
    /// Work area (excludes the taskbar), physical px: x, y, w, h.
    pub work: (i32, i32, u32, u32),
}

impl MonitorInfo {
    pub fn reference(&self) -> MonitorRef {
        MonitorRef { name: self.name.clone(), width: self.w, height: self.h }
    }
}

/// Physical top-left for an Instance, or `None` when its monitor is absent (park it).
pub fn resolve(cfg: &InstanceCfg, monitors: &[MonitorInfo]) -> Option<(i32, i32)> {
    let m = monitors.iter().find(|m| m.name == cfg.monitor.name)?;
    Some((m.work.0 + (cfg.x as f64 * m.scale).round() as i32, m.work.1 + (cfg.y as f64 * m.scale).round() as i32))
}

/// The inverse: given where a window really is, what to store.
pub fn anchor(pos: (i32, i32), size: (u32, u32), monitors: &[MonitorInfo]) -> Option<(MonitorRef, f32, f32)> {
    let centre = (pos.0 + size.0 as i32 / 2, pos.1 + size.1 as i32 / 2);
    let dist = |m: &MonitorInfo| {
        let dx = (m.x - centre.0).max(centre.0 - (m.x + m.w as i32)).max(0) as i64;
        let dy = (m.y - centre.1).max(centre.1 - (m.y + m.h as i32)).max(0) as i64;
        dx * dx + dy * dy
    };
    let m = monitors.iter().min_by_key(|m| dist(m))?;
    Some((m.reference(), ((pos.0 - m.work.0) as f64 / m.scale) as f32, ((pos.1 - m.work.1) as f64 / m.scale) as f32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(name: &str, x: i32, scale: f64) -> MonitorInfo {
        MonitorInfo { name: name.into(), x, y: 0, w: 1920, h: 1080, scale, work: (x, 0, 1920, 1032) }
    }

    fn cfg(name: &str, x: f32, y: f32) -> InstanceCfg {
        InstanceCfg { monitor: MonitorRef { name: name.into(), width: 1920, height: 1080 }, x, y, ..Default::default() }
    }

    #[test]
    fn anchor_and_resolve_round_trip_across_monitors_and_dpi() {
        let mons = [mon("A", 0, 1.0), mon("B", 1920, 1.5)];
        for (pos, want) in [((100, 50), "A"), ((1920 + 300, 200), "B")] {
            let (m, x, y) = anchor(pos, (200, 100), &mons).unwrap();
            assert_eq!(m.name, want);
            let c = InstanceCfg { monitor: m, x, y, ..Default::default() };
            let back = resolve(&c, &mons).unwrap();
            assert!((back.0 - pos.0).abs() <= 1 && (back.1 - pos.1).abs() <= 1, "{pos:?} -> {back:?}");
        }
    }

    #[test]
    fn a_missing_monitor_parks_instead_of_relocating() {
        let one = [mon("A", 0, 1.0)];
        assert_eq!(resolve(&cfg("B", 40.0, 40.0), &one), None, "must not fall back to the primary monitor");
        assert_eq!(resolve(&cfg("A", 40.0, 40.0), &one), Some((40, 40)));
        // and it comes back in place when the monitor reappears
        let two = [mon("A", 0, 1.0), mon("B", 1920, 1.0)];
        assert_eq!(resolve(&cfg("B", 40.0, 40.0), &two), Some((1960, 40)));
    }

    #[test]
    fn anchor_picks_the_nearest_monitor_for_an_offscreen_window() {
        let mons = [mon("A", 0, 1.0), mon("B", 1920, 1.0)];
        let (m, _, _) = anchor((5000, 100), (100, 100), &mons).unwrap();
        assert_eq!(m.name, "B");
    }

    #[test]
    fn save_is_atomic_and_a_corrupt_file_is_kept_not_overwritten() {
        let dir = std::env::temp_dir().join(format!("wf-ws-{}", std::process::id()));
        let mut w = Workspace::default();
        w.instances.push(InstanceCfg { id: "clock-1".into(), widget: "clock".into(), ..Default::default() });
        w.save(&dir).unwrap();
        let (back, err) = Workspace::load(&dir);
        assert!(err.is_none());
        assert_eq!(back.instances.len(), 1);
        assert!(!Workspace::path(&dir).with_extension("json.tmp").exists(), "temp file must be renamed away");

        std::fs::write(Workspace::path(&dir), "{ not json").unwrap();
        let (fresh, err) = Workspace::load(&dir);
        assert!(err.unwrap().contains("unreadable") && fresh.instances.is_empty());
        assert!(dir.join("workspace.json.broken").exists(), "the user's file is preserved");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_and_missing_fields_do_not_break_loading() {
        let w: Workspace = serde_json::from_str(r#"{"instances":[{"id":"a","widget":"clock","future_field":1}],"other":true}"#).unwrap();
        assert_eq!((w.instances[0].w, w.gpu.as_str()), (200.0, "low"));
        assert_eq!(Workspace::default().next_id("clock"), "clock-1");
    }
}
