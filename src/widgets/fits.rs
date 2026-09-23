//! Every built-in Widget lays out cleanly at its min, default and max card size.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use super::{Inputs, ParamType, Registry};
use crate::anim::Anim;
use crate::data::{DataSources, SourceCx, Tm};
use crate::text::{TextEngine, TextSpec};
use crate::theme::{Library, Selection, Theme};
use crate::ui::{self, Env, Kind, Node};
use crate::value::Value;
use crate::workspace::InstanceCfg;

/// Text a reader must see whole: anything outside a scroll container.
fn fixed_texts(n: &Node, out: &mut Vec<(String, TextSpec)>) {
    if n.scroll_offset.is_some() {
        return;
    }
    if let Kind::Text(t) = &n.kind {
        if !t.text.trim().is_empty() {
            out.push((n.key.clone(), t.clone()));
        }
    }
    n.children.iter().for_each(|c| fixed_texts(c, out));
}

#[test]
fn every_builtin_fits_its_min_default_and_max_size() {
    let reg = Registry::load(Path::new("no-such-dir"));
    let theme = Theme::compose(&Library::load(Path::new("nope")), &Selection::default(), &[]);
    let sources = DataSources::builtin();
    // the widest realistic clock text
    let tm = Tm { year: 2026, month: 9, day: 23, dow: 3, hour: 23, minute: 58, second: 58, ms: 0 };
    let mut text = TextEngine::new();
    let mut failures = Vec::new();
    for id in reg.ids() {
        let Some(Ok(w)) = reg.get(&id) else { continue };
        let meta = w.meta();
        let mut sizes = vec![("min", meta.min_card_size), ("default", meta.default_card_size)];
        sizes.extend(meta.max_card_size.map(|m| ("max", m)));
        let all_on: BTreeMap<String, Value> = meta.params.iter().filter(|p| p.ty == ParamType::Bool).map(|p| (p.name.clone(), Value::Bool(true))).collect();
        for (variant, extra) in [("defaults", BTreeMap::new()), ("every switch on", all_on)] {
            let mut cfg = InstanceCfg { id: format!("{id}-1"), widget: id.clone(), ..Default::default() };
            meta.seed_params(&mut cfg);
            extra.iter().for_each(|(k, v)| cfg.set_param(k, v));
            let params = cfg.params_map();
            for (size_name, size) in &sizes {
                let case = format!("{id} at {size_name} {size:?}, {variant}");
                let cx = SourceCx { cfg: &cfg, tm, icon_pack: "Default" };
                let read = |n: &str| sources.value(n, &cx);
                let state = BTreeMap::new();
                let inp = Inputs { params: &params, state: &state, card_size: *size, key_prefix: &cfg.id, read_source: &read };
                let b = match w.build(&inp, &theme, &|_| None) {
                    Ok(b) => b,
                    Err(e) => {
                        failures.push(format!("{case}: {e}"));
                        continue;
                    }
                };
                if !b.warnings.is_empty() {
                    failures.push(format!("{case}: warnings {:?}", b.warnings));
                }
                let frame = {
                    let mut anim = Anim::default();
                    let mut env = Env { text: &mut text, anim: &mut anim, hover: None, now: Instant::now(), scale: 1.0 };
                    ui::layout(&b.root, *size, &mut env)
                };
                let mut texts = Vec::new();
                fixed_texts(&b.root, &mut texts);
                for (key, spec) in texts {
                    let Some([x, y, tw, th]) = frame.rect_of(&key) else { continue };
                    if tw < 1.0 || th < 1.0 {
                        failures.push(format!("{case}: `{}` squashed to {tw}x{th}", spec.text));
                    } else if x < -0.5 || y < -0.5 || x + tw > size.0 + 0.5 || y + th > size.1 + 0.5 {
                        failures.push(format!("{case}: `{}` at {:?} sticks out of the card", spec.text, [x, y, tw, th]));
                    } else if !spec.wrap && text.measure(&key, &spec, None).0 > tw + 1.0 {
                        failures.push(format!("{case}: `{}` is cut off ({tw} px wide)", spec.text));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
