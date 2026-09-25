//! Small copies of big pictures, made off the UI thread: an `image` with `max = 256`. A grid
//! of photos would otherwise decode each 24-megapixel original on the UI thread and upload
//! it whole, about 96 MB of texture each. The small copy of a still is kept on disk under
//! `<data>/.cache/thumbs`, keyed by the file's path, size and time and by `max`, so the
//! next start reads small PNGs instead of originals.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::images::{self, Decoded};

pub const PREFIX: &str = "thumb:";
const WORKERS: usize = 2;
/// The disk cache is trimmed to this, oldest first, when the workers start.
const DISK_BUDGET: u64 = 256 << 20;

/// The image id of `path` shrunk to fit `max`.
pub fn id(max: u32, path: &str) -> String {
    format!("{PREFIX}{max}:{path}")
}

/// `thumb:256:C:\x.jpg` is (256, `C:\x.jpg`).
pub fn parse(id: &str) -> Option<(u32, PathBuf)> {
    let (max, path) = id.strip_prefix(PREFIX)?.split_once(':')?;
    Some((max.parse().ok()?, PathBuf::from(path)))
}

/// Where the small copy of `path` at `max` is kept; a changed file gets a new name.
fn cache_file(dir: &Path, path: &Path, max: u32) -> Option<PathBuf> {
    let m = std::fs::metadata(path).ok()?;
    let time = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let key = format!("{}|{}|{time}|{max}", path.to_string_lossy().to_lowercase(), m.len());
    let h = key.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    Some(dir.join(format!("{h:016x}.png")))
}

/// `path` decoded and shrunk to fit `max`, from the disk cache when it has it. Animations
/// keep their frames and are not cached.
pub fn make(path: &Path, max: u32, dir: Option<&Path>) -> Option<Decoded> {
    let cached = dir.and_then(|d| cache_file(d, path, max));
    if let Some(d) = cached.as_ref().and_then(|c| images::decode_file(c)) {
        return Some(d);
    }
    let small = images::decode_file(path)?.fit(max);
    if let (Some(c), None) = (&cached, &small.frames) {
        if let Some(img) = image::RgbaImage::from_raw(small.w, small.h, small.px.clone()) {
            let part = c.with_extension("part");
            let saved = c.parent().is_some_and(|p| std::fs::create_dir_all(p).is_ok()) && img.save_with_format(&part, image::ImageFormat::Png).is_ok() && std::fs::rename(&part, c).is_ok();
            if !saved {
                let _ = std::fs::remove_file(&part);
            }
        }
    }
    Some(small)
}

/// Deletes the oldest cached copies until the rest fit in `budget` bytes.
pub fn prune(dir: &Path, budget: u64) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = rd.flatten().filter_map(|e| {
        let m = e.metadata().ok()?;
        Some((m.modified().ok()?, m.len(), e.path()))
    }).collect();
    files.sort();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    for (_, len, p) in files {
        if total <= budget {
            break;
        }
        if std::fs::remove_file(&p).is_ok() {
            total -= len;
        }
    }
}

type Waker = Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>;
type Done = Arc<Mutex<Vec<(String, Option<Decoded>)>>>;

/// The queue of small copies being made. Workers start at the first request.
#[derive(Default)]
pub struct Thumbs {
    cache: Option<PathBuf>,
    jobs: Option<Sender<String>>,
    done: Done,
    pending: HashSet<String>,
    wake: Waker,
}

impl Thumbs {
    /// Where small copies are kept; set before the first request.
    pub fn set_cache(&mut self, dir: PathBuf) {
        self.cache = Some(dir);
    }

    /// Called from a worker whenever a copy is ready to upload.
    pub fn set_waker(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        *self.wake.lock().unwrap() = Some(wake);
    }

