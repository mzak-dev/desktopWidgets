//! Embeds every `assets/widgets/*.toml` as a built-in widget, so adding a
//! built-in TOML Widget is dropping a file there.

use std::path::Path;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("widgets");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("assets/widgets")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();
    let mut out = String::from("&[\n");
    for p in files {
        let id = p.file_stem().and_then(|s| s.to_str()).expect("utf-8 widget file name");
        out += &format!("    ({id:?}, include_str!({:?})),\n", p.display().to_string());
    }
    out += "]\n";
    let dest = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("builtin_widgets.rs");
    std::fs::write(dest, out).expect("write builtin_widgets.rs");
}
