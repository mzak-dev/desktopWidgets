//! Icon sourcing (decision 22): explicit path -> Icon Pack by app name -> the
//! target's own icon (from the Ambient's `IconSource`) -> a generic one. Each image id is
//! decoded once; the store asks the renderer to upload it with an `ImageOp` and keeps only
//! its size, never a device.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::ambient::{IconSource, TileIcons};
use crate::images::{Decoded, ImageOp, load_image};
use crate::shortcut::ID_SEP;

pub const GENERIC: &str = "icon:generic";

/// A neutral rounded tile for anything with no icon at all.
pub fn generic() -> Decoded {
    let n = 48u32;
    let mut px = vec![0u8; (n * n * 4) as usize];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 + 0.5 - 24.0, y as f32 + 0.5 - 24.0);
            let q = (fx.abs() - 19.0 + 8.0, fy.abs() - 19.0 + 8.0);
            let d = (q.0.max(0.0).powi(2) + q.1.max(0.0).powi(2)).sqrt() + q.0.max(q.1).min(0.0) - 8.0;
            let cover = (0.5 - d).clamp(0.0, 1.0);
            let ring = (1.0 - (d + 1.5).abs()).clamp(0.0, 1.0);
            let dot = (0.5 - ((fx * fx + fy * fy).sqrt() - 6.0)).clamp(0.0, 1.0);
            let a = (cover * 0.22 + ring * 0.45 + dot * 0.55).min(1.0);
            let i = ((y * n + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&[235, 240, 255, (a * 255.0) as u8]);
        }
    }
    Decoded { px, w: n, h: n, frames: None }
}

/// Icon Pack lookup: `<pack>/<name>.png` where name is the target's file
/// stem, lowercased (`Chrome.lnk` -> `chrome.png`), or its full file name.
fn from_pack(pack_dir: &Path, target: &str) -> Option<Decoded> {
    let stem = crate::shortcut::file_stem(target).to_lowercase();
    let file = Path::new(target).file_name()?.to_string_lossy().to_lowercase();
    [format!("{stem}.png"), format!("{file}.png")].iter().find_map(|n| load_image(&pack_dir.join(n)))
}

pub fn resolve(target: &str, explicit: &str, pack_dir: Option<&Path>, source: &dyn IconSource) -> Decoded {
    if !explicit.is_empty() {
        if let Some(i) = load_image(Path::new(explicit)) {
            return i;
        }
    }
    if let Some(i) = pack_dir.and_then(|d| from_pack(d, target)) {
        return i;
    }
    source.resolve_path(target).and_then(|p| source.shell_icon(&p)).unwrap_or_else(generic)
}

/// A `file:` image, or an app icon that may come from an Icon Pack.
fn from_files(id: &str) -> bool {
    id.starts_with("file:") || id.starts_with(crate::thumbs::PREFIX) || id.strip_prefix("icon:").and_then(|rest| rest.split_once(ID_SEP)).is_some_and(|(pack, _)| !matches!(pack, "Default" | ""))
}

/// Images no window draws that stay on the GPU, so a gallery flipping between a few
/// pictures doesn't decode them again. Past this, the longest unused go first.
pub const IDLE_BUDGET: u64 = 64 << 20;

/// Which uploaded images no window draws any more, oldest first.
#[derive(Default)]
pub struct Residency {
    idle: Vec<(String, u64)>,
}

impl Residency {
    /// `loaded` is every image on the GPU with its size, `drawn` what some window draws now.
    /// Returns the images to drop.
    pub fn settle<'a>(&mut self, loaded: impl IntoIterator<Item = (&'a str, u64)>, drawn: &HashSet<&str>, budget: u64) -> Vec<String> {
        let loaded: BTreeMap<&str, u64> = loaded.into_iter().collect();
        self.idle.retain(|(id, _)| loaded.contains_key(id.as_str()) && !drawn.contains(id.as_str()));
        for (id, bytes) in &loaded {
            if !drawn.contains(id) && !self.idle.iter().any(|(i, _)| i == id) {
                self.idle.push((id.to_string(), *bytes));
            }
        }
        let mut total: u64 = self.idle.iter().map(|(_, b)| b).sum();
        let mut out = vec![];
        while total > budget && !self.idle.is_empty() {
            let (id, bytes) = self.idle.remove(0);
            total -= bytes;
            out.push(id);
        }
        out
    }

    fn clear(&mut self) {
        self.idle.clear();
    }
}

