//! `workspace.json` (decision 24). Positions are relative to a monitor's work
//! area (decision 14); a missing monitor parks its Instances instead of moving them.
//!
//! The file holds several Workspaces. The one on screen lives in `Workspace`'s own
//! fields (`instances`, `theme`, `style`), so the rest of the app reads it as before; the
//! others are kept whole in `workspaces` until switched to, each with the rules (a virtual
//! desktop, a monitor setup) that bring it up by itself.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::data::Shortcut;
use crate::theme::{Library, Selection, Theme, ThemePick};
use crate::value::Value;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(default)]
pub struct MonitorRef {
    pub name: String,
    pub width: u32,
    pub height: u32,
}

/// Per tier, per slot, the Module ids an Instance's user arranged; a tier with no entry uses the Widget's default.
pub type Layout = BTreeMap<String, BTreeMap<String, Vec<String>>>;

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
    /// Where its user put the Widget's Modules.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub layout: Layout,
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
            layout: BTreeMap::new(),
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
/// The Workspace on screen, the settings every Workspace shares (GPU, startup, grid, plugins),
/// and the other saved Workspaces.
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
    /// Plugins switched off; every other installed Plugin loads.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub disabled_plugins: BTreeSet<String>,
    /// The first-run setup was finished or skipped. A file saved before it existed
    /// reads as done, so only a fresh install sees the setup.
    #[serde(default = "yes")]
    pub onboarded: bool,
    /// Every Workspace, in the user's order; `active` names the one on screen. Its entry
    /// keeps only its name and rules: its Instances and look are the fields above.
    pub workspaces: Vec<Saved>,
    pub active: String,
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
            disabled_plugins: BTreeSet::new(),
            onboarded: false,
            workspaces: vec![Saved { name: FIRST_WORKSPACE.into(), ..Default::default() }],
            active: FIRST_WORKSPACE.into(),
            overrides: BTreeMap::new(),
            blur: None,
            outlines: None,
        }
    }
}

