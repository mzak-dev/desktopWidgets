//! Modules: the parts of a Widget a user arranges (a gauge, a graph, a footer). A definition
//! file declares named tiers, slots that hold Modules, and the Modules themselves; the
//! Instance's saved layout, or the tier's default, says which Module sits in which slot.

use std::collections::BTreeMap;

use crate::expr::{Scope, Template};
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

/// What arranging a Widget for one frame produced, before its Modules are built into a tree:
/// which Tier is active, every Module (or item of a `for` Module) that exists right now, and
/// which slot each landed in (see `place`).
pub struct Resolved {
    pub tier: String,
    pub insts: Vec<Inst>,
    pub placed: BTreeMap<String, Vec<usize>>,
}

impl ModuleSet {
    /// Picks the Tier (forced, else the first whose `when` holds, else the one with none, else
    /// the last), lists the Modules that exist now against `scope`, and places them per the
    /// Instance's saved layout or the Tier's default.
    pub fn resolve(&self, scope: &mut Scope, a: Option<Arrange>) -> Result<Resolved, String> {
        let forced = a.and_then(|a| a.tier).and_then(|n| self.tiers.iter().find(|t| t.name == n));
        let tier = match forced {
            Some(t) => t,
            None => {
                let mut hit = None;
                for t in &self.tiers {
                    if let Some(w) = &t.when {
                        if w.eval(scope).map_err(|e| format!("tiers.{}.when: {e}", t.name))?.truthy() {
                            hit = Some(t);
                            break;
                        }
                    }
                }
                hit.or_else(|| self.tiers.iter().find(|t| t.when.is_none())).unwrap_or(&self.tiers[self.tiers.len() - 1])
            }
        };
        let tier_name = tier.name.clone();
        scope.set("tier", Value::Str(tier_name.clone()));
        let mut insts = Vec::new();
        for (mi, m) in self.modules.iter().enumerate() {
            let what = format!("modules.{}", m.name);
            let Some(e) = &m.each else {
                if let Some(inst) = Self::instance(scope, m, mi, None, 0, &what)? {
                    insts.push(inst);
                }
                continue;
            };
            let list = match e.list.eval(scope).map_err(|x| format!("{what}.for: {x}"))? {
                Value::List(l) => l,
                Value::Nil => vec![],
                other => return Err(format!("{what}.for: expected a list, got `{other}`")),
            };
            for (i, item) in list.into_iter().enumerate() {
                scope.set(&e.var, item.clone());
                scope.set("index", Value::Num(i as f64));
                let inst = Self::instance(scope, m, mi, Some(item), i, &what);
                scope.pop();
                scope.pop();
                if let Some(inst) = inst? {
                    insts.push(inst);
                }
            }
        }
        let user = a.and_then(|a| a.layout.get(&tier_name));
        let placed = place(self, &insts, user.unwrap_or(&tier.layout));
        Ok(Resolved { tier: tier_name, insts, placed })
    }

    /// The Module (or one item of it), unless its `when` says it does not exist.
    fn instance(scope: &Scope, m: &ModuleDef, module: usize, item: Option<Value>, i: usize, what: &str) -> Result<Option<Inst>, String> {
        if let Some(w) = &m.when {
            if !w.eval(scope).map_err(|e| format!("{what}.when: {e}"))?.truthy() {
                return Ok(None);
            }
        }
        let label = m.label.eval(scope).map_err(|e| format!("{what}.label: {e}"))?.to_string();
        let id = match m.each.as_ref().map(|e| &e.key) {
            None => m.name.clone(),
            Some(None) => format!("{}:{i}", m.name),
            Some(Some(k)) => format!("{}:{}", m.name, k.eval(scope).map_err(|e| format!("{what}.key: {e}"))?),
        };
        Ok(Some(Inst { id, module, label, item }))
    }
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

    fn modules(src: &str) -> ModuleSet {
        WidgetDef::parse("t", src).unwrap().modules.unwrap()
    }

    fn scope_at(w: f64) -> Scope<'static> {
        let mut sc = Scope::new();
        sc.set("self", Value::obj([("w", w.into())]));
        sc
    }

    #[test]
    fn a_forced_tier_wins_even_if_its_when_does_not_hold() {
        let ms = modules("[tiers.compact]\nwhen = \"{self.w < 260}\"\n[tiers.normal]\n[root]\ntype = 'box'");
        let layout = Layout::new();
        let a = Arrange { layout: &layout, tier: Some("compact"), preview: false };
        let r = ms.resolve(&mut scope_at(500.0), Some(a)).unwrap();
        assert_eq!(r.tier, "compact", "the caller named a tier, so its own `when` is not asked");
    }

    #[test]
    fn the_first_tier_whose_when_holds_wins_else_the_one_without() {
        let ms = modules("[tiers.a]\nwhen = \"{self.w < 100}\"\n[tiers.b]\nwhen = \"{self.w < 300}\"\n[tiers.c]\n[root]\ntype = 'box'");
        assert_eq!(ms.resolve(&mut scope_at(200.0), None).unwrap().tier, "b", "a's when fails, b's holds");
        assert_eq!(ms.resolve(&mut scope_at(500.0), None).unwrap().tier, "c", "neither a nor b holds, c has no when");
    }

    #[test]
    fn the_last_tier_is_the_fallback_when_every_one_has_a_when() {
        let ms = modules("[tiers.a]\nwhen = \"{self.w < 100}\"\n[tiers.b]\nwhen = \"{self.w < 200}\"\n[root]\ntype = 'box'");
        assert_eq!(ms.resolve(&mut scope_at(500.0), None).unwrap().tier, "b", "nothing holds and nothing lacks a when");
    }

    #[test]
    fn a_for_module_makes_one_instance_per_item_and_when_hides_others() {
        let ms = modules(
            "[tiers.a]\n[modules.g]\nfor = \"{items}\"\nas = \"it\"\nkey = \"{it.k}\"\n[modules.hidden]\nwhen = \"{show_hidden}\"\n[root]\ntype = 'box'",
        );
        let mut sc = Scope::new();
        sc.set("items", Value::List(vec![Value::obj([("k", "x".into())]), Value::obj([("k", "y".into())])]));
        sc.set("show_hidden", Value::Bool(false));
        let r = ms.resolve(&mut sc, None).unwrap();
        let ids: Vec<&str> = r.insts.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["g:x", "g:y"], "one Inst per item, keyed by `key`, and `hidden`'s when excludes it");
    }
}
