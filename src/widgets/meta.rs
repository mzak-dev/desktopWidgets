use crate::data::starter_apps;
use crate::meta::{Seed, WidgetMeta};
use crate::workspace::InstanceCfg;

/// A saved `show_x = false` of a param a Module replaced (`legacy`) becomes a layout
/// without that Module, then the old param goes.
pub fn migrate(meta: &WidgetMeta, cfg: &mut InstanceCfg) {
    let off: Vec<(&String, &String)> = meta.modules.iter().flat_map(|m| &m.legacy).collect();
    if off.is_empty() {
        return;
    }
    let hidden: Vec<&str> = off.iter().filter(|(_, p)| cfg.params.get(*p) == Some(&serde_json::Value::Bool(false))).map(|(e, _)| e.as_str()).collect();
    if !hidden.is_empty() && cfg.layout.is_empty() {
        for t in &meta.tiers {
            let l = t.layout.iter().map(|(s, v)| (s.clone(), v.iter().filter(|e| !hidden.contains(&e.as_str())).cloned().collect())).collect();
            cfg.layout.insert(t.name.clone(), l);
        }
    }
    for (_, p) in off {
        cfg.params.remove(p);
    }
}

pub fn seed_params(meta: &WidgetMeta, cfg: &mut InstanceCfg) {
    for p in &meta.params {
        if let Some(s) = p.seed {
            apply_seed(s, cfg, &p.name);
        }
    }
}

/// Writes the seed's value into the Instance's param, once.
pub fn apply_seed(seed: Seed, cfg: &mut InstanceCfg, param: &str) {
    match seed {
        Seed::StarterApps => cfg.set_shortcuts(param, &starter_apps()),
    }
}
