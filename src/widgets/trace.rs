//! The UI trace over every built-in Widget: turning it on must not change what is drawn, and
//! what it records must agree with the tree and the frame.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use super::safety_net::{TM, sample_items, scripted_sources, theme};
use super::{Inputs, ParamType, Registry};
use crate::anim::Anim;
use crate::data::SourceCx;
use crate::text::TextEngine;
use crate::ui::{self, Env, Frame, Kind, Node, PlacedKind};
use crate::value::Value;
use crate::workspace::InstanceCfg;

fn layout(root: &Node, size: (f32, f32), scale: f32, trace: bool, text: &mut TextEngine) -> Frame {
    ui::layout(root, size, &mut Env { text, anim: &mut Anim::default(), hover: None, now: Instant::now(), scale, trace })
}

fn count(n: &Node, f: &mut dyn FnMut(&Node)) {
    f(n);
    n.children.iter().for_each(|c| count(c, f));
}

#[test]
fn tracing_every_builtin_widget_changes_no_draw_list_and_matches_the_tree() {
    let reg = Registry::load(Path::new("no-such-dir"));
    let (theme, sources, mut text) = (theme(), scripted_sources(), TextEngine::new());
    let (mut cases, mut texts, mut shapes) = (0, 0, 0);
    let mut ids = reg.ids();
    ids.sort();
    for id in ids {
        let Some(Ok(w)) = reg.get(&id) else { continue };
        let meta = w.meta();
        let all_on: BTreeMap<String, Value> = meta.params.iter().filter(|p| p.ty == ParamType::Bool).map(|p| (p.name.clone(), Value::Bool(true))).collect();
        for (variant, extra) in [("defaults", BTreeMap::new()), ("every switch on", all_on)] {
            let mut cfg = InstanceCfg { id: format!("{id}-1"), widget: id.clone(), ..Default::default() };
            super::seed_params(meta, &mut cfg);
            if cfg.items().is_empty() {
                cfg.set_items(&sample_items());
            }
            extra.iter().for_each(|(k, v)| cfg.set_param(k, v));
            let params = meta.effective_params(&cfg.params_map());
            let cx = SourceCx::new(cfg.instance(), &params, TM, "Default");
            let read = |n: &str| sources.value(n, &cx);
            let state = BTreeMap::new();
            let max = meta.max_card_size.unwrap_or(meta.default_card_size);
            for (size, scale) in [(meta.default_card_size, 1.0), (meta.min_card_size, 1.25), (max, 2.0)] {
                let inp = Inputs { params: &params, state: &state, card_size: size, key_prefix: &cfg.id, read_source: &read, arrange: None };
                let Ok(b) = w.build(&inp, &theme, &|_| None) else { continue };
                let case = format!("{id} {size:?} x{scale}, {variant}");
                let (off, on) = (layout(&b.root, size, scale, false, &mut text), layout(&b.root, size, scale, true, &mut text));
                assert!(off.nodes.is_empty(), "{case}: nothing is recorded with the trace off");
                assert!(off.list.to_bytes() == on.list.to_bytes(), "{case}: the draw list differs with the trace on");
                assert_eq!((&off.rects, off.content_size, off.animating), (&on.rects, on.content_size, on.animating), "{case}");
                assert_eq!(off.hits.len(), on.hits.len(), "{case}");
                // the trace is the tree, pre-order, with the frame's rects
                let mut tree = Vec::new();
                count(&b.root, &mut |n| tree.push((n.key.clone(), matches!(n.kind, Kind::Text(_)), matches!(n.kind, Kind::Shape(_)))));
                assert_eq!(on.nodes.len(), tree.len(), "{case}");
                for ((key, is_text, is_shape), (p, (rk, rect))) in tree.iter().zip(on.nodes.iter().zip(&on.rects)) {
                    assert!(p.key == *key && p.key == *rk && p.rect == *rect, "{case}: `{key}` is out of step with the frame");
                    match &p.kind {
                        PlacedKind::Text { spec, run, .. } => {
                            assert!(*is_text, "{case}: `{key}`");
                            texts += 1;
                            let run = run.as_ref().unwrap_or_else(|| panic!("{case}: no shaping facts for `{key}` ({:?})", spec.text));
                            assert!(run.lines >= 1 || spec.text.is_empty(), "{case}: `{key}` shaped no lines");
                        }
                        PlacedKind::Shape { name, attrs } => {
                            assert!(*is_shape && !name.is_empty() && !attrs.is_empty(), "{case}: `{key}` ({name})");
                            shapes += 1;
                        }
                        PlacedKind::Box | PlacedKind::Image { .. } => assert!(!is_text && !is_shape, "{case}: `{key}`"),
                    }
                }
                cases += 1;
            }
        }
    }
    assert!(cases > 20 && texts > 100 && shapes > 10, "the fixtures cover real trees: {cases} cases, {texts} texts, {shapes} shapes");
}
