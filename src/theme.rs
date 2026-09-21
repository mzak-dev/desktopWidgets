//! Design tokens (decision 19). A Theme is three independently swappable axes,
//! palette / font set / glyph set, layered over built-in base defaults, with
//! per-Instance overrides on top. A token nobody defines resolves to loud
//! magenta (decision 14), never a plausible default.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::color::{Color, MAGENTA};
use crate::value::Value;

#[derive(Clone, Debug, Default)]
pub struct Axis {
    pub name: String,
    pub tokens: BTreeMap<String, Value>,
    /// Font files to register (font sets only).
    pub files: Vec<PathBuf>,
}

impl Axis {
    pub fn parse(src: &str, base: Option<&Path>) -> Result<Axis, String> {
        let t: toml::Table = src.parse().map_err(|e| format!("{e}"))?;
        let name = t.get("name").and_then(|v| v.as_str()).ok_or("missing `name`")?.to_string();
        let tokens = t
            .get("tokens")
            .and_then(|v| v.as_table())
            .map(|tb| tb.iter().map(|(k, v)| (k.clone(), Value::from(v))).collect())
            .unwrap_or_default();
        let files = t
            .get("files")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|f| f.as_str()).map(|f| base.map_or(PathBuf::from(f), |b| b.join(f))).collect())
            .unwrap_or_default();
        Ok(Axis { name, tokens, files })
    }
}

/// Which axis of each kind is active, plus the icon pack. Serialised into the workspace.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Selection {
    pub palette: String,
    pub fonts: String,
    pub glyphs: String,
    pub icon_pack: String,
}

impl Default for Selection {
    fn default() -> Self {
        Self { palette: "Midnight".into(), fonts: "System".into(), glyphs: "Fluent".into(), icon_pack: "Default".into() }
    }
}

const BUILTIN_PALETTES: &[&str] = &[
    include_str!("../assets/palettes/midnight.toml"),
    include_str!("../assets/palettes/daylight.toml"),
    include_str!("../assets/palettes/aurora.toml"),
    include_str!("../assets/palettes/graphite.toml"),
];
const BUILTIN_FONTS: &[&str] = &[
    include_str!("../assets/fonts/system.toml"),
    include_str!("../assets/fonts/editorial.toml"),
    include_str!("../assets/fonts/technical.toml"),
];
const BUILTIN_GLYPHS: &[&str] = &[
    include_str!("../assets/glyphs/fluent.toml"),
    include_str!("../assets/glyphs/mdl2.toml"),
    include_str!("../assets/glyphs/symbols.toml"),
];

#[derive(Clone, Debug, Default)]
pub struct Library {
    pub palettes: Vec<Axis>,
    pub fonts: Vec<Axis>,
    pub glyphs: Vec<Axis>,
    pub icon_packs: Vec<String>,
    /// Problems found while loading user files, for the settings log.
    pub errors: Vec<String>,
}

fn load_dir(dir: &Path, into: &mut Vec<Axis>, errors: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "toml")).collect();
    paths.sort();
    for p in paths {
        match std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|s| Axis::parse(&s, p.parent())) {
            // a user file with the same name replaces the built-in
            Ok(a) => {
                into.retain(|x| x.name != a.name);
                into.push(a);
            }
            Err(e) => errors.push(format!("{}: {e}", p.display())),
        }
    }
}

impl Library {
    /// Built-ins plus anything in `<data>/{palettes,fonts,glyphs,iconpacks}`.
    pub fn load(data: &Path) -> Library {
        let mut lib = Library::default();
        for (src, into) in [
            (BUILTIN_PALETTES, &mut lib.palettes),
            (BUILTIN_FONTS, &mut lib.fonts),
            (BUILTIN_GLYPHS, &mut lib.glyphs),
        ] {
            for s in src {
                into.push(Axis::parse(s, None).expect("built-in axis is valid"));
            }
        }
        load_dir(&data.join("palettes"), &mut lib.palettes, &mut lib.errors);
        load_dir(&data.join("fonts"), &mut lib.fonts, &mut lib.errors);
        load_dir(&data.join("glyphs"), &mut lib.glyphs, &mut lib.errors);
        lib.icon_packs = vec!["Default".into()];
        if let Ok(rd) = std::fs::read_dir(data.join("iconpacks")) {
            let mut names: Vec<String> = rd.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).collect();
            names.sort();
            lib.icon_packs.extend(names);
        }
        lib
    }

    fn pick<'a>(axes: &'a [Axis], name: &str) -> &'a Axis {
        axes.iter().find(|a| a.name == name).unwrap_or(&axes[0])
    }

    pub fn palette(&self, n: &str) -> &Axis {
        Self::pick(&self.palettes, n)
    }
    pub fn fonts(&self, n: &str) -> &Axis {
        Self::pick(&self.fonts, n)
    }
    pub fn glyphs(&self, n: &str) -> &Axis {
        Self::pick(&self.glyphs, n)
    }
}