fn yes() -> bool {
    true
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
            Flag::HeaderDrag => "Drag a widget's top strip to move it without Edit layout.",
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
                w.tidy_workspaces();
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

    /// Unused in every Workspace, so an Instance keeps its id when its Workspace is shown again.
    pub fn next_id(&self, widget: &str) -> String {
        let taken = |id: &str| self.instances.iter().chain(self.workspaces.iter().flat_map(|w| &w.instances)).any(|i| i.id == id);
        (1..).map(|n| format!("{widget}-{n}")).find(|id| !taken(id)).unwrap()
    }

    /// Only a fresh install gets the starter widgets, never a Workspace the user emptied.
    pub fn wants_starter_widgets(&self) -> bool {
        self.instances.is_empty() && !self.onboarded && self.workspaces.len() <= 1
    }

    /// A file from before Workspaces, or edited by hand: one entry per name, the active one
    /// among them, and nothing kept twice.
    fn tidy_workspaces(&mut self) {
        let mut seen = BTreeSet::new();
        self.workspaces.retain(|w| !w.name.trim().is_empty() && seen.insert(w.name.clone()));
        if self.workspaces.is_empty() {
            self.workspaces.push(Saved { name: FIRST_WORKSPACE.into(), ..Default::default() });
        }
        if !self.workspaces.iter().any(|w| w.name == self.active) {
            self.active = self.workspaces[0].name.clone();
        }
        let i = self.active_index();
        let a = &mut self.workspaces[i];
        (a.instances, a.style, a.theme) = (Vec::new(), BTreeMap::new(), None);
    }

    pub fn names(&self) -> Vec<String> {
        self.workspaces.iter().map(|w| w.name.clone()).collect()
    }

    pub fn active_index(&self) -> usize {
        self.workspaces.iter().position(|w| w.name == self.active).unwrap_or(0)
    }

    pub fn saved(&self, name: &str) -> Option<&Saved> {
        self.workspaces.iter().find(|w| w.name == name)
    }

    /// `base`, or `base 2`, `base 3`... whichever is free.
    pub fn free_name(&self, base: &str) -> String {
        let base = base.trim();
        let base = if base.is_empty() { "Workspace" } else { base };
        std::iter::once(base.to_string()).chain((2..).map(|n| format!("{base} {n}"))).find(|n| self.saved(n).is_none()).unwrap()
    }

    /// Adds a Workspace after the active one and returns its name. A copy takes the Instances
    /// and look on screen; a new one starts empty, in the same look. Neither takes the rules.
    pub fn add_workspace(&mut self, name: &str, copy: bool) -> String {
        let name = self.free_name(name);
        let instances = if copy { self.instances.clone() } else { Vec::new() };
        let at = self.active_index() + 1;
        self.workspaces.insert(at, Saved { name: name.clone(), rules: Rules::default(), theme: Some(self.theme.clone()), style: self.style.clone(), instances });
        name
    }

    /// Puts the Workspace on screen away and brings `name` up. False when it is already up or
    /// there is no such Workspace.
    pub fn switch_to(&mut self, name: &str) -> bool {
        if name == self.active {
            return false;
        }
        let Some(to) = self.workspaces.iter().position(|w| w.name == name) else { return false };
        let from = self.active_index();
        let out = &mut self.workspaces[from];
        out.instances = std::mem::take(&mut self.instances);
        out.style = std::mem::take(&mut self.style);
        out.theme = Some(self.theme.clone());
        let inn = &mut self.workspaces[to];
        self.instances = std::mem::take(&mut inn.instances);
        self.style = std::mem::take(&mut inn.style);
        if let Some(t) = inn.theme.take() {
            self.theme = t;
        }
        self.active = name.to_string();
        true
    }

    /// Returns the name it got, trimmed.
    pub fn rename_workspace(&mut self, old: &str, new: &str) -> Result<String, String> {
        let new = new.trim();
        if new.is_empty() {
            return Err("a Workspace needs a name".into());
        }
        if new == old {
            return Ok(new.into());
        }
        if self.saved(new).is_some() {
            return Err(format!("there already is a Workspace called {new}"));
        }
        let w = self.workspaces.iter_mut().find(|w| w.name == old).ok_or_else(|| format!("no Workspace called {old}"))?;
        w.name = new.into();
        if self.active == old {
            self.active = new.into();
        }
        Ok(new.into())
    }

    /// The last Workspace stays. Removing the one on screen brings up its neighbour first,
    /// whose name is returned so the app can show it.
    pub fn delete_workspace(&mut self, name: &str) -> Result<Option<String>, String> {
        let i = self.workspaces.iter().position(|w| w.name == name).ok_or_else(|| format!("no Workspace called {name}"))?;
        if self.workspaces.len() == 1 {
            return Err("the last Workspace cannot go".into());
        }
        let mut shown = None;
        if name == self.active {
            let next = self.workspaces[if i + 1 < self.workspaces.len() { i + 1 } else { i - 1 }].name.clone();
            self.switch_to(&next);
            shown = Some(next);
        }
        self.workspaces.retain(|w| w.name != name);
        Ok(shown)
    }

    pub fn rules_mut(&mut self, name: &str) -> Option<&mut Rules> {
        self.workspaces.iter_mut().find(|w| w.name == name).map(|w| &mut w.rules)
    }

    /// The Workspace whose rules fit `desktop` and the monitors connected (`setup`) best, if
    /// any does: see [`Rules::score`]. Ties go to the first in the user's order.
    pub fn pick(&self, desktop: Option<&str>, setup: &[MonitorRef]) -> Option<String> {
        let mut best: Option<(u8, &Saved)> = None;
        for w in &self.workspaces {
            if let Some(s) = w.rules.score(desktop, setup) {
                if best.is_none_or(|(b, _)| s > b) {
                    best = Some((s, w));
                }
            }
        }
        best.map(|(_, w)| w.name.clone())
    }
}

pub const FIRST_WORKSPACE: &str = "Main";

/// One saved Workspace. The one on screen keeps only its name and rules here.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Saved {
    pub name: String,
    #[serde(skip_serializing_if = "Rules::is_empty")]
    pub rules: Rules,
    /// Its palette, fonts, glyphs and icon pack; `None` while it is on screen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<Selection>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub style: BTreeMap<String, serde_json::Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub instances: Vec<InstanceCfg>,
}

/// What brings a Workspace up by itself. With none it only comes up when picked.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Rules {
    /// Windows virtual desktops, by id (`{GUID}`): going to one shows this Workspace.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub desktops: Vec<String>,
    /// A monitor setup: these monitors connected, no more and no fewer.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub monitors: Vec<MonitorRef>,
}

/// How well a saved monitor setup fits the monitors connected now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SetupFit {
    /// Different monitors.
    No,
    /// As many monitors at the same sizes, under other names: Windows renumbers
    /// `\\.\DISPLAYn` when a dock reconnects.
    Renamed,
    Same,
}