/// What the renderer holds of an image: its layout size (one frame's, for an animation)
/// and its texture's size in memory.
struct Loaded {
    size: (u32, u32),
    bytes: u64,
}

/// Decodes images on demand and remembers which ids the renderer has. It never touches a
/// device: what to upload or drop is queued as `ImageOp`s, and `Gpu::apply` takes them
/// (`drain`) once before each render.
pub struct ImageStore {
    /// Where a target's own icon comes from.
    icons: Arc<dyn IconSource>,
    /// Icon Pack name to its folder, from every content root.
    packs: BTreeMap<String, PathBuf>,
    seen: HashSet<String>,
    /// Every id whose upload is queued or done, and not dropped since.
    loaded: HashMap<String, Loaded>,
    ops: Vec<ImageOp>,
    residency: Residency,
    /// `thumb:` images, made off the UI thread.
    thumbs: crate::thumbs::Thumbs,
}

/// Tile icons and no Icon Packs: what a test or a headless render wants.
impl Default for ImageStore {
    fn default() -> Self {
        Self::new(Arc::new(TileIcons), BTreeMap::new())
    }
}

impl ImageStore {
    pub fn new(icons: Arc<dyn IconSource>, packs: BTreeMap<String, PathBuf>) -> Self {
        Self { icons, packs, seen: HashSet::new(), loaded: HashMap::new(), ops: Vec::new(), residency: Residency::default(), thumbs: crate::thumbs::Thumbs::default() }
    }

    pub fn set_packs(&mut self, packs: BTreeMap<String, PathBuf>) {
        self.packs = packs;
    }

    /// Where small copies of big pictures are kept (`<data>/.cache/thumbs`).
    pub fn set_cache(&mut self, dir: PathBuf) {
        self.thumbs.set_cache(dir);
    }

    /// Called from another thread when a `thumb:` image is ready: then call `take_ready`.
    pub fn set_waker(&self, wake: std::sync::Arc<dyn Fn() + Send + Sync>) {
        self.thumbs.set_waker(wake);
    }

    /// An image's layout size, once `ensure` has decoded it: what `Widget::build` lays an
    /// `image` element out at.
    pub fn size(&self, id: &str) -> Option<(u32, u32)> {
        self.loaded.get(id).map(|l| l.size)
    }

    /// The queued uploads and drops, oldest first. The renderer applies them in this order.
    pub fn drain(&mut self) -> Vec<ImageOp> {
        std::mem::take(&mut self.ops)
    }

    fn upload(&mut self, id: &str, d: Decoded) {
        self.loaded.insert(id.to_string(), Loaded { size: d.size(), bytes: d.w as u64 * d.h as u64 * 4 });
        self.ops.push(ImageOp::Upload(id.to_string(), d));
    }

    fn drop_image(&mut self, id: &str) {
        if self.loaded.remove(id).is_some() {
            self.ops.push(ImageOp::Drop(id.to_string()));
        }
    }

    /// Queues the `thumb:` images made since last asked and returns their ids.
    pub fn take_ready(&mut self) -> Vec<String> {
        let ready = self.thumbs.take();
        let mut ids = Vec::with_capacity(ready.len());
        for (id, d) in ready {
            self.upload(&id, d.unwrap_or_else(generic));
            self.seen.insert(id.clone());
            ids.push(id);
        }
        ids
    }

    /// Whether `thumb:` images are still being made.
    pub fn pending(&self) -> bool {
        self.thumbs.has_pending()
    }

    /// Make sure `id` is decoded and queued for upload. Returns true when a new size became
    /// known. A `thumb:` image is only queued here; `take_ready` uploads it.
    pub fn ensure(&mut self, id: &str) -> bool {
        if id.is_empty() || self.loaded.contains_key(id) {
            return false;
        }
        if id.starts_with(crate::thumbs::PREFIX) {
            self.thumbs.request(id);
            return false;
        }
        if !self.seen.insert(id.to_string()) && self.loaded.contains_key(GENERIC) {
            return false; // tried already; fall back to generic below
        }
        let img = if id == GENERIC {
            generic()
        } else if let Some(path) = id.strip_prefix("file:") {
            match crate::images::decode_file(Path::new(path)) {
                Some(d) => {
                    self.upload(id, d);
                    return true;
                }
                None => generic(),
            }
        } else if let Some(rest) = id.strip_prefix("icon:") {
            let mut it = rest.split(ID_SEP);
            let (pack, target, explicit) = (it.next().unwrap_or(""), it.next().unwrap_or(""), it.next().unwrap_or(""));
            resolve(target, explicit, self.packs.get(pack).map(PathBuf::as_path), self.icons.as_ref())
        } else {
            generic()
        };
        self.upload(id, img);
        true
    }

