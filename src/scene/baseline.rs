//! Dump baselines: the blessed text dump of each scene, kept beside the font environment it
//! was made in.
//!
//! There are no bundled fonts, so a dump's text metrics (and every rect that depends on them)
//! are exact only on the machine and font set that made it. A baseline is therefore filed
//! under a folder named for the font hash of its header:
//!
//! ```text
//! <scene root>/baselines/                  untracked (.gitignore), per machine
//!   system-297b4419/                       the font set the baselines inside were made with
//!     baseline.json                        format, fonts, face count, engine version
//!     bless.log                            one line per bless: scene, reason, what changed
//!     <scene id>.dump.txt                  the full dump text, as `scene dump --view full` writes it
//!     <scene id>.png                       golden scenes only: the render, over the backdrop
//!     <scene id>.env.json                  golden scenes only: the record of the render (the
//!                                          fingerprint a later render is compared under)
//! ```
//!
//! PNG baselines are as per machine as the dumps (the glyphs are the machine's, and WARP ships
//! with Windows), and untracked like them.
//!
//! A run looks only in the folder of its own font hash. A scene that has a baseline only under
//! other hashes is not diffed at all (`Found::Elsewhere`): comparing across font sets would
//! report text metrics as geometry changes. A team whose machines share a font set can commit
//! the folder of that hash (remove `scenes/baselines/` from `.gitignore`); anyone else's
//! machine then finds nothing under its own hash and says so instead of reporting false diffs.

use std::path::{Path, PathBuf};

use serde_json::json;

use super::diff::{Diff, diff};
use super::dump::SceneDump;

/// The folder under a scene root that holds baselines.
pub const DIR: &str = "baselines";

/// Where one scene root keeps its baselines.
#[derive(Clone, Debug, PartialEq)]
pub struct Store {
    dir: PathBuf,
}

/// What a scene's baseline lookup found.
#[derive(Debug, PartialEq)]
pub enum Found {
    /// The baseline text, made with this run's font set.
    Here(String),
    /// No baseline under this font set, but one under these (their `fonts=` values).
    Elsewhere(Vec<String>),
    /// No baseline at all.
    Missing,
}

/// The folder name of a font hash: `system:297b4419` is `system-297b4419`.
fn folder_of(fonts: &str) -> String {
    fonts.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect()
}

fn normalised(text: String) -> String {
    text.replace("\r\n", "\n")
}

