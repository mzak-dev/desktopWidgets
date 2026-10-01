//! What a render records about the environment it ran in: the sidecar `<png>.env.json`. It
//! says whether the run was hermetic (and if not, why), which Pins were in effect, what
//! content and fonts it used and which adapter drew it, so two images that differ can be
//! explained. Nothing in it changes from run to run on one machine (no times, no temp paths).
//!
//! Fonts are the machine's (there are no bundled fonts), so the font hash says which font
//! set the text metrics belong to: a render is exact on the machine and font set it ran on.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::ambient::{Pins, Seam};
use crate::gfx::AdapterReport;

/// A folder of content the render read, and a hash of it.
#[derive(Clone, Debug, PartialEq)]
pub struct ContentRoot {
    /// `widget-folder` (the folder of a widget file named on the command line), `installed`
    /// (a plugin or the data folder, read only with `--installed`).
    pub kind: &'static str,
    pub path: PathBuf,
    pub hash: String,
}

/// Everything the sidecar is written from.
pub struct Facts<'a> {
    pub widget: &'a str,
    /// The PNG's size in pixels.
    pub size: (u32, u32),
    pub pins: &'a Pins,
    /// The seams taken from the machine: the Pins' `real` plus the modes that imply one.
    pub real: &'a BTreeSet<Seam>,
    pub installed: bool,
    pub roots: &'a [ContentRoot],
    pub faces: &'a [String],
    /// The adapter that drew it; `None` for a run that drew nothing (a scene dump).
    pub adapter: Option<&'a AdapterReport>,
    /// Every data path the widget reads.
    pub deps: &'a BTreeSet<String>,
    pub code_sources: &'a [String],
    /// Frames rendered after the first one to settle (1 without any waiting to do).
    pub rounds: u32,
}

/// FNV-1a over `parts`, each followed by a separator byte, as 16 hex digits.
pub fn fnv(parts: impl IntoIterator<Item = impl AsRef<[u8]>>) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in p.as_ref().iter().copied().chain([0xff]) {
            h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

/// The sidecar beside a PNG: `a.png` gets `a.png.env.json`.
pub fn sidecar_path(png: &Path) -> PathBuf {
    let mut s = png.as_os_str().to_os_string();
    s.push(".env.json");
    s.into()
}

/// Why the run is not hermetic; empty when it is. A hermetic run read only pinned values,
/// built-in content and what the command line named, on a software adapter.
pub fn leaks(f: &Facts) -> Vec<String> {
    let mut out = Vec::new();
    let real: Vec<&str> = f.real.iter().filter(|s| **s != Seam::Gpu).map(|s| s.name()).collect();
    if !real.is_empty() {
        out.push(format!("read from the machine: {}", real.join(", ")));
    }
    if f.installed {
        out.push("installed widgets and plugins were read".into());
    }
    if let Some(a) = f.adapter.filter(|a| !a.software) {
        out.push(format!("drawn on a hardware adapter ({})", a.name));
    }
    if f.deps.iter().any(|d| d.split('.').next() == Some("shortcuts")) {
        out.push("the shortcuts source reads the machine's folders".into());
    }
    out
}

/// The versions in `Cargo.lock` of the crates that decide how pixels and glyphs come out, and
/// a hash of the whole lock file.
fn lock_versions() -> (BTreeMap<String, String>, String) {
    const LOCK: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.lock"));
    let mut found = BTreeMap::new();
    let mut name = None;
    for line in LOCK.lines() {
        if let Some(n) = line.strip_prefix("name = \"").and_then(|n| n.strip_suffix('"')) {
            name = Some(n);
        } else if let (Some(v), Some(n)) = (line.strip_prefix("version = \"").and_then(|v| v.strip_suffix('"')), name.take()) {
            if ["wgpu", "wgpu-core", "wgpu-hal", "naga", "cosmic-text", "swash", "fontdb", "rustybuzz", "glyphon", "taffy", "image", "rustfft"].contains(&n) {
                found.insert(n.to_string(), v.to_string());
            }
        }
    }
    (found, fnv([LOCK.as_bytes()]))
}

/// "10.0.26200.6584" from `ver`, or "unknown". Starting `cmd` can fail while many processes
/// start at once, so it is tried a few times; a console window is never shown.
fn windows_build() -> String {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    for attempt in 0..4 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let out = std::process::Command::new("cmd").args(["/c", "ver"]).creation_flags(CREATE_NO_WINDOW).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        if let Some(v) = out.split_once("Version ").and_then(|(_, r)| r.split_once(']')) {
            return v.0.trim().to_string();
        }
    }
    "unknown".into()
}

/// A hash of the files that make a content folder: their names, sizes and (up to 8 MB each)
/// bytes, in name order.
pub fn hash_files(base: &Path, files: &[PathBuf]) -> String {
    let mut files = files.to_vec();
    files.sort();
    let mut parts: Vec<Vec<u8>> = Vec::new();
    for p in &files {
        let rel = p.strip_prefix(base).unwrap_or(p).to_string_lossy().replace('\\', "/");
        let len = std::fs::metadata(p).map_or(0, |m| m.len());
        parts.push(format!("{rel}:{len}").into_bytes());
        if len <= 8 << 20 {
            parts.push(std::fs::read(p).unwrap_or_default());
        }
    }
    fnv(parts)
}

