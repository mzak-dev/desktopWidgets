//! `workspace.json` (decision 24). Positions are relative to a monitor's work
//! area (decision 14); a missing monitor parks its Instances instead of moving them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::data::Shortcut;
use crate::theme::{Library, Selection, Theme, ThemePick};
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
    /// Resizing stops at the Widget's max card size; off lets it grow (never below the min).
    pub size_limit: bool,
    pub params: BTreeMap<String, serde_json::Value>,
    /// Its own palette, fonts, glyphs or icon pack; unset uses the global one.
    #[serde(skip_serializing_if = "ThemePick::is_empty")]
    pub theme: ThemePick,
    /// Its Style overrides, over the Workspace's.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub style: BTreeMap<String, serde_json::Value>,
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
            size_limit: true,
            params: BTreeMap::new(),
            theme: ThemePick::default(),
            style: BTreeMap::new(),
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
        self.set_shortcuts("items", items);
    }

    pub fn set_shortcuts(&mut self, name: &str, items: &[Shortcut]) {
        let list = items.iter().map(|s| serde_json::Value::from(&s.to_value())).collect();
        self.params.insert(name.into(), serde_json::Value::Array(list));
    }

    pub fn style_map(&self) -> BTreeMap<String, Value> {
        self.style.iter().map(|(k, v)| (k.clone(), Value::from(v))).collect()
    }

    pub fn folder(&self) -> String {
        self.params.get("folder").and_then(|v| v.as_str()).unwrap_or("").to_string()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    pub version: u32,
    /// "low" (default) | "high" | "software": ADR-005.
    pub gpu: String,
    pub theme: Selection,
    /// Style tokens (`assets/style.toml`) for every Instance; each may override them.
    pub style: BTreeMap<String, serde_json::Value>,
    /// Snap grid in logical px (0 = off).
    pub grid: f32,
    pub autostart: bool,
    pub header_drag: bool,
    pub instances: Vec<InstanceCfg>,
    /// Version 1 fields, folded into `style` by `migrate`; read, never written.
    #[serde(skip_serializing)]
    overrides: BTreeMap<String, String>,
    #[serde(skip_serializing)]
    blur: Option<bool>,
    #[serde(skip_serializing)]
    outlines: Option<bool>,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            version: 2,
            gpu: "low".into(),
            theme: Selection::default(),
            style: BTreeMap::new(),
            grid: 8.0,
            autostart: false,
            header_drag: false,
            instances: Vec::new(),
            overrides: BTreeMap::new(),
            blur: None,
            outlines: None,
        }
    }
}

/// Adding one: a variant, its `Workspace` field and a `settings::flag_row`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flag {
    HeaderDrag,
}

impl Flag {
    pub const ALL: [Flag; 1] = [Flag::HeaderDrag];

    pub fn id(self) -> &'static str {
        match self {
            Flag::HeaderDrag => "header_drag",
        }
    }

    pub fn parse(s: &str) -> Option<Flag> {
        Flag::ALL.into_iter().find(|f| f.id() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            Flag::HeaderDrag => "Move by header",
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            Flag::HeaderDrag => "Drag the top strip of any widget to move it, without Edit layout",
        }
    }
}

/// Per built-in Widget, its old style params and the Style token each became.
const MOVED_TO_STYLE: &[(&str, &[(&str, &str)])] = &[
    ("drawer", &[("transparent", "transparent"), ("opacity", "bg-opacity"), ("blur", "blur"), ("tint", "tint")]),
    ("clock", &[("accent", "accent")]),
    ("digital_clock", &[("accent", "accent")]),
];

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
            Ok(mut w) => {
                if w.version < 2 {
                    w.migrate();
                }
                (w, None)
            }
            Err(e) => {
                let bak = p.with_extension("json.broken");
                let _ = std::fs::rename(&p, &bak);
                (Workspace::default(), Some(format!("workspace.json was unreadable ({e}); moved to {}", bak.display())))
            }
        }
    }

    /// Atomic: a crash mid-save never leaves half a file.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let p = Self::path(dir);
        let tmp = p.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
    }

    /// Version 1 kept style in `overrides`, two switches and some Widgets' params.
    fn migrate(&mut self) {
        let old = std::mem::take(&mut self.overrides);
        self.style.extend(old.into_iter().map(|(k, v)| (k, serde_json::Value::String(v))));
        if self.blur.take() == Some(true) {
            self.style.insert("blur".into(), true.into());
        }
        if self.outlines.take() == Some(false) {
            self.style.insert("outlines".into(), false.into());
        }
        for cfg in &mut self.instances {
            let moved = MOVED_TO_STYLE.iter().find(|(w, _)| *w == cfg.widget).map_or(&[][..], |(_, m)| *m);
            for (param, token) in moved {
                if let Some(v) = cfg.params.remove(*param) {
                    cfg.style.insert(token.to_string(), v);
                }
            }
        }
        self.version = 2;
    }

    pub fn style_map(&self) -> BTreeMap<String, Value> {
        self.style.iter().map(|(k, v)| (k.clone(), Value::from(v))).collect()
    }

    pub fn global_theme(&self, lib: &Library) -> Theme {
        Theme::compose(lib, &self.theme, &[&self.style_map()])
    }

    /// base < axes (its own pick, else global) < global style < its own style.
    pub fn theme_for(&self, lib: &Library, cfg: &InstanceCfg) -> Theme {
        Theme::compose(lib, &cfg.theme.resolve(&self.theme), &[&self.style_map(), &cfg.style_map()])
    }

    pub fn flag(&self, f: Flag) -> bool {
        match f {
            Flag::HeaderDrag => self.header_drag,
        }
    }

    pub fn set_flag(&mut self, f: Flag, on: bool) {
        match f {
            Flag::HeaderDrag => self.header_drag = on,
        }
    }

    pub fn next_id(&self, widget: &str) -> String {
        (1..).map(|n| format!("{widget}-{n}")).find(|id| !self.instances.iter().any(|i| &i.id == id)).unwrap()
    }
}


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