pub fn setup_fit(saved: &[MonitorRef], now: &[MonitorRef]) -> SetupFit {
    let sorted = |v: &[MonitorRef]| {
        let mut v = v.to_vec();
        v.sort();
        v
    };
    let sizes = |v: &[MonitorRef]| {
        let mut v: Vec<(u32, u32)> = v.iter().map(|m| (m.width, m.height)).collect();
        v.sort();
        v
    };
    if sorted(saved) == sorted(now) {
        SetupFit::Same
    } else if sizes(saved) == sizes(now) {
        SetupFit::Renamed
    } else {
        SetupFit::No
    }
}

impl Rules {
    pub fn is_empty(&self) -> bool {
        self.desktops.is_empty() && self.monitors.is_empty()
    }

    /// `None` when a rule it has does not hold. A desktop that holds counts 4, the same
    /// monitors 2 and renamed ones 1, so a Workspace tied to both beats one tied to either,
    /// and a desktop (a choice made just now) beats a monitor setup. No rules: never.
    pub fn score(&self, desktop: Option<&str>, setup: &[MonitorRef]) -> Option<u8> {
        if self.is_empty() {
            return None;
        }
        let d = match (self.desktops.is_empty(), desktop) {
            (true, _) => 0,
            (false, Some(d)) if self.desktops.iter().any(|x| x.eq_ignore_ascii_case(d)) => 4,
            _ => return None,
        };
        let m = match (self.monitors.is_empty(), setup_fit(&self.monitors, setup)) {
            (true, _) => 0,
            (false, SetupFit::Same) => 2,
            (false, SetupFit::Renamed) => 1,
            (false, SetupFit::No) => return None,
        };
        Some(d + m)
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
    fn disabled_plugins_round_trip_and_are_omitted_when_empty() {
        assert!(!serde_json::to_string(&Workspace::default()).unwrap().contains("disabled_plugins"));
        let mut w = Workspace::default();
        w.disabled_plugins.insert("sunset".into());
        let back: Workspace = serde_json::from_str(&serde_json::to_string(&w).unwrap()).unwrap();
        assert_eq!(back.disabled_plugins, w.disabled_plugins);
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

    fn inst(id: &str) -> InstanceCfg {
        InstanceCfg { id: id.into(), widget: id.split('-').next().unwrap().into(), ..Default::default() }
    }

    fn mref(name: &str, w: u32, h: u32) -> MonitorRef {
        MonitorRef { name: name.into(), width: w, height: h }
    }

    #[test]
    fn a_file_from_before_workspaces_is_one_called_main_and_round_trips() {
        let mut old: Workspace = serde_json::from_str(r#"{"version": 2, "instances": [{"id": "clock-1", "widget": "clock"}]}"#).unwrap();
        old.tidy_workspaces();
        assert_eq!((old.names(), old.active.as_str(), old.instances.len()), (vec!["Main".to_string()], "Main", 1));
        let text = serde_json::to_string(&old).unwrap();
        let back: Workspace = serde_json::from_str(&text).unwrap();
        assert_eq!((back.names(), back.instances.len()), (vec!["Main".to_string()], 1));
        assert_eq!(text.matches("clock-1").count(), 1, "the Workspace on screen is saved once: {text}");
    }

    #[test]
    fn switching_puts_the_widgets_and_look_away_and_brings_the_others_back() {
        let mut ws = Workspace { instances: vec![inst("clock-1")], ..Default::default() };
        ws.theme.palette = "Midnight".into();
        ws.style.insert("blur".into(), true.into());
        let work = ws.add_workspace("Work", false);
        assert_eq!((work.as_str(), ws.names()), ("Work", vec!["Main".to_string(), "Work".to_string()]));
        assert!(ws.switch_to("Work") && !ws.switch_to("Work") && !ws.switch_to("Nope"));
        assert!(ws.instances.is_empty(), "a new Workspace starts empty");
        assert_eq!(ws.style.get("blur"), Some(&serde_json::Value::Bool(true)), "in the same look");
        ws.instances.push(inst("drawer-1"));
        ws.theme.palette = "Daylight".into();
        assert!(ws.switch_to("Main"));
        assert_eq!((ws.instances[0].id.as_str(), ws.theme.palette.as_str()), ("clock-1", "Midnight"));
        assert!(ws.switch_to("Work"));
        assert_eq!((ws.instances[0].id.as_str(), ws.theme.palette.as_str()), ("drawer-1", "Daylight"), "each keeps its own look");
        assert_eq!(ws.names(), ["Main", "Work"], "switching never reorders them");
        let back: Workspace = serde_json::from_str(&serde_json::to_string(&ws).unwrap()).unwrap();
        assert_eq!((back.active.as_str(), back.saved("Main").unwrap().instances.len(), back.instances.len()), ("Work", 1, 1));
    }

    #[test]
    fn ids_are_unique_across_workspaces_and_a_copy_keeps_its_widgets() {
        let mut ws = Workspace { instances: vec![inst("clock-1")], ..Default::default() };
        let copy = ws.add_workspace("Main", true);
        assert_eq!(copy, "Main 2", "a name already used gets a number");
        ws.switch_to(&copy);
        assert_eq!(ws.instances[0].id, "clock-1");
        ws.instances.clear();
        assert_eq!(ws.next_id("clock"), "clock-2", "clock-1 is still in Main");
    }

    #[test]
    fn rename_and_delete_keep_one_workspace_on_screen() {
        let mut ws = Workspace { instances: vec![inst("clock-1")], ..Default::default() };
        ws.add_workspace("Work", false);
        assert!(ws.rename_workspace("Main", " Work ").is_err() && ws.rename_workspace("Main", "  ").is_err());
        assert_eq!(ws.rename_workspace("Main", " Home ").unwrap(), "Home");
        assert_eq!(ws.active, "Home");
        assert_eq!(ws.delete_workspace("Home").unwrap(), Some("Work".to_string()), "the next one comes up");
        assert_eq!((ws.names(), ws.active.as_str()), (vec!["Work".to_string()], "Work"));
        assert!(ws.delete_workspace("Work").is_err(), "the last one stays");
    }

    #[test]
    fn only_a_fresh_install_gets_starter_widgets() {
        let mut ws = Workspace::default();
        assert!(ws.wants_starter_widgets());
        ws.onboarded = true;
        assert!(!ws.wants_starter_widgets(), "a Workspace the user emptied stays empty");
        let mut two = Workspace::default();
        two.add_workspace("Work", false);
        assert!(!two.wants_starter_widgets());
    }

    #[test]
    fn rules_pick_the_most_specific_workspace_and_none_without_rules() {
        let (dock, laptop) = (vec![mref("\\\\.\\DISPLAY1", 2560, 1440), mref("\\\\.\\DISPLAY2", 1920, 1080)], vec![mref("\\\\.\\DISPLAY1", 1920, 1200)]);
        let mut ws = Workspace::default();
        for n in ["Desk", "Laptop", "Focus", "Desk focus"] {
            ws.add_workspace(n, false);
        }
        ws.rules_mut("Desk").unwrap().monitors = dock.clone();
        ws.rules_mut("Laptop").unwrap().monitors = laptop.clone();
        ws.rules_mut("Focus").unwrap().desktops = vec!["{AAAA}".into()];
        let both = ws.rules_mut("Desk focus").unwrap();
        (both.desktops, both.monitors) = (vec!["{aaaa}".into()], dock.clone());
        assert_eq!(ws.pick(None, &dock).as_deref(), Some("Desk"));
        assert_eq!(ws.pick(Some("{BBBB}"), &laptop).as_deref(), Some("Laptop"), "an unbound desktop leaves the monitors to decide");
        assert_eq!(ws.pick(Some("{AAAA}"), &laptop).as_deref(), Some("Focus"), "a desktop beats a monitor setup");
        assert_eq!(ws.pick(Some("{AAAA}"), &dock).as_deref(), Some("Desk focus"), "both beat either");
        let renamed = vec![mref("\\\\.\\DISPLAY3", 1920, 1080), mref("\\\\.\\DISPLAY4", 2560, 1440)];
        assert_eq!(setup_fit(&dock, &renamed), SetupFit::Renamed);
        assert_eq!(ws.pick(None, &renamed).as_deref(), Some("Desk"), "a dock that renumbered its monitors still counts");
        assert_eq!(ws.pick(Some("{CCCC}"), &[mref("x", 800, 600)]), None, "nothing fits: stay where you are");
        assert!(Rules::default().score(Some("{AAAA}"), &dock).is_none(), "no rules: only picked by hand");
    }

    #[test]
    fn only_a_fresh_install_needs_the_first_run_setup() {
        assert!(!Workspace::default().onboarded);
        let old: Workspace = serde_json::from_str(r#"{"version": 2, "instances": []}"#).unwrap();
        assert!(old.onboarded, "a file from before the setup existed is done");
        let fresh: Workspace = serde_json::from_str(&serde_json::to_string(&Workspace::default()).unwrap()).unwrap();
        assert!(!fresh.onboarded, "quitting mid-setup shows it again");
    }
}
