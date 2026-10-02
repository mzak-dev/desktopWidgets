//! Modules: the parts of a Widget a user arranges (a gauge, a graph, a footer). A definition
//! file declares named tiers, slots that hold Modules, and the Modules themselves; the
//! Instance's saved layout, or the tier's default, says which Module sits in which slot.

use std::collections::BTreeMap;

use crate::expr::Template;
use crate::format::Elem;
use crate::value::Value;
use crate::workspace::Layout;

#[derive(Clone, Debug)]
pub struct TierDef {
    pub name: String,
    pub label: String,
    /// The first tier whose `when` holds is the current one; a tier without `when` is the
    /// fallback when none does.
    pub when: Option<Template>,
    /// What the Settings preview renders this tier at, logical px.
    pub size: (f32, f32),
    /// slot -> Module names (all of a `for` Module) or ids.
    pub layout: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug)]
pub struct SlotDef {
    pub name: String,
    pub label: String,
}

/// `for = "{sys.gpus}"`: one Module per item.
#[derive(Clone, Debug)]
pub struct Each {
    pub list: Template,
    pub var: String,
    pub key: Option<Template>,
}

#[derive(Clone, Debug)]
pub struct ModuleDef {
    pub name: String,
    pub label: Template,
    /// Where it may be dropped; empty = any slot.
    pub slots: Vec<String>,
    /// layout entry -> the old `show_*` param it replaced, so a saved `false` still hides it.
    pub legacy: BTreeMap<String, String>,
    /// Whether it exists at all right now (a battery on a desktop PC).
    pub when: Option<Template>,
    pub each: Option<Each>,
    pub body: Elem,
}

#[derive(Clone, Debug, Default)]
pub struct ModuleSet {
    pub tiers: Vec<TierDef>,
    pub slots: Vec<SlotDef>,
    pub modules: Vec<ModuleDef>,
}

/// What a build is told about the Instance's arrangement.
#[derive(Clone, Copy)]
pub struct Arrange<'a> {
    pub layout: &'a Layout,
    /// Build this tier whatever the card size says (Settings' tier tabs).
    pub tier: Option<&'a str>,
    /// Settings preview: Modules are hit targets, everything else inert.
    pub preview: bool,
}

/// One Module, or one item of a `for` Module, on this machine right now.
#[derive(Clone, Debug)]
pub struct Inst {
    /// The module name, or `name:key` for an item.
    pub id: String,
    pub module: usize,
    pub label: String,
    pub item: Option<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub id: String,
    pub module: String,
    pub label: String,
    /// The Module's node in the built tree.
    pub key: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlacedSlot {
    pub name: String,
    pub label: String,
    pub key: String,
    pub modules: Vec<Placed>,
    /// Placed here, but the slot has no room for them at this size.
    pub cut: Vec<Placed>,
}

/// What a build placed: for Settings to draw a tray, drop targets and tabs.
#[derive(Clone, Debug, PartialEq)]
pub struct Arrangement {
    pub tier: String,
    pub slots: Vec<PlacedSlot>,
    /// Modules that exist but sit in no slot of this tier.
    pub hidden: Vec<Placed>,
}

impl Arrangement {
    /// The saved form of what is placed now, for one tier.
    pub fn as_layout(&self) -> BTreeMap<String, Vec<String>> {
        self.slots.iter().map(|s| (s.name.clone(), s.modules.iter().chain(&s.cut).map(|m| m.id.clone()).collect())).collect()
    }
}

impl ModuleDef {
    pub fn fits(&self, slot: &str) -> bool {
        self.slots.is_empty() || self.slots.iter().any(|s| s == slot)
    }
}

/// Whether a layout `entry` names `inst`: its id, its Module (every item), or a prefix (`gauge:gpu*`).
pub fn names(entry: &str, inst: &Inst, module: &ModuleDef) -> bool {
    inst.id == entry || module.name == entry || entry.strip_suffix('*').is_some_and(|p| inst.id.starts_with(p))
}

/// Indexes into `insts` per slot. Entries name an id, or a whole Module (every item of it).
/// Each instance lands once, in the first slot that names it and accepts it.
pub fn place(set: &ModuleSet, insts: &[Inst], layout: &BTreeMap<String, Vec<String>>) -> BTreeMap<String, Vec<usize>> {
    let mut used = vec![false; insts.len()];
    let mut out: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (slot, entries) in layout {
        let list = out.entry(slot.clone()).or_default();
        for e in entries {
            for (i, inst) in insts.iter().enumerate() {
                let m = &set.modules[inst.module];
                if !used[i] && names(e, inst, m) && m.fits(slot) {
                    used[i] = true;
                    list.push(i);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::WidgetDef;

    fn set() -> (ModuleSet, Vec<Inst>) {
        let def = WidgetDef::parse(
            "t",
            "[tiers.a]\nlayout = { row = ['x', 'y'], col = ['z'] }\n[slots.row]\n[slots.col]\n[modules.x]\nslots = ['row']\n[modules.y]\nslots = ['row', 'col']\n[modules.z]\nslots = ['row']\n[root]\ntype = 'box'",
        )
        .unwrap();
        let ms = def.modules.unwrap();
        let insts = ms.modules.iter().enumerate().map(|(i, m)| Inst { id: m.name.clone(), module: i, label: m.name.clone(), item: None }).collect();
        (ms, insts)
    }

    #[test]
    fn a_module_lands_once_and_only_where_it_is_allowed() {
        let (ms, insts) = set();
        let placed = place(&ms, &insts, &ms.tiers[0].layout);
        assert_eq!(placed["row"], [0, 1], "x and y");
        assert!(placed["col"].is_empty(), "z is not allowed in col");
        let moved = BTreeMap::from([("col".to_string(), vec!["y".to_string()]), ("row".to_string(), vec!["y".into(), "x".into()])]);
        let placed = place(&ms, &insts, &moved);
        assert_eq!((&placed["col"], &placed["row"]), (&vec![1], &vec![0]), "y is placed once, in the first slot that names it");
    }
}