    /// Content was reloaded: images read from files (and Icon Packs) may have changed.
    /// The system's own icons are kept; extracting them again is slow.
    pub fn flush_files(&mut self) {
        let stale: Vec<String> = self.seen.iter().filter(|id| from_files(id)).cloned().collect();
        for id in stale {
            self.drop_image(&id);
            self.seen.remove(&id);
        }
    }

    /// Drops images no window has drawn for a while (see `IDLE_BUDGET`). `drawn` is what
    /// every open window draws now; the generic icon always stays.
    pub fn release_unused<'a>(&mut self, drawn: impl IntoIterator<Item = &'a str>) {
        let drawn: HashSet<&str> = drawn.into_iter().collect();
        let loaded = self.seen.iter().filter(|id| id.as_str() != GENERIC).filter_map(|id| Some((id.as_str(), self.loaded.get(id)?.bytes)));
        for id in self.residency.settle(loaded, &drawn, IDLE_BUDGET) {
            self.drop_image(&id);
            self.seen.remove(&id);
        }
    }

    /// The GPU was rebuilt: nothing is uploaded any more, and what was queued for it is moot.
    pub fn forget(&mut self) {
        self.seen.clear();
        self.loaded.clear();
        self.ops.clear();
        self.residency.clear();
    }

    /// Forget everything (icon pack changed, so ids are new anyway, but this frees the memory).
    pub fn reset(&mut self, ids: impl IntoIterator<Item = String>) {
        for id in ids {
            self.drop_image(&id);
        }
        self.seen.clear();
        self.residency.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_file_and_pack_images_are_flushed_on_reload() {
        let icon = |pack: &str| format!("icon:{pack}{ID_SEP}C:\\app.exe{ID_SEP}");
        assert!(from_files("file:C:\\x.png") && from_files("thumb:256:C:\\x.png"));
        assert!(from_files(&icon("Neon")));
        assert!(!from_files(&icon("Default")), "a shell icon is expensive to extract again");
        assert!(!from_files(GENERIC));
    }

    #[test]
    fn unused_images_stay_within_a_budget_then_go_oldest_first() {
        let mut r = Residency::default();
        let set = |ids: &[&'static str]| ids.iter().copied().collect::<HashSet<&str>>();
        let loaded = |ids: &[&'static str]| ids.iter().map(|id| (*id, 10u64)).collect::<Vec<_>>();
        assert!(r.settle(loaded(&["a", "b", "c"]), &set(&["a", "b", "c"]), 25).is_empty(), "all drawn");
        assert!(r.settle(loaded(&["a", "b", "c"]), &set(&["c"]), 25).is_empty(), "a and b idle, 20 bytes fit");
        assert!(r.settle(loaded(&["a", "b", "c", "d"]), &set(&["b", "d"]), 25).is_empty(), "b drawn again, a and c idle");
        assert_eq!(r.settle(loaded(&["a", "b", "c", "d"]), &set(&[]), 30), ["a"], "a idled first, so it goes first");
        assert_eq!(r.settle(loaded(&["b", "c", "d"]), &set(&[]), 0), ["c", "b", "d"], "then in the order they idled");
        assert!(r.settle(loaded(&["e"]), &set(&[]), 25).is_empty() && r.idle.len() == 1, "images no longer loaded are forgotten");
    }

    #[test]
    fn generic_icon_is_a_centred_shape_not_a_blank_square() {
        let g = generic();
        assert_eq!((g.w, g.h, g.px.len()), (48, 48, 48 * 48 * 4));
        let alpha = |x: usize, y: usize| g.px[(y * 48 + x) * 4 + 3];
        assert_eq!(alpha(0, 0), 0, "corner is transparent");
        assert!(alpha(24, 24) > 100, "centre dot is visible");
    }

    fn describe(ops: Vec<ImageOp>) -> Vec<String> {
        let name = |id: &str| Path::new(id).file_name().unwrap().to_string_lossy().into_owned();
        ops.into_iter().map(|op| match op {
            ImageOp::Upload(id, d) => format!("up {} {}x{}", name(&id), d.w, d.h),
            ImageOp::Drop(id) => format!("drop {}", name(&id)),
        }).collect()
    }

    #[test]
    fn the_store_queues_uploads_and_drops_in_the_order_it_decided_them() {
        let dir = std::env::temp_dir().join(format!("wf-ops-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = |name: &str, w: u32, h: u32| {
            let p = dir.join(name);
            image::RgbaImage::new(w, h).save(&p).unwrap();
            format!("file:{}", p.display())
        };
        let (a, b) = (file("a.png", 4, 3), file("b.png", 8, 2));
        let mut s = ImageStore::default();
        assert!(s.ensure(&a) && s.ensure(&b) && !s.ensure(&a), "a known id is not decoded again");
        assert_eq!((s.size(&a), s.size(&b), s.size("file:nope")), (Some((4, 3)), Some((8, 2)), None));
        assert_eq!(describe(s.drain()), ["up a.png 4x3", "up b.png 8x2"]);
        assert!(s.drain().is_empty(), "drained once");

        // content reloaded: the files may have changed, so the next ensure decodes again, after the drops
        s.flush_files();
        assert_eq!(s.size(&a), None);
        s.ensure(&a);
        let dropped = describe(s.drain());
        assert_eq!(dropped.len(), 3);
        assert!(dropped[..2].contains(&"drop a.png".to_string()) && dropped[..2].contains(&"drop b.png".to_string()));
        assert_eq!(dropped[2], "up a.png 4x3", "the re-upload comes after the drop of the same id");

        // the GPU was rebuilt: what was queued for the old one is moot, and everything is decoded again
        s.ensure(&b);
        s.forget();
        assert!(s.drain().is_empty() && s.size(&a).is_none());
        assert!(s.ensure(&a));
        assert_eq!(describe(s.drain()), ["up a.png 4x3"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unreadable_image_is_stored_as_the_generic_tile_once() {
        let mut s = ImageStore::default();
        assert!(s.ensure("file:/no/such/x.png"));
        assert_eq!(s.size("file:/no/such/x.png"), Some((48, 48)));
        assert!(!s.ensure("file:/no/such/x.png"));
        assert_eq!(describe(s.drain()), ["up x.png 48x48"]);
    }

    #[test]
    fn an_animation_lays_out_at_one_frames_size() {
        let mut s = ImageStore::default();
        let frames = crate::images::Frames { cols: 2, rows: 1, count: 2, frame_w: 10, frame_h: 6, delays_ms: vec![100, 100], total_ms: 200 };
        s.upload("gif", Decoded { px: vec![0; 20 * 6 * 4], w: 20, h: 6, frames: Some(frames) });
        assert_eq!(s.size("gif"), Some((10, 6)));
        assert_eq!(s.loaded["gif"].bytes, 20 * 6 * 4);
    }

    #[test]
    fn fallback_chain_reaches_each_step_in_order() {
        let dir = std::env::temp_dir().join(format!("wf-icons-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // step 2: an Icon Pack entry beats the target's own icon
        let mut px = image::RgbaImage::new(8, 8);
        px.put_pixel(0, 0, image::Rgba([1, 2, 3, 255]));
        px.save(dir.join("notepad.png")).unwrap();
        let hit = resolve(r"C:\Windows\notepad.exe", "", Some(&dir), &TileIcons);
        assert_eq!((hit.w, hit.px[0]), (8, 1), "pack icon used");
        // step 1: an explicit path beats the pack
        let explicit = dir.join("mine.png");
        image::RgbaImage::new(4, 4).save(&explicit).unwrap();
        assert_eq!(resolve("notepad", explicit.to_str().unwrap(), Some(&dir), &TileIcons).w, 4);
        // step 3: no pack entry -> the source's icon for what the target names
        let tile = |target: &str| resolve(target, "", None, &TileIcons);
        let (notepad, calc) = (tile(r"C:\Windows\notepad.exe"), tile(r"C:\Windows\calc.exe"));
        assert_eq!((notepad.w, notepad.h), (48, 48));
        assert_ne!(notepad.px, generic().px, "a tile, not the generic one");
        assert_ne!(notepad.px, calc.px, "two apps look different");
        assert_eq!(notepad.px, tile(r"D:\Other\NOTEPAD.lnk").px, "the colour follows the file stem");
        // step 4: nothing resolvable -> generic
        assert_eq!(tile("https://example.com/app").px, generic().px);
        assert_eq!(tile("").px, generic().px);
        std::fs::remove_dir_all(&dir).ok();
    }
}
