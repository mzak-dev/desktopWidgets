//! The pictures in an Instance's `folder` param, for the photo gallery, the photo frame and the
//! GIF player. The folder is watched, not polled, and a file or folder dropped on a widget
//! (`on_drop = "gallery.drop"`) becomes its setting, so dropping a photo picks its folder.
//!
//! Fields: `items` (name, path, ext, size, modified_ms), `count`, `folder`, `truncated`,
//! `error`, and the slide a frame shows: `index` and `current`. Slides move on every
//! `interval` seconds, and by hand with `gallery.next` / `gallery.prev`.
//! Params: `folder`, `sort` (name · newest · oldest), `kinds` (all · animated), `interval`,
//! `shuffle`.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use super::clock::days_from_civil;
use super::{Cadence, DataSource, Notifier, SourceCx, Tm};
use crate::value::Value;

const PICTURES: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "bmp"];
/// What may move: a PNG may be an APNG and a WebP may be animated, so both count.
const ANIMATIONS: &[&str] = &["gif", "webp", "png"];
/// A grid of thumbnails this long is already more than a desktop widget shows.
pub const MAX_ITEMS: usize = 400;

#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    /// The file name without its extension.
    pub name: String,
    pub path: String,
    /// Lower case, without the dot.
    pub ext: String,
    pub size: u64,
    pub modified_ms: i64,
}

impl Picture {
    fn to_value(&self) -> Value {
        Value::obj([
            ("name", self.name.as_str().into()),
            ("path", self.path.as_str().into()),
            ("ext", self.ext.as_str().into()),
            ("size", (self.size as f64).into()),
            ("modified_ms", (self.modified_ms as f64).into()),
        ])
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Listing {
    pub items: Vec<Picture>,
    /// More pictures than `MAX_ITEMS`: only the first ones are listed.
    pub truncated: bool,
    /// Why the folder could not be read, for the widget to say.
    pub error: String,
}

/// The lower-case extension when `name` is a picture of the `kinds` asked for.
pub fn ext_of(name: &str, kinds: &str) -> Option<String> {
    let e = name.rsplit_once('.')?.1.to_ascii_lowercase();
    let wanted = if kinds == "animated" { ANIMATIONS } else { PICTURES };
    wanted.contains(&e.as_str()).then_some(e)
}

pub fn sort(items: &mut [Picture], how: &str) {
    match how {
        "newest" => items.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms)),
        "oldest" => items.sort_by(|a, b| a.modified_ms.cmp(&b.modified_ms)),
        _ => items.sort_by_key(|p| p.name.to_lowercase()),
    }
}

pub fn list(folder: &str, how: &str, kinds: &str) -> Listing {
    let rd = match std::fs::read_dir(folder) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Listing { error: "This folder is not there any more.".into(), ..Default::default() },
        Err(e) => return Listing { error: format!("Cannot read this folder: {e}"), ..Default::default() },
    };
    let mut items: Vec<Picture> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let file = e.file_name().to_string_lossy().into_owned();
            if file.starts_with('.') {
                return None;
            }
            let ext = ext_of(&file, kinds)?;
            let m = e.metadata().ok().filter(|m| m.is_file())?;
            let modified_ms = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64);
            let name = file[..file.len() - ext.len() - 1].to_string();
            Some(Picture { name, path: e.path().to_string_lossy().into_owned(), ext, size: m.len(), modified_ms })
        })
        .collect();
    sort(&mut items, how);
    let truncated = items.len() > MAX_ITEMS;
    items.truncate(MAX_ITEMS);
    Listing { items, truncated, error: String::new() }
}

/// Which picture a slideshow shows at step `pos`: in order, or shuffled afresh each time round.
pub fn slide(pos: i64, count: usize, shuffle: bool, seed: u64) -> usize {
    let n = count.max(1) as i64;
    let (round, at) = (pos.div_euclid(n), pos.rem_euclid(n) as usize);
    if !shuffle || count < 2 {
        return at;
    }
    let mut order: Vec<usize> = (0..count).collect();
    let mut state = seed ^ (round as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    for i in (1..count).rev() {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let j = (mix(state) % (i as u64 + 1)) as usize;
        order.swap(i, j);
    }
    order[at]
}

/// SplitMix64's output step.
fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// FNV-1a, so a folder shuffles the same way every run.
fn seed_of(s: &str) -> u64 {
    s.bytes().fold(0xCBF2_9CE4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01B3))
}

