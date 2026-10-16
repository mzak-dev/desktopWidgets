//! What a window drew, as the Automation Bridge's elements. Pure: a laid-out `Frame` in,
//! descriptions out, so it is tested without a window or a GPU.

use std::collections::{BTreeMap, HashMap, HashSet};

use raywright_bridge::protocol::{ElementDesc, Rect as PRect};

use crate::ui::Frame;

/// Keys count as outside any clip when the clip is this large (`draw::NO_CLIP`).
const UNCLIPPED: f32 = 1e5;

/// The nodes a test can use: those that take a click or a hover, show text, or scroll.
/// Everything else the frame laid out is structure and would only make polling slower.
///
/// `text_of` gives a Text node's string by key; `attrs` go on every element.
pub fn elements(frame: &Frame, scale: f32, text_of: &dyn Fn(&str) -> Option<String>, attrs: &BTreeMap<String, String>) -> Vec<ElementDesc> {
    let hits: HashMap<&str, &crate::ui::Hit> = frame.hits.iter().map(|h| (h.key.as_str(), h)).collect();
    let texts: HashMap<&str, [f32; 4]> = frame.list.layers.iter().flat_map(|l| &l.texts).map(|t| (t.key.as_str(), t.clip)).collect();
    let scrolls: HashSet<&str> = frame.scrolls.iter().map(|s| s.key.as_str()).collect();
    let used = |k: &str| hits.contains_key(k) || texts.contains_key(k) || scrolls.contains(k);
    let keys: HashSet<&str> = frame.rects.iter().map(|(k, _)| k.as_str()).filter(|k| used(k)).collect();

    // a button's name is the text inside it, in paint order
    let mut inside: HashMap<&str, Vec<String>> = HashMap::new();
    for (k, _) in &frame.rects {
        if !texts.contains_key(k.as_str()) {
            continue;
        }
        if let Some(t) = text_of(k).filter(|t| !t.trim().is_empty()) {
            let mut up = parent_key(k, &keys);
            while let Some(p) = up {
                inside.entry(p).or_default().push(t.clone());
                up = parent_key(p, &keys);
            }
        }
    }

    frame
        .rects
        .iter()
        // JSON has no NaN or infinity: one such rect would spoil the whole answer
        .filter(|(k, r)| keys.contains(k.as_str()) && r.iter().all(|v| v.is_finite()))
        .map(|(k, [x, y, w, h])| {
            let mut e = ElementDesc::new(k.clone(), PRect::new(*x as f64, *y as f64, *w as f64, *h as f64));
            let hit = hits.get(k.as_str());
            if let Some(a) = hit.and_then(|h| h.action.as_deref()) {
                e = e.action(a);
            }
            let own = texts.contains_key(k.as_str()).then(|| text_of(k)).flatten();
            let role = match (&own, hit.is_some_and(|h| h.action.is_some() || h.on_slide.is_some())) {
                (_, true) => "button",
                (Some(_), false) => "text",
                _ if scrolls.contains(k.as_str()) => "list",
                _ => "group",
            };
            e = e.role(role);
            if let Some(t) = own {
                e = e.text(t);
            } else if let Some(parts) = inside.get(k.as_str()) {
                e = e.attr("name", parts.join(" "));
            }
            // the visible part: a hit knows its clip (logical px, edges); a text knows its own (physical)
            let clip = hit.map(|h| h.clip).or_else(|| texts.get(k.as_str()).map(|c| c.map(|v| v / scale)));
            if let Some([l, t, r, b]) = clip.filter(|c| c.iter().all(|v| v.abs() < UNCLIPPED)) {
                e = e.clip(PRect::new(l as f64, t as f64, (r - l).max(0.0) as f64, (b - t).max(0.0) as f64));
            }
            if let Some(p) = parent_key(k, &keys) {
                e = e.parent(p);
            }
            for (name, value) in attrs {
                e = e.attr(name.clone(), value.clone());
            }
            e
        })
        .collect()
}

