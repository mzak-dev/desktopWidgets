use std::collections::BTreeMap;

use crate::data::starter_apps;
use crate::value::Value;
use crate::workspace::InstanceCfg;

#[derive(Clone, Debug)]
pub struct WidgetMeta {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Logical px, like every size here.
    pub default_card_size: (f32, f32),
    pub min_card_size: (f32, f32),
    /// Resizing stops here unless the Instance switches its size limit off.
    pub max_card_size: Option<(f32, f32)>,
    pub params: Vec<ParamDef>,
    pub initial_state: BTreeMap<String, Value>,
}

impl WidgetMeta {
    pub fn effective_params(&self, saved: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
        self.params.iter().map(|p| (p.name.clone(), saved.get(&p.name).cloned().unwrap_or_else(|| p.default.clone()))).collect()
    }

    pub fn seed_params(&self, cfg: &mut InstanceCfg) {
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
    pub seed: Option<Seed>,
}

/// Unlike a default, a seed is written once and then saved like any edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seed {
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

    pub fn param_type(self) -> ParamType {
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