    pub fn is_pending(&self, id: &str) -> bool {
        self.pending.contains(id)
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Queues `id` (a `thumb:` id) once.
    pub fn request(&mut self, id: &str) {
        if !self.pending.insert(id.to_string()) {
            return;
        }
        let tx = self.jobs.get_or_insert_with(|| {
            let (tx, rx) = mpsc::channel::<String>();
            let rx = Arc::new(Mutex::new(rx));
            if let Some(dir) = self.cache.clone() {
                std::thread::spawn(move || prune(&dir, DISK_BUDGET));
            }
            for n in 0..WORKERS {
                let (rx, done, wake, cache) = (rx.clone(), self.done.clone(), self.wake.clone(), self.cache.clone());
                let _ = std::thread::Builder::new().name(format!("thumbs {n}")).spawn(move || work(&rx, &done, &wake, cache.as_deref()));
            }
            tx
        });
        let _ = tx.send(id.to_string());
    }

    /// The copies made since last asked; `None` for a file that could not be read.
    pub fn take(&mut self) -> Vec<(String, Option<Decoded>)> {
        let out = std::mem::take(&mut *self.done.lock().unwrap());
        for (id, _) in &out {
            self.pending.remove(id);
        }
        out
    }
}

fn work(rx: &Mutex<Receiver<String>>, done: &Mutex<Vec<(String, Option<Decoded>)>>, wake: &Waker, cache: Option<&Path>) {
    loop {
        let Ok(id) = rx.lock().unwrap().recv() else { return };
        let d = parse(&id).and_then(|(max, path)| make(&path, max, cache));
        done.lock().unwrap().push((id, d));
        if let Some(w) = wake.lock().unwrap().clone() {
            w();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wf-thumbs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn ids_carry_the_size_and_the_path() {
        let i = id(256, "C:\\Photos\\a.jpg");
        assert_eq!(i, "thumb:256:C:\\Photos\\a.jpg");
        assert_eq!(parse(&i), Some((256, PathBuf::from("C:\\Photos\\a.jpg"))));
        assert_eq!(parse("file:C:\\a.jpg"), None);
    }

    #[test]
    fn a_small_copy_is_made_once_and_kept() {
        let dir = tmp("make");
        let photo = dir.join("photo.png");
        image::RgbaImage::from_pixel(800, 600, image::Rgba([10, 20, 30, 255])).save(&photo).unwrap();
        let cache = dir.join("cache");
        let d = make(&photo, 200, Some(&cache)).unwrap();
        assert_eq!((d.w, d.h), (200, 150));
        let kept = cache_file(&cache, &photo, 200).unwrap();
        assert!(kept.is_file(), "kept on disk");
        assert_ne!(Some(kept.clone()), cache_file(&cache, &photo, 100), "another size is another copy");
        assert_eq!(images::decode_file(&kept).map(|k| (k.w, k.h)), Some((200, 150)));
        assert!(make(&dir.join("missing.png"), 200, Some(&cache)).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn copies_are_made_off_thread_and_wake_the_app() {
        let dir = tmp("queue");
        let photo = dir.join("p.png");
        image::RgbaImage::new(300, 300).save(&photo).unwrap();
        let mut t = Thumbs::default();
        t.set_cache(dir.join("cache"));
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        t.set_waker(Arc::new(move || {
            let _ = tx.lock().unwrap().send(());
        }));
        let (a, b) = (id(64, &photo.to_string_lossy()), id(64, &dir.join("nope.png").to_string_lossy()));
        t.request(&a);
        t.request(&a);
        t.request(&b);
        assert!(t.is_pending(&a) && t.has_pending());
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut got = vec![];
        while got.len() < 2 && Instant::now() < deadline {
            let _ = rx.recv_timeout(Duration::from_millis(200));
            got.extend(t.take());
        }
        got.sort_by(|x, y| x.0.cmp(&y.0));
        assert_eq!(got.len(), 2, "one each, the duplicate request dropped");
        assert_eq!(got.iter().find(|g| g.0 == a).and_then(|g| g.1.as_ref()).map(|d| (d.w, d.h)), Some((64, 64)));
        assert!(got.iter().find(|g| g.0 == b).is_some_and(|g| g.1.is_none()), "an unreadable file comes back empty");
        assert!(!t.has_pending());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_disk_cache_keeps_within_its_budget_newest_first() {
        let dir = tmp("prune");
        for i in 0..4 {
            std::fs::write(dir.join(format!("{i}.png")), vec![0u8; 100]).unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        prune(&dir, 250);
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["2.png", "3.png"]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
