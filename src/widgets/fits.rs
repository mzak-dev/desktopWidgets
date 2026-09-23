//! Every built-in Widget lays out cleanly at every size it allows: its default and a
//! 4x4 grid from its min to its max, so a size tier cannot break in between.

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

/// Text a reader must see whole, and vector shapes (gauges, graphs, hands), outside
/// any scroll container.
fn must_fit(n: &Node, texts: &mut Vec<(String, TextSpec)>, shapes: &mut Vec<String>) {
    if n.scroll_offset.is_some() {
        return;
    }
    match &n.kind {
        Kind::Text(t) if !t.text.trim().is_empty() => texts.push((n.key.clone(), t.clone())),
        Kind::Shape(_) => shapes.push(n.key.clone()),
        _ => {}
    }
    n.children.iter().for_each(|c| must_fit(c, texts, shapes));
}

fn sizes_of(meta: &super::WidgetMeta) -> Vec<(String, (f32, f32))> {
    let (min, max) = (meta.min_card_size, meta.max_card_size.unwrap_or(meta.default_card_size));
    let mut out = vec![("default".to_string(), meta.default_card_size)];
    for i in 0..4 {
        for j in 0..4 {
            let (tx, ty) = (i as f32 / 3.0, j as f32 / 3.0);
            out.push((format!("{i}/3,{j}/3 of min..max"), ((min.0 + (max.0 - min.0) * tx).round(), (min.1 + (max.1 - min.1) * ty).round())));
        }
    }
    out
}

#[test]
fn every_builtin_fits_every_size_it_allows() {
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
        let sizes = sizes_of(meta);
        let all_on: BTreeMap<String, Value> = meta.params.iter().filter(|p| p.ty == ParamType::Bool).map(|p| (p.name.clone(), Value::Bool(true))).collect();
        for (variant, extra) in [("defaults", BTreeMap::new()), ("every switch on", all_on)] {
            let mut cfg = InstanceCfg { id: format!("{id}-1"), widget: id.clone(), ..Default::default() };
            meta.seed_params(&mut cfg);
            extra.iter().for_each(|(k, v)| cfg.set_param(k, v));
            let params = meta.effective_params(&cfg.params_map());
            for (size_name, size) in &sizes {
                let case = format!("{id} at {size_name} {size:?}, {variant}");
                let cx = SourceCx { cfg: &cfg, params: &params, tm, icon_pack: "Default" };
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
                let (mut texts, mut shapes) = (Vec::new(), Vec::new());
                must_fit(&b.root, &mut texts, &mut shapes);
                let inside = |[x, y, w, h]: [f32; 4]| x >= -0.5 && y >= -0.5 && x + w <= size.0 + 0.5 && y + h <= size.1 + 0.5;
                for key in shapes {
                    if let Some(r) = frame.rect_of(&key).filter(|r| !inside(*r)) {
                        failures.push(format!("{case}: shape `{key}` at {r:?} sticks out of the card"));
                    }
                }
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

/// Every text a Widget builds at `size` with its defaults, the way the app reads its sources.
fn texts_at(id: &str, size: (f32, f32)) -> Vec<String> {
    let reg = Registry::load(Path::new("no-such-dir"));
    let Some(Ok(w)) = reg.get(id) else { panic!("{id}") };
    let cfg = InstanceCfg { id: format!("{id}-1"), widget: id.into(), ..Default::default() };
    let params = w.meta().effective_params(&cfg.params_map());
    let sources = DataSources::builtin();
    let tm = Tm { year: 2026, month: 9, day: 23, dow: 3, hour: 12, minute: 0, second: 0, ms: 0 };
    let cx = SourceCx { cfg: &cfg, params: &params, tm, icon_pack: "Default" };
    let read = |n: &str| sources.value(n, &cx);
    let state = BTreeMap::new();
    let inp = Inputs { params: &params, state: &state, card_size: size, key_prefix: "t", read_source: &read };
    let root = w.build(&inp, &Theme::compose(&Library::load(Path::new("nope")), &Selection::default(), &[]), &|_| None).unwrap().root;
    fn walk(n: &Node, out: &mut Vec<String>) {
        if let Kind::Text(t) = &n.kind {
            out.push(t.text.clone());
        }
        n.children.iter().for_each(|c| walk(c, out));
    }
    let mut out = Vec::new();
    walk(&root, &mut out);
    out
}

#[test]
fn a_big_clock_shows_its_default_cities_and_a_small_one_does_not() {
    let tall = texts_at("clock", (300.0, 360.0));
    assert!(tall.iter().any(|t| t == "London") && !tall.iter().any(|t| t == "Tokyo"), "tall: as many chips as fit under the face (two at 300 px): {tall:?}");
    assert!(texts_at("clock", (520.0, 220.0)).iter().any(|t| t == "London"), "wide: a list beside it");
    assert!(!texts_at("clock", (220.0, 220.0)).iter().any(|t| t == "Tokyo"), "default size: just the face");
}