/// Size/spacing tokens every theme inherits.
fn base_sizes() -> BTreeMap<String, Value> {
    [
        ("radius-sm", 8.0),
        ("radius-md", 14.0),
        ("radius-lg", 22.0),
        ("space-1", 4.0),
        ("space-2", 8.0),
        ("space-3", 12.0),
        ("space-4", 16.0),
        ("font-size-sm", 12.0),
        ("font-size-md", 14.0),
        ("font-size-lg", 18.0),
        ("font-size-xl", 28.0),
        ("font-size-2xl", 48.0),
        ("stroke", 1.0),
        ("gutter", 20.0),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), Value::Num(v)))
    .collect()
}

#[derive(Clone, Debug, Default)]
pub struct Theme {
    tokens: BTreeMap<String, Value>,
}

impl Theme {
    /// overrides > palette > fonts > glyphs > sizes/base (decision 19's fallback chain).
    pub fn compose(lib: &Library, sel: &Selection, overrides: &BTreeMap<String, Value>) -> Theme {
        let mut tokens = base_sizes();
        // Base colours/fonts come from the first (built-in) entry of each axis.
        for axis in [&lib.glyphs[0], &lib.fonts[0], &lib.palettes[0]] {
            tokens.extend(axis.tokens.clone());
        }
        for axis in [lib.glyphs(&sel.glyphs), lib.fonts(&sel.fonts), lib.palette(&sel.palette)] {
            tokens.extend(axis.tokens.clone());
        }
        tokens.extend(overrides.clone());
        Theme { tokens }
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.tokens.get(name)
    }

    pub fn color(&self, name: &str) -> Color {
        match self.tokens.get(name) {
            Some(Value::Str(s)) => Color::parse(s).unwrap_or(MAGENTA),
            _ => MAGENTA,
        }
    }

    pub fn num(&self, name: &str) -> f32 {
        self.tokens.get(name).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32
    }

    pub fn str(&self, name: &str) -> String {
        match self.tokens.get(name) {
            Some(v) => v.to_string(),
            None => String::new(),
        }
    }

    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.tokens.keys()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_load_and_fallback_chain_holds() {
        let lib = Library::load(Path::new("definitely-not-a-dir"));
        assert!(lib.palettes.len() >= 4 && lib.fonts.len() >= 3 && lib.glyphs.len() >= 3);
        let t = Theme::compose(&lib, &Selection::default(), &BTreeMap::new());
        assert_eq!(t.num("radius-md"), 14.0);
        assert_eq!(t.str("glyph-gear"), "\u{E713}");

        // override beats palette beats base
        let mut ov = BTreeMap::new();
        ov.insert("accent".to_string(), Value::Str("#123456".into()));
        let t = Theme::compose(&lib, &Selection { palette: "Daylight".into(), ..Default::default() }, &ov);
        assert_eq!(t.color("accent").to_hex(), "#123456");
        assert_eq!(t.color("text").to_hex(), "#141a2a");

        // a token nobody defines is magenta, never a default
        assert_eq!(t.color("no-such-token"), MAGENTA);
        // an unknown axis name falls back to the built-in rather than failing
        let t = Theme::compose(&lib, &Selection { palette: "Nope".into(), ..Default::default() }, &BTreeMap::new());
        assert_eq!(t.color("surface"), lib.palettes[0].tokens.get("surface").map(|v| Color::parse(&v.to_string()).unwrap()).unwrap());
    }

    #[test]
    fn axes_swap_independently() {
        let lib = Library::load(Path::new("definitely-not-a-dir"));
        let a = Theme::compose(&lib, &Selection { glyphs: "Symbols".into(), ..Default::default() }, &BTreeMap::new());
        assert_eq!(a.str("font-glyph"), "Segoe UI Symbol");
        assert_eq!(a.color("accent").to_hex(), "#6ea8ff"); // palette untouched
    }
}