impl Store {
    /// The baselines of the scenes under `root`, or of the folder `over` names.
    pub fn new(root: &Path, over: Option<&Path>) -> Store {
        Store { dir: over.map_or_else(|| root.join(DIR), Path::to_path_buf) }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The folder this font set's baselines are in.
    pub fn fonts_dir(&self, fonts: &str) -> PathBuf {
        self.dir.join(folder_of(fonts))
    }

    pub fn path(&self, fonts: &str, id: &str) -> PathBuf {
        self.fonts_dir(fonts).join(format!("{id}.dump.txt"))
    }

    /// The baseline of scene `id` for a run whose font hash is `fonts`.
    pub fn read(&self, fonts: &str, id: &str) -> Found {
        if let Ok(text) = std::fs::read_to_string(self.path(fonts, id)) {
            let text = normalised(text);
            return match SceneDump::fonts_of(&text) {
                Some(f) if f == fonts => Found::Here(text),
                // a file moved into the wrong folder says so in its own header
                Some(f) => Found::Elsewhere(vec![f.to_string()]),
                None => Found::Elsewhere(vec!["(unreadable header)".into()]),
            };
        }
        let mut others = Vec::new();
        let own = folder_of(fonts);
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            let mut dirs: Vec<PathBuf> = rd.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_dir()).collect();
            dirs.sort();
            for d in dirs {
                if d.file_name().is_some_and(|n| n.to_string_lossy() == own) {
                    continue;
                }
                if let Ok(text) = std::fs::read_to_string(d.join(format!("{id}.dump.txt"))) {
                    others.push(SceneDump::fonts_of(&normalised(text)).map_or_else(|| d.file_name().unwrap_or_default().to_string_lossy().into_owned(), str::to_string));
                }
            }
        }
        if others.is_empty() { Found::Missing } else { Found::Elsewhere(others) }
    }

    /// The scene ids that have a baseline under this font set.
    pub fn ids(&self, fonts: &str) -> Vec<String> {
        let base = self.fonts_dir(fonts);
        let mut out = Vec::new();
        let mut todo = vec![base.clone()];
        while let Some(d) = todo.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.filter_map(Result::ok) {
                let p = e.path();
                if p.is_dir() {
                    todo.push(p);
                } else if let Some(id) = p.strip_prefix(&base).ok().and_then(|r| r.to_string_lossy().replace('\\', "/").strip_suffix(".dump.txt").map(str::to_string)) {
                    out.push(id);
                }
            }
        }
        out.sort();
        out
    }

    pub fn png_path(&self, fonts: &str, id: &str) -> PathBuf {
        self.fonts_dir(fonts).join(format!("{id}.png"))
    }

    /// The PNG baseline of scene `id` and the record it was made with, for this font set.
    pub fn read_png(&self, fonts: &str, id: &str) -> Result<Option<(image::RgbaImage, Option<serde_json::Value>)>, String> {
        let path = self.png_path(fonts, id);
        if !path.is_file() {
            return Ok(None);
        }
        let img = image::open(&path).map_err(|e| format!("{}: {e}", path.display()))?.to_rgba8();
        let record = std::fs::read_to_string(self.fonts_dir(fonts).join(format!("{id}.env.json"))).ok().and_then(|t| serde_json::from_str(&t).ok());
        Ok(Some((img, record)))
    }

    /// Writes the PNG baseline of a scene and the record of the render it came from.
    pub fn write_png(&self, fonts: &str, id: &str, img: &image::RgbaImage, record: &serde_json::Value) -> Result<(), String> {
        let path = self.png_path(fonts, id);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        img.save(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let rec = self.fonts_dir(fonts).join(format!("{id}.env.json"));
        std::fs::write(&rec, serde_json::to_string_pretty(record).unwrap_or_default() + "\n").map_err(|e| format!("{}: {e}", rec.display()))
    }

    /// How many scenes have a PNG baseline under this font set.
    pub fn png_count(&self, fonts: &str) -> usize {
        let base = self.fonts_dir(fonts);
        let mut n = 0;
        let mut todo = vec![base];
        while let Some(d) = todo.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.filter_map(Result::ok) {
                let p = e.path();
                if p.is_dir() {
                    todo.push(p);
                } else if p.extension().is_some_and(|x| x == "png") {
                    n += 1;
                }
            }
        }
        n
    }

    /// Writes a baseline and records the font set it belongs to.
    pub fn write(&self, fonts: &str, faces: u64, id: &str, text: &str) -> Result<(), String> {
        let path = self.path(fonts, id);
        let put = |p: &Path, body: &str| -> Result<(), String> {
            if let Some(dir) = p.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            std::fs::write(p, body).map_err(|e| format!("{}: {e}", p.display()))
        };
        put(&path, text)?;
        let manifest = json!({ "format": 1, "fonts": fonts, "faces": faces, "engine": env!("CARGO_PKG_VERSION") });
        put(&self.fonts_dir(fonts).join("baseline.json"), &(serde_json::to_string_pretty(&manifest).unwrap_or_default() + "\n"))
    }

    /// Adds a line to the folder's `bless.log`.
    pub fn log(&self, fonts: &str, line: &str) -> Result<(), String> {
        use std::io::Write;
        let path = self.fonts_dir(fonts).join("bless.log");
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        writeln!(f, "{line}").map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// How a run's dump stands against the baseline of its scene.
#[derive(Debug)]
pub enum Against {
    Same,
    Changed(Diff),
    /// No baseline for the scene (under any font set).
    New,
    /// Baselines exist, but only for other font sets: nothing was compared.
    Fonts(Vec<String>),
    /// The baseline could not be read back as a dump.
    Broken(String),
}

/// Compares `text` (this run's dump) with the baseline of `id` made with the same fonts.
pub fn against(store: &Store, fonts: &str, id: &str, text: &str) -> Against {
    match store.read(fonts, id) {
        Found::Missing => Against::New,
        Found::Elsewhere(f) => Against::Fonts(f),
        Found::Here(base) if base == text => Against::Same,
        Found::Here(base) => match diff(&base, text) {
            Ok(d) => Against::Changed(d),
            Err(e) => Against::Broken(e),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("wf-baseline-{name}-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn dump_with(fonts: &str) -> String {
        let mut d = crate::scene::dump::fixtures::tiny_dump(&["Hi"]);
        d.header.fonts = fonts.to_string();
        d.to_text()
    }

    #[test]
    fn a_baseline_is_filed_under_the_font_hash_it_was_made_with() {
        let root = temp("filed");
        let store = Store::new(&root, None);
        assert_eq!(store.dir(), root.join("baselines"));
        assert_eq!(store.read("system:aaaa1111", "engine/clock"), Found::Missing);
        let text = dump_with("system:aaaa1111");
        store.write("system:aaaa1111", 412, "engine/clock@default#defaults", &text).unwrap();
        assert_eq!(store.read("system:aaaa1111", "engine/clock@default#defaults"), Found::Here(text.clone()));
        assert!(root.join("baselines/system-aaaa1111/engine/clock@default#defaults.dump.txt").is_file());
        let manifest: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(root.join("baselines/system-aaaa1111/baseline.json")).unwrap()).unwrap();
        assert_eq!((manifest["fonts"].as_str(), manifest["faces"].as_u64(), manifest["format"].as_u64()), (Some("system:aaaa1111"), Some(412), Some(1)));
        // another machine's font set finds nothing of its own, but is told there are baselines
        assert_eq!(store.read("system:bbbb2222", "engine/clock@default#defaults"), Found::Elsewhere(vec!["system:aaaa1111".into()]));
        assert_eq!(store.read("system:bbbb2222", "engine/other"), Found::Missing);
        assert_eq!(store.ids("system:aaaa1111"), ["engine/clock@default#defaults"]);
        assert!(store.ids("system:bbbb2222").is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_file_whose_own_header_names_other_fonts_is_never_compared() {
        let root = temp("moved");
        let store = Store::new(&root, None);
        // a baseline copied into the wrong folder: the header is the authority
        std::fs::create_dir_all(root.join("baselines/system-cccc3333")).unwrap();
        std::fs::write(root.join("baselines/system-cccc3333/x.dump.txt"), dump_with("system:dddd4444").replace('\n', "\r\n")).unwrap();
        assert_eq!(store.read("system:cccc3333", "x"), Found::Elsewhere(vec!["system:dddd4444".into()]));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_png_baseline_is_filed_beside_its_dump_with_the_record_of_its_render() {
        let root = temp("png");
        let store = Store::new(&root, None);
        assert!(store.read_png("system:aaaa1111", "golden/clock").unwrap().is_none());
        let img = image::RgbaImage::from_pixel(3, 2, image::Rgba([1, 2, 3, 255]));
        store.write_png("system:aaaa1111", "golden/clock", &img, &serde_json::json!({ "format": 1 })).unwrap();
        let (back, record) = store.read_png("system:aaaa1111", "golden/clock").unwrap().unwrap();
        assert_eq!((back, record), (img, Some(serde_json::json!({ "format": 1 }))));
        assert!(root.join("baselines/system-aaaa1111/golden/clock.png").is_file());
        assert_eq!((store.png_count("system:aaaa1111"), store.png_count("system:bbbb2222")), (1, 0));
        assert!(store.read_png("system:bbbb2222", "golden/clock").unwrap().is_none(), "another font set has none of its own");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn crlf_from_a_checkout_is_read_as_lf_and_the_bless_log_appends() {
        let root = temp("crlf");
        let store = Store::new(&root, Some(&root.join("elsewhere")));
        let text = dump_with("system:eeee5555");
        std::fs::create_dir_all(root.join("elsewhere/system-eeee5555")).unwrap();
        std::fs::write(root.join("elsewhere/system-eeee5555/y.dump.txt"), text.replace('\n', "\r\n")).unwrap();
        assert_eq!(store.read("system:eeee5555", "y"), Found::Here(text));
        store.log("system:eeee5555", "y\tfirst").unwrap();
        store.log("system:eeee5555", "y\tsecond").unwrap();
        assert_eq!(std::fs::read_to_string(root.join("elsewhere/system-eeee5555/bless.log")).unwrap().lines().collect::<Vec<_>>(), ["y\tfirst", "y\tsecond"]);
        let _ = std::fs::remove_dir_all(root);
    }
}