/// Physical top-left, or `None` when the monitor is absent (park it).
pub fn resolve(cfg: &InstanceCfg, monitors: &[MonitorInfo]) -> Option<(i32, i32)> {
    let m = monitors.iter().find(|m| m.name == cfg.monitor.name)?;
    Some((m.work.0 + (cfg.x as f64 * m.scale).round() as i32, m.work.1 + (cfg.y as f64 * m.scale).round() as i32))
}

/// The inverse of `resolve`.
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

    #[test]
    fn header_drag_is_the_one_flag_left_and_it_is_saved() {
        let mut w = Workspace::default();
        for f in Flag::ALL {
            assert_eq!(Flag::parse(f.id()), Some(f));
            w.set_flag(f, !w.flag(f));
        }
        assert!(w.header_drag);
        assert_eq!(serde_json::to_value(&w).unwrap()["header_drag"].as_bool(), Some(true));
    }

    #[test]
    fn a_version_1_file_migrates_style_and_never_writes_legacy_fields() {
        let old = r##"{"version":1,"overrides":{"accent":"#ff8800","radius-lg":"30"},"blur":true,"outlines":false,
          "instances":[{"id":"drawer-1","widget":"drawer","params":{"transparent":true,"opacity":40,"blur":true,"tint":false,"title":"Apps"}},
                       {"id":"clock-1","widget":"clock","params":{"accent":"#00ff00","ticks":false}},
                       {"id":"mine-1","widget":"mine","params":{"accent":"#123456"}}]}"##;
        let dir = std::env::temp_dir().join(format!("wf-migrate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(Workspace::path(&dir), old).unwrap();
        let (w, err) = Workspace::load(&dir);
        assert!(err.is_none());
        assert_eq!(w.version, 2);
        assert_eq!(w.style.get("accent"), Some(&serde_json::json!("#ff8800")));
        assert_eq!((w.style.get("blur"), w.style.get("outlines")), (Some(&serde_json::json!(true)), Some(&serde_json::json!(false))));
        let d = &w.instances[0];
        assert_eq!((d.style.get("bg-opacity"), d.style.get("tint")), (Some(&serde_json::json!(40)), Some(&serde_json::json!(false))));
        assert!(!d.params.contains_key("opacity") && d.params.contains_key("title"));
        assert_eq!(w.instances[1].style.get("accent"), Some(&serde_json::json!("#00ff00")));
        assert!(w.instances[1].params.contains_key("ticks") && !w.instances[1].params.contains_key("accent"));
        assert!(w.instances[2].style.is_empty() && w.instances[2].params.contains_key("accent"), "user widgets keep their own accent");
        w.save(&dir).unwrap();
        let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(Workspace::path(&dir)).unwrap()).unwrap();
        assert!(json.get("overrides").is_none() && json.get("blur").is_none() && json.get("outlines").is_none());
        assert!(json["instances"][2].get("style").is_none() && json["instances"][2].get("theme").is_none(), "empty maps stay out of the file");
        let (again, _) = Workspace::load(&dir);
        assert_eq!(again.style, w.style, "a version 2 file loads as saved");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn theme_for_layers_instance_over_global() {
        let lib = crate::theme::Library::load(Path::new("nope"));
        let mut w = Workspace::default();
        w.style.insert("accent".into(), serde_json::json!("#111111"));
        let mut c = InstanceCfg::default();
        assert_eq!(w.theme_for(&lib, &c).color("accent").to_hex(), "#111111");
        assert_eq!(w.global_theme(&lib).color("accent").to_hex(), "#111111");
        c.style.insert("accent".into(), serde_json::json!("#222222"));
        c.theme.palette = Some("Daylight".into());
        let t = w.theme_for(&lib, &c);
        assert_eq!((t.color("accent").to_hex(), t.color("text").to_hex()), ("#222222".to_string(), "#141a2a".to_string()));
    }
}
