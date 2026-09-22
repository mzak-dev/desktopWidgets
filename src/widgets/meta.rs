//! What the engine and Settings know about a Widget without building it:
//! names, sizes, typed params (which become the settings form) and the
//! initial Instance state. Every Widget adapter provides one.

use std::collections::BTreeMap;

use crate::data::starter_apps;
use crate::value::Value;
use crate::workspace::InstanceCfg;

#[derive(Clone, Debug)]
pub struct WidgetMeta {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Default card size, logical px.
    pub size: (f32, f32),
    /// Smallest card size in Edit Mode, logical px.
    pub min_size: (f32, f32),
    pub params: Vec<ParamDef>,
    /// Initial `state.*` of a new Instance.
    pub state: BTreeMap<String, Value>,
}

impl WidgetMeta {
    /// Defaults overlaid with an Instance's saved values.
    pub fn effective_params(&self, over: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
        self.params.iter().map(|p| (p.name.clone(), over.get(&p.name).cloned().unwrap_or_else(|| p.default.clone()))).collect()
    }

    /// The Widget styles blur itself (it has a `blur` param), so the
    /// Workspace-wide blur look is left to it.
    pub fn styles_blur(&self) -> bool {
        self.params.iter().any(|p| p.name == "blur")
    }

    /// Write every param's seed into a new Instance.
    pub fn seed(&self, cfg: &mut InstanceCfg) {
        for p in &self.params {
            if let Some(s) = p.seed {
                s.apply(cfg, &p.name);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamType {
    Color,
    Font,
    Number,
    Enum,
    Bool,
    Str,
    Path,
    Duration,
    Shortcuts,
}

impl ParamType {
    pub const ALL: [ParamType; 9] = [Self::Color, Self::Font, Self::Number, Self::Enum, Self::Bool, Self::Str, Self::Path, Self::Duration, Self::Shortcuts];

    /// The `type = "..."` name in a widget file.
    pub fn id(self) -> &'static str {
        match self {
            Self::Color => "color",
            Self::Font => "font",
            Self::Number => "number",
            Self::Enum => "enum",
            Self::Bool => "bool",
            Self::Str => "string",
            Self::Path => "path",
            Self::Duration => "duration",
            Self::Shortcuts => "shortcuts",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.id() == s)
    }
}

#[derive(Clone, Debug)]
pub struct ParamDef {
    pub name: String,
    pub ty: ParamType,
    pub default: Value,
    pub label: String,
    pub help: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    pub choices: Vec<String>,
    /// Written once when an Instance is added (CONTEXT.md: Seed).
    pub seed: Option<Seed>,
}

/// A value a param is given once, when an Instance is added, and then saved
/// like any edit: unlike a default, it shows in Settings and can be changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seed {
    /// A few apps every Windows machine has (`shortcuts` params).
    StarterApps,
}

impl Seed {
    pub const ALL: [Seed; 1] = [Seed::StarterApps];

    pub fn id(self) -> &'static str {
        match self {
            Seed::StarterApps => "starter-apps",
        }
    }

    pub fn parse(s: &str) -> Option<Seed> {
        Seed::ALL.into_iter().find(|x| x.id() == s)
    }

    /// The param type this seed fills.
    pub fn fits(self) -> ParamType {
        match self {
            Seed::StarterApps => ParamType::Shortcuts,
        }
    }

    pub fn apply(self, cfg: &mut InstanceCfg, param: &str) {
        match self {
            Seed::StarterApps => cfg.set_shortcuts(param, &starter_apps()),
        }
    }
}