/// Seconds since 1970 on the clock the widgets show, so slides change with it.
fn local_secs(tm: &Tm) -> i64 {
    days_from_civil(tm.year, tm.month, tm.day) * 86_400 + tm.hour as i64 * 3600 + tm.minute as i64 * 60 + tm.second as i64
}

/// How often the slide can change: the coarsest boundary every step lands on.
pub fn slide_cadence(interval: i64) -> Option<Cadence> {
    match interval {
        i if i <= 0 => None,
        i if i % 60 == 0 => Some(Cadence::Minute),
        i if i % 10 == 0 => Some(Cadence::TenSecond),
        _ => Some(Cadence::Second),
    }
}

/// The params a dropped path sets: a folder is the folder (and wins over a single file); a file
/// is the widget's `path` when it has one, else its folder is.
pub fn dropped(p: &Path, has_path: bool) -> Vec<(&'static str, String)> {
    let text = |p: &Path| p.to_string_lossy().into_owned();
    if p.is_dir() {
        let mut v = vec![("folder", text(p))];
        if has_path {
            v.push(("path", String::new()));
        }
        v
    } else if p.is_file() && has_path {
        vec![("path", text(p))]
    } else if p.is_file() {
        p.parent().map(|d| vec![("folder", text(d))]).unwrap_or_default()
    } else {
        vec![]
    }
}

fn text(cx: &SourceCx, k: &str) -> String {
    cx.params.get(k).map(|v| v.to_string()).unwrap_or_default()
}

fn interval(cx: &SourceCx) -> i64 {
    cx.params.get("interval").and_then(Value::as_f64).unwrap_or(0.0).max(0.0) as i64
}

#[derive(Default)]
pub struct Gallery {
    /// Per (folder, sort, kinds), until the folder changes.
    listings: Mutex<HashMap<(String, String, String), Listing>>,
    /// Slides moved by hand, per Instance.
    offsets: Mutex<HashMap<String, i64>>,
    notify: Mutex<Option<Notifier>>,
}

impl Gallery {
    pub fn listing(&self, folder: &str, how: &str, kinds: &str) -> Listing {
        let mut cache = self.listings.lock().unwrap_or_else(|e| e.into_inner());
        cache.entry((folder.to_string(), how.to_string(), kinds.to_string())).or_insert_with(|| list(folder, how, kinds)).clone()
    }