/// The nearest enclosing key among `keys`: `a/b/c` sits inside `a/b`, else `a`.
fn parent_key<'a>(key: &str, keys: &HashSet<&'a str>) -> Option<&'a str> {
    let mut rest = key;
    while let Some(i) = rest.rfind('/') {
        rest = &rest[..i];
        if let Some(k) = keys.get(rest) {
            return Some(k);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::TextItem;
    use crate::ui::Hit;

    fn hit(key: &str, action: Option<&str>, rect: [f32; 4]) -> Hit {
        Hit { rect, clip: crate::draw::NO_CLIP, key: key.into(), action: action.map(String::from), on_drop: None, on_slide: None }
    }

    fn text(key: &str) -> TextItem {
        TextItem { key: key.into(), x: 0.0, y: 0.0, scale: 1.0, color: [0; 4], clip: [0.0, 0.0, 200.0, 100.0] }
    }

    fn media_card() -> Frame {
        let mut f = Frame::default();
        f.rects = vec![
            ("m".into(), [0.0, 0.0, 200.0, 100.0]),
            ("m/title".into(), [10.0, 5.0, 100.0, 20.0]),
            ("m/play".into(), [10.0, 40.0, 40.0, 40.0]),
            ("m/play/t".into(), [20.0, 50.0, 20.0, 16.0]),
            ("m/pad".into(), [0.0, 90.0, 200.0, 10.0]),
        ];
        f.hits = vec![hit("m/play", Some("media.play_pause"), [10.0, 40.0, 40.0, 40.0])];
        f.list.layers[0].texts = vec![text("m/title"), text("m/play/t")];
        f
    }

    fn strings(k: &str) -> Option<String> {
        match k {
            "m/title" => Some("Song".into()),
            "m/play/t" => Some("Play".into()),
            _ => None,
        }
    }

    #[test]
    fn only_what_takes_clicks_or_shows_text_is_an_element() {
        let els = elements(&media_card(), 1.0, &strings, &BTreeMap::new());
        let keys: Vec<&str> = els.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["m/title", "m/play", "m/play/t"], "the card and its padding are structure, in paint order");
    }

    #[test]
    fn a_click_target_is_a_button_named_by_the_text_inside_it() {
        let els = elements(&media_card(), 1.0, &strings, &BTreeMap::new());
        let play = els.iter().find(|e| e.key == "m/play").unwrap();
        assert_eq!((play.role.as_deref(), play.action.as_deref(), play.attrs.get("name").map(String::as_str)), (Some("button"), Some("media.play_pause"), Some("Play")));
        assert_eq!(play.text, None, "the label belongs to the text node inside");
        let title = els.iter().find(|e| e.key == "m/title").unwrap();
        assert_eq!((title.role.as_deref(), title.text.as_deref()), (Some("text"), Some("Song")));
    }

    #[test]
    fn elements_know_the_element_they_sit_in() {
        let els = elements(&media_card(), 1.0, &strings, &BTreeMap::new());
        let parent = |k: &str| els.iter().find(|e| e.key == k).unwrap().parent.clone();
        assert_eq!((parent("m/title"), parent("m/play"), parent("m/play/t")), (None, None, Some("m/play".into())), "the card is not an element, so the button has no parent");
    }

    #[test]
    fn clips_are_kept_only_when_something_clips() {
        let mut f = media_card();
        f.hits[0].clip = [10.0, 40.0, 30.0, 60.0];
        let els = elements(&f, 2.0, &strings, &BTreeMap::new());
        let get = |k: &str| els.iter().find(|e| e.key == k).unwrap().clip;
        assert_eq!(get("m/play"), Some(PRect::new(10.0, 40.0, 20.0, 20.0)), "a hit's clip is in logical px");
        assert_eq!(get("m/title"), Some(PRect::new(0.0, 0.0, 100.0, 50.0)), "a text's clip is physical: halved at 2x");
        let unclipped = elements(&media_card(), 1.0, &|_| None, &BTreeMap::new());
        assert_eq!(unclipped.iter().find(|e| e.key == "m/play").unwrap().clip, None);
    }

    #[test]
    fn a_rect_json_cannot_hold_is_left_out() {
        let mut f = media_card();
        f.rects[1].1 = [f32::NAN, 5.0, 100.0, 20.0];
        let keys: Vec<String> = elements(&f, 1.0, &strings, &BTreeMap::new()).into_iter().map(|e| e.key).collect();
        assert_eq!(keys, ["m/play", "m/play/t"]);
        assert!(serde_json::to_string(&elements(&f, 1.0, &strings, &BTreeMap::new())).unwrap().find("null").is_none());
    }

    #[test]
    fn app_attributes_ride_on_every_element() {
        let attrs = BTreeMap::from([("instance".to_string(), "clock-1".to_string())]);
        let els = elements(&media_card(), 1.0, &strings, &attrs);
        assert!(els.iter().all(|e| e.attrs.get("instance").map(String::as_str) == Some("clock-1")));
    }
}
