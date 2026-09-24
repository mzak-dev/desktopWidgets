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
    /// Data sources it cannot work without (`needs = ["agents"]`), so a missing one is
    /// named instead of the widget showing blank.
    pub needs: Vec<String>,
    /// Named card-size tiers, slots and Modules the user may arrange; all empty for a Widget that declares none.
    pub tiers: Vec<TierMeta>,
    pub slots: Vec<(String, String)>,
    pub modules: Vec<ModuleMeta>,
}

#[derive(Clone, Debug)]
pub struct TierMeta {
    pub name: String,
    pub label: String,
    pub size: (f32, f32),
    pub layout: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug)]
pub struct ModuleMeta {
    pub name: String,
    pub label: String,
    pub slots: Vec<String>,
    pub legacy: BTreeMap<String, String>,
}

/// "needs the `agents` data source, which is missing…"
pub fn needs_message(missing: &[&str]) -> String {
    let names = missing.iter().map(|m| format!("`{m}`")).collect::<Vec<_>>().join(" and ");
    let (what, is) = if missing.len() == 1 { ("data source", "is") } else { ("data sources", "are") };
    format!("needs the {names} {what}, which {is} missing. Is the plugin that provides it installed and switched on?")
}

impl WidgetMeta {
    /// Its needed data sources that `has` does not know.
    pub fn unmet(&self, has: impl Fn(&str) -> bool) -> Vec<&str> {
        self.needs.iter().map(String::as_str).filter(|n| !has(n)).collect()
    }

    pub fn effective_params(&self, saved: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
        self.params.iter().map(|p| (p.name.clone(), saved.get(&p.name).cloned().unwrap_or_else(|| p.default.clone()))).collect()
    }

    /// A saved `show_x = false` of a param a Module replaced (`legacy`) becomes a layout
    /// without that Module, then the old param goes.
    pub fn migrate(&self, cfg: &mut InstanceCfg) {
        let off: Vec<(&String, &String)> = self.modules.iter().flat_map(|m| &m.legacy).collect();
        if off.is_empty() {
            return;
        }
        let hidden: Vec<&str> = off.iter().filter(|(_, p)| cfg.params.get(*p) == Some(&serde_json::Value::Bool(false))).map(|(e, _)| e.as_str()).collect();
        if !hidden.is_empty() && cfg.layout.is_empty() {
            for t in &self.tiers {
                let l = t.layout.iter().map(|(s, v)| (s.clone(), v.iter().filter(|e| !hidden.contains(&e.as_str())).cloned().collect())).collect();
                cfg.layout.insert(t.name.clone(), l);
            }
        }
        for (_, p) in off {
            cfg.params.remove(p);
        }
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

    /// `folder` names the folder picker too.
    pub fn parse(s: &str) -> Option<Self> {
        if s == "folder" {
            return Some(Self::Path);
        }
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
    pub choices: Vec<Choice>,
    pub seed: Option<Seed>,
    /// Settings shows params with one group under its own heading (`group = "Motion"`).
    pub group: Option<String>,
    /// The Module this option belongs to; none = an option of the whole Widget.
    pub module: Option<String>,
}

/// One option of an `enum` param: the saved value, and what Settings shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

impl ParamDef {
    /// The params in the order Settings shows them: ungrouped first, then each group in
    /// the order it first appears.
    pub fn grouped(params: &[ParamDef]) -> Vec<(Option<&str>, Vec<&ParamDef>)> {
        let mut out: Vec<(Option<&str>, Vec<&ParamDef>)> = vec![(None, vec![])];
        for p in params {
            let g = p.group.as_deref();
            match out.iter_mut().find(|(k, _)| *k == g) {
                Some((_, v)) => v.push(p),
                None => out.push((g, vec![p])),
            }
        }
        out.retain(|(_, v)| !v.is_empty());
        out
    }
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