/// Every file under `dir` but dot-folders and the workspace file, for `hash_files`.
pub fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.filter_map(Result::ok) {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name.starts_with("workspace.json") {
                continue;
            }
            let p = e.path();
            if p.is_dir() {
                todo.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

/// The sidecar as JSON.
pub fn sidecar(f: &Facts) -> Value {
    let because = leaks(f);
    let (versions, lock) = lock_versions();
    let adapter = f.adapter.map(|a| json!({ "name": a.name, "driver": a.driver, "driver_info": a.driver_info, "backend": a.backend, "device_type": a.device_type, "vendor": a.vendor, "device": a.device, "software": a.software }));
    json!({
        "format": 1,
        "hermetic": because.is_empty(),
        "not_hermetic_because": because,
        "widget": f.widget,
        "size_px": [f.size.0, f.size.1],
        "pins": Value::Object(f.pins.entries().into_iter().map(|(k, v)| (k.to_string(), v)).collect()),
        "real": f.real.iter().map(|s| s.name()).collect::<Vec<_>>(),
        "content": {
            "installed": f.installed,
            "roots": f.roots.iter().map(|r| json!({ "kind": r.kind, "path": r.path.to_string_lossy(), "hash": r.hash })).collect::<Vec<_>>(),
        },
        "code_sources": f.code_sources,
        "settle_rounds": f.rounds,
        "fonts": { "mode": "system", "faces": f.faces.len(), "hash": fnv(f.faces) },
        "adapter": adapter,
        "windows": windows_build(),
        "engine": { "wayfinder": env!("CARGO_PKG_VERSION"), "cargo_lock": lock, "crates": versions },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(software: bool) -> AdapterReport {
        AdapterReport { name: "Microsoft Basic Render Driver".into(), driver: String::new(), driver_info: String::new(), backend: "Dx12".into(), device_type: if software { "Cpu" } else { "DiscreteGpu" }.into(), vendor: 0, device: 0, software }
    }

    fn facts<'a>(pins: &'a Pins, real: &'a BTreeSet<Seam>, adapter: &'a AdapterReport, deps: &'a BTreeSet<String>) -> Facts<'a> {
        Facts { widget: "clock", size: (10, 10), pins, real, installed: false, roots: &[], faces: &[], adapter: Some(adapter), deps, code_sources: &[], rounds: 1 }
    }

    #[test]
    fn fnv_is_a_fixed_function_of_its_parts() {
        assert_eq!(fnv(Vec::<&[u8]>::new()), "cbf29ce484222325");
        assert_eq!(fnv(["a"]), fnv(["a"]));
        assert_ne!(fnv(["ab"]), fnv(["a", "b"]), "where a part ends matters");
        assert_eq!(sidecar_path(Path::new("out/a.png")), PathBuf::from("out/a.png.env.json"));
    }

    #[test]
    fn a_run_is_hermetic_until_something_is_taken_from_the_machine() {
        let (pins, none, soft, deps) = (Pins::default(), BTreeSet::new(), adapter(true), BTreeSet::from(["clock.minute".to_string()]));
        assert!(leaks(&facts(&pins, &none, &soft, &deps)).is_empty());
        let real = BTreeSet::from([Seam::Clock, Seam::Sys, Seam::Gpu]);
        assert_eq!(leaks(&facts(&pins, &real, &soft, &deps)), ["read from the machine: clock, sys"]);
        let hard = adapter(false);
        assert!(leaks(&facts(&pins, &none, &hard, &deps))[0].starts_with("drawn on a hardware adapter"));
        let shortcuts = BTreeSet::from(["shortcuts.items".to_string()]);
        assert!(leaks(&facts(&pins, &none, &soft, &shortcuts))[0].contains("shortcuts"));
        let mut f = facts(&pins, &none, &soft, &deps);
        f.installed = true;
        assert_eq!(leaks(&f), ["installed widgets and plugins were read"]);
    }

    #[test]
    fn the_sidecar_holds_the_pins_the_adapter_and_the_versions_and_no_clock() {
        let (pins, none, soft, deps) = (Pins::default(), BTreeSet::new(), adapter(true), BTreeSet::new());
        let faces = ["Segoe UI|x".to_string()];
        let mut f = facts(&pins, &none, &soft, &deps);
        f.faces = &faces;
        let j = sidecar(&f);
        let mut drawn_nothing = facts(&pins, &none, &soft, &deps);
        drawn_nothing.adapter = None;
        assert!(leaks(&drawn_nothing).is_empty() && sidecar(&drawn_nothing)["adapter"].is_null(), "a dump names no adapter and is still hermetic");
        assert_eq!((j["hermetic"].as_bool(), j["pins"]["now"].as_str(), j["pins"]["sys.cpu"].as_u64(), j["adapter"]["software"].as_bool()), (Some(true), Some("2026-01-15T10:10:30"), Some(37), Some(true)));
        assert_eq!((j["fonts"]["faces"].as_u64(), j["engine"]["crates"]["wgpu"].is_string(), j["engine"]["crates"]["cosmic-text"].is_string()), (Some(1), true, true));
        assert_eq!(sidecar(&f)["fonts"], j["fonts"], "the same fonts hash the same");
    }

    #[test]
    fn a_folder_hash_follows_its_files_and_ignores_dot_folders() {
        let dir = std::env::temp_dir().join(format!("wf-env-hash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".cache")).unwrap();
        std::fs::write(dir.join("a.toml"), "x").unwrap();
        std::fs::write(dir.join(".cache").join("junk"), "1").unwrap();
        let first = hash_files(&dir, &files_under(&dir));
        std::fs::write(dir.join(".cache").join("junk"), "2").unwrap();
        assert_eq!(hash_files(&dir, &files_under(&dir)), first, "a cache does not count");
        std::fs::write(dir.join("a.toml"), "y").unwrap();
        assert_ne!(hash_files(&dir, &files_under(&dir)), first);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