    fn notifier(&self) -> Option<Notifier> {
        self.notify.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl DataSource for Gallery {
    fn name(&self) -> &str {
        "gallery"
    }

    fn value(&self, cx: &SourceCx) -> Value {
        let folder = text(cx, "folder");
        let l = if folder.is_empty() { Listing::default() } else { self.listing(&folder, &text(cx, "sort"), &text(cx, "kinds")) };
        let items: Vec<Value> = l.items.iter().map(Picture::to_value).collect();
        let index = (!items.is_empty()).then(|| {
            let step = match interval(cx) {
                0 => 0,
                i => local_secs(&cx.tm).div_euclid(i),
            };
            let offset = self.offsets.lock().unwrap_or_else(|e| e.into_inner()).get(&cx.cfg.id).copied().unwrap_or(0);
            slide(step + offset, items.len(), cx.params.get("shuffle").is_some_and(Value::truthy), seed_of(&folder))
        });
        Value::obj([
            ("count", (items.len() as i32).into()),
            ("index", index.map_or(-1, |i| i as i32).into()),
            ("current", index.map_or(Value::Nil, |i| items[i].clone())),
            ("items", Value::List(items)),
            ("folder", folder.into()),
            ("truncated", l.truncated.into()),
            ("error", l.error.into()),
        ])
    }

    fn cadence(&self, field: &str, cx: &SourceCx) -> Option<Cadence> {
        match field {
            "" | "index" | "current" => slide_cadence(interval(cx)),
            _ => None,
        }
    }

    fn watched_paths(&self, cx: &SourceCx) -> Vec<PathBuf> {
        let folder = text(cx, "folder");
        if folder.is_empty() { vec![] } else { vec![PathBuf::from(folder)] }
    }

    fn invalidate(&self) {
        self.listings.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    fn act(&self, verb: &str, arg: &str, cx: &SourceCx) -> bool {
        let id = &cx.cfg.id;
        match verb {
            "next" | "prev" => {
                *self.offsets.lock().unwrap_or_else(|e| e.into_inner()).entry(id.clone()).or_default() += if verb == "next" { 1 } else { -1 };
                if let Some(n) = self.notifier() {
                    n.changed_for(id);
                }
            }
            "drop" => {
                let sets = dropped(Path::new(arg.trim()), cx.params.contains_key("path"));
                if let Some(n) = self.notifier() {
                    if sets.is_empty() {
                        n.log(format!("{id}: nothing to show in `{}`", arg.trim()));
                    }
                    for (param, v) in sets {
                        n.set_param(id, param, Value::Str(v));
                    }
                }
            }
            "refresh" => {
                self.invalidate();
                if let Some(n) = self.notifier() {
                    n.changed_for(id);
                }
            }
            _ => return false,
        }
        true
    }

    fn retain(&self, live: &BTreeSet<String>) {
        self.offsets.lock().unwrap_or_else(|e| e.into_inner()).retain(|id, _| live.contains(id));
    }

    fn attach(&self, notify: Notifier) {
        *self.notify.lock().unwrap_or_else(|e| e.into_inner()) = Some(notify);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::InstanceCfg;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wf-gallery-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn tm(h: u32, m: u32, s: u32) -> Tm {
        Tm { year: 2026, month: 10, day: 1, dow: 4, hour: h, minute: m, second: s, ms: 0 }
    }

    fn cfg(params: serde_json::Value) -> InstanceCfg {
        let mut c = InstanceCfg { id: "photo_frame-1".into(), ..Default::default() };
        c.params = serde_json::from_value(params).unwrap();
        c
    }

    fn read(g: &Gallery, c: &InstanceCfg, t: Tm) -> Value {
        let params = c.params_map();
        g.value(&SourceCx { cfg: c, params: &params, tm: t, icon_pack: "Default" })
    }

    fn names(v: &Value) -> Vec<String> {
        match v.get("items") {
            Some(Value::List(l)) => l.iter().map(|i| i.get("name").unwrap().to_string()).collect(),
            _ => panic!("no items"),
        }
    }

    #[test]
    fn only_pictures_sorted_by_name_with_full_paths() {
        let d = tmp("list");
        for n in ["b.PNG", "a.jpg", "notes.txt", "c.webp", ".hidden.png"] {
            std::fs::write(d.join(n), b"x").unwrap();
        }
        std::fs::create_dir_all(d.join("sub.png")).unwrap(); // a folder named like a picture
        let l = list(&d.to_string_lossy(), "name", "all");
        assert_eq!(l.items.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(PathBuf::from(&l.items[0].path), d.join("a.jpg"));
        assert_eq!(l.items[1].ext, "png", "extensions are lower-cased");
        assert!(!l.truncated && l.error.is_empty());
        let moving = list(&d.to_string_lossy(), "name", "animated");
        assert_eq!(moving.items.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["b", "c"], "a JPEG never moves");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn newest_and_oldest_order_by_time_and_a_missing_folder_says_so() {
        let mk = |n: &str, t| Picture { name: n.into(), path: n.into(), ext: "png".into(), size: 0, modified_ms: t };
        let mut v = vec![mk("a", 5), mk("b", 9), mk("c", 1)];
        sort(&mut v, "newest");
        assert_eq!(v.iter().map(|p| p.name.as_str()).collect::<String>(), "bac");
        sort(&mut v, "oldest");
        assert_eq!(v.iter().map(|p| p.name.as_str()).collect::<String>(), "cab");
        let gone = list(&tmp("gone").join("nope").to_string_lossy(), "name", "all");
        assert!(gone.items.is_empty() && !gone.error.is_empty());
    }

    #[test]
    fn slides_wrap_both_ways_and_a_shuffle_shows_each_picture_once_a_round() {
        assert_eq!((0..5).map(|p| slide(p, 3, false, 0)).collect::<Vec<_>>(), [0, 1, 2, 0, 1]);
        assert_eq!(slide(-1, 3, false, 0), 2, "previous from the first is the last");
        for round in 0..4 {
            let mut seen: Vec<usize> = (0..10).map(|k| slide(round * 10 + k, 10, true, 42)).collect();
            assert_eq!(slide(round * 10, 10, true, 42), seen[0], "the same every time");
            seen.sort();
            assert_eq!(seen, (0..10).collect::<Vec<_>>());
        }
        let first: Vec<usize> = (0..10).map(|k| slide(k, 10, true, 42)).collect();
        let second: Vec<usize> = (10..20).map(|k| slide(k, 10, true, 42)).collect();
        assert_ne!(first, second, "each round is shuffled anew");
    }

    #[test]
    fn a_frame_moves_on_with_the_clock_and_by_hand() {
        let d = tmp("frame");
        for n in ["a.jpg", "b.jpg", "c.jpg"] {
            std::fs::write(d.join(n), b"x").unwrap();
        }
        let g = Gallery::default();
        let c = cfg(serde_json::json!({ "folder": d.to_string_lossy(), "interval": 60 }));
        let at = |t| read(&g, &c, t).get("current").and_then(|p| p.get("name")).map(|n| n.to_string()).unwrap();
        let (now, later) = (at(tm(10, 0, 0)), at(tm(10, 1, 0)));
        assert_ne!(now, later, "a minute later is the next picture");
        assert_eq!(at(tm(10, 0, 59)), now, "not before the minute is up");
        let params = c.params_map();
        let cx = SourceCx { cfg: &c, params: &params, tm: tm(10, 0, 0), icon_pack: "Default" };
        assert!(g.act("next", "", &cx));
        assert_eq!(at(tm(10, 0, 0)), later, "next shows what the clock would show next");
        assert!(g.act("prev", "", &cx) && g.act("prev", "", &cx));
        assert_eq!(read(&g, &c, tm(10, 1, 0)).get("current").and_then(|p| p.get("name")).map(|n| n.to_string()), Some(now));
        assert_eq!((g.cadence("current", &cx), g.cadence("items", &cx)), (Some(Cadence::Minute), None), "the list itself waits for the watcher");
        g.retain(&BTreeSet::new());
        assert!(g.offsets.lock().unwrap().is_empty(), "a removed widget's steps go");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn no_folder_is_an_empty_gallery_and_the_folder_is_watched_and_listed_again_on_change() {
        let g = Gallery::default();
        let none = read(&g, &cfg(serde_json::json!({})), tm(9, 0, 0));
        assert_eq!((none.get("count"), none.get("index"), none.get("current")), (Some(&Value::Num(0.0)), Some(&Value::Num(-1.0)), Some(&Value::Nil)));
        let d = tmp("watch");
        std::fs::write(d.join("a.png"), b"x").unwrap();
        let c = cfg(serde_json::json!({ "folder": d.to_string_lossy() }));
        assert_eq!(names(&read(&g, &c, tm(9, 0, 0))), ["a"]);
        let params = c.params_map();
        assert_eq!(g.watched_paths(&SourceCx { cfg: &c, params: &params, tm: tm(9, 0, 0), icon_pack: "Default" }), vec![d.clone()]);
        std::fs::write(d.join("b.png"), b"x").unwrap();
        assert_eq!(names(&read(&g, &c, tm(9, 0, 0))), ["a"], "cached until the watcher says otherwise");
        g.path_changed(&d);
        assert_eq!(names(&read(&g, &c, tm(9, 0, 0))), ["a", "b"]);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_dropped_folder_is_the_folder_and_a_dropped_file_its_path_or_its_folder() {
        let d = tmp("drop");
        let f = d.join("cat.gif");
        std::fs::write(&f, b"x").unwrap();
        let s = |p: &Path| p.to_string_lossy().into_owned();
        assert_eq!(dropped(&d, false), [("folder", s(&d))]);
        assert_eq!(dropped(&d, true), [("folder", s(&d)), ("path", String::new())], "a folder wins over a single file");
        assert_eq!(dropped(&f, true), [("path", s(&f))]);
        assert_eq!(dropped(&f, false), [("folder", s(&d))], "a photo dropped on a gallery picks its folder");
        assert!(dropped(&d.join("nope"), false).is_empty());
        assert_eq!((slide_cadence(0), slide_cadence(30), slide_cadence(300), slide_cadence(15)), (None, Some(Cadence::TenSecond), Some(Cadence::Minute), Some(Cadence::Second)));
        std::fs::remove_dir_all(&d).ok();
    }
}
