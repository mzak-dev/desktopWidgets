//! Design tokens (decision 19): palette, font set and glyph set over built-in
//! defaults, style layers on top. An undefined token is loud magenta (decision 14).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::color::{Color, MAGENTA};
use crate::value::Value;
use crate::widgets::ParamDef;

/// The Style tokens, declared in `assets/style.toml`: set globally, overridable per Instance.
pub fn style_schema() -> &'static [ParamDef] {
    static SCHEMA: OnceLock<Vec<ParamDef>> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let t: toml::Table = include_str!("../assets/style.toml").parse().expect("style.toml is valid TOML");
        let params = t.get("params").and_then(|v| v.as_table()).expect("style.toml has [params]");
        crate::format::parse_params(params).expect("built-in style schema is valid")
    })
}

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

/// An Instance's own axes; `None` uses the global one.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ThemePick {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub palette: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fonts: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub glyphs: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_pack: Option<String>,
}

impl ThemePick {
    pub fn resolve(&self, global: &Selection) -> Selection {
        let or = |mine: &Option<String>, g: &String| mine.clone().unwrap_or_else(|| g.clone());
        Selection { palette: or(&self.palette, &global.palette), fonts: or(&self.fonts, &global.fonts), glyphs: or(&self.glyphs, &global.glyphs), icon_pack: or(&self.icon_pack, &global.icon_pack) }
    }

    pub fn is_empty(&self) -> bool {
        *self == ThemePick::default()
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
    /// The built-in default axes' tokens, untouched by any override, so a partial
    /// override of Midnight still inherits the rest of Midnight.
    base: BTreeMap<String, Value>,
}

fn load_dir(dir: &Path, into: &mut Vec<Axis>, errors: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "toml")).collect();
    paths.sort();
    for p in paths {
        match std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|s| Axis::parse(&s, p.parent())) {
            // a user file with the same name replaces the built-in, in its place
            Ok(a) => match into.iter_mut().find(|x| x.name == a.name) {
                Some(slot) => *slot = a,
                None => into.push(a),
            },
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
        let d = Selection::default();
        for axis in [Self::pick(&lib.glyphs, &d.glyphs), Self::pick(&lib.fonts, &d.fonts), Self::pick(&lib.palettes, &d.palette)] {
            lib.base.extend(axis.tokens.clone());
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

    /// An unknown name (a removed file) falls back to the default axis, wherever it sits.
    fn pick_or_default<'a>(axes: &'a [Axis], name: &str, default: &str) -> &'a Axis {
        axes.iter().find(|a| a.name == name).unwrap_or_else(|| Self::pick(axes, default))
    }

    pub fn palette(&self, n: &str) -> &Axis {
        Self::pick_or_default(&self.palettes, n, &Selection::default().palette)
    }
    pub fn fonts(&self, n: &str) -> &Axis {
        Self::pick_or_default(&self.fonts, n, &Selection::default().fonts)
    }
    pub fn glyphs(&self, n: &str) -> &Axis {
        Self::pick_or_default(&self.glyphs, n, &Selection::default().glyphs)
    }
}

/// Size/spacing tokens every theme inherits, then the Style defaults (`accent` has
/// none, so it stays the palette's).
fn base_tokens() -> BTreeMap<String, Value> {
    let sizes = [
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
    ];
    let defaults = style_schema().iter().filter(|p| p.default != Value::Str(String::new())).map(|p| (p.name.clone(), p.default.clone()));
    sizes.into_iter().map(|(k, v)| (k.to_string(), Value::Num(v))).chain(defaults).collect()
}

#[derive(Clone, Debug, Default)]
pub struct Theme {
    tokens: BTreeMap<String, Value>,
}

impl Theme {
    /// style layers (later wins) > palette > fonts > glyphs > base (decision 19's fallback chain).
    pub fn compose(lib: &Library, sel: &Selection, layers: &[&BTreeMap<String, Value>]) -> Theme {
        let mut tokens = base_tokens();
        // Base colours/fonts come from the built-in default axes, as shipped.
        tokens.extend(lib.base.clone());
        for axis in [lib.glyphs(&sel.glyphs), lib.fonts(&sel.fonts), lib.palette(&sel.palette)] {
            tokens.extend(axis.tokens.clone());
        }
        for layer in layers {
            tokens.extend(layer.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        Theme { tokens }
    }

    /// A bool token; the string "false" (an old override) is false, unlike `Value::truthy`.
    pub fn flag(&self, name: &str) -> bool {
        match self.tokens.get(name) {
            Some(Value::Bool(b)) => *b,
            Some(Value::Num(n)) => *n != 0.0,
            Some(Value::Str(s)) => s == "true",
            _ => false,
        }
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
        let t = Theme::compose(&lib, &Selection::default(), &[]);
        assert_eq!(t.num("radius-md"), 14.0);
        assert_eq!(t.str("glyph-gear"), "\u{E713}");

        // override beats palette beats base
        let mut ov = BTreeMap::new();
        ov.insert("accent".to_string(), Value::Str("#123456".into()));
        let t = Theme::compose(&lib, &Selection { palette: "Daylight".into(), ..Default::default() }, &[&ov]);
        assert_eq!(t.color("accent").to_hex(), "#123456");
        assert_eq!(t.color("text").to_hex(), "#141a2a");

        // a token nobody defines is magenta, never a default
        assert_eq!(t.color("no-such-token"), MAGENTA);
        // an unknown axis name falls back to the built-in rather than failing
        let t = Theme::compose(&lib, &Selection { palette: "Nope".into(), ..Default::default() }, &[]);
        assert_eq!(t.color("surface"), lib.palettes[0].tokens.get("surface").map(|v| Color::parse(&v.to_string()).unwrap()).unwrap());
    }

    #[test]
    fn style_layers_cascade_over_palette_and_base() {
        let lib = Library::load(Path::new("nope"));
        let v = |s: &str| Value::Str(s.into());
        assert!(Theme::compose(&lib, &Selection::default(), &[]).flag("outlines"), "schema default");
        assert_eq!(Theme::compose(&lib, &Selection::default(), &[]).num("bg-opacity"), 60.0);
        let global = BTreeMap::from([("accent".to_string(), v("#111111")), ("blur".to_string(), Value::Bool(true))]);
        let mine = BTreeMap::from([("accent".to_string(), v("#222222"))]);
        let t = Theme::compose(&lib, &Selection::default(), &[&global, &mine]);
        assert_eq!(t.color("accent").to_hex(), "#222222", "instance beats global");
        assert!(t.flag("blur"), "global beats base");
        assert!(!Theme::compose(&lib, &Selection::default(), &[&BTreeMap::from([("blur".to_string(), v("false"))])]).flag("blur"));
    }

    #[test]
    fn a_palette_can_set_style_defaults_and_a_pick_overrides_the_global_axes() {
        let mut lib = Library::load(Path::new("nope"));
        lib.palettes.push(Axis::parse("name = 'Glass'\n[tokens]\ntransparent = true\nbg-opacity = 30", None).unwrap());
        let pick = ThemePick { palette: Some("Glass".into()), ..Default::default() };
        let t = Theme::compose(&lib, &pick.resolve(&Selection::default()), &[]);
        assert!(t.flag("transparent"));
        assert_eq!(t.num("bg-opacity"), 30.0);
        assert!(ThemePick::default().is_empty() && !pick.is_empty());
        assert_eq!(ThemePick::default().resolve(&Selection::default()), Selection::default());
    }

    #[test]
    fn the_style_schema_keeps_its_file_order() {
        let names: Vec<&str> = style_schema().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["accent", "radius-lg", "outlines", "blur", "transparent", "bg-opacity", "tint", "shadow", "text-scale", "anim-speed"]);
    }

    #[test]
    fn a_user_axis_replaces_the_builtin_in_place() {
        let dir = std::env::temp_dir().join(format!("wf-axis-inplace-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("palettes")).unwrap();
        std::fs::write(dir.join("palettes").join("mine.toml"), "name = 'Midnight'\n[tokens]\naccent = '#123456'").unwrap();
        let lib = Library::load(&dir);
        assert_eq!(lib.palettes[0].name, "Midnight", "the override keeps the built-in's place");
        assert_eq!(lib.palettes.iter().filter(|a| a.name == "Midnight").count(), 1);
        assert_eq!(lib.palettes[1].name, "Daylight");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_partial_override_keeps_builtin_tokens() {
        let builtin = Theme::compose(&Library::load(Path::new("nope")), &Selection::default(), &[]);
        let dir = std::env::temp_dir().join(format!("wf-axis-partial-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("palettes")).unwrap();
        std::fs::write(dir.join("palettes").join("mine.toml"), "name = 'Midnight'\n[tokens]\naccent = '#123456'").unwrap();
        let t = Theme::compose(&Library::load(&dir), &Selection::default(), &[]);
        assert_eq!(t.color("accent").to_hex(), "#123456");
        assert_eq!(t.color("surface"), builtin.color("surface"), "the rest of Midnight still applies");
        assert_ne!(t.color("surface"), MAGENTA);
        // an unknown name falls back to Midnight even when it is no longer first
        let mut lib = Library::load(&dir);
        lib.palettes.rotate_left(1);
        assert_eq!(lib.palette("Removed").name, "Midnight");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn every_file_in_assets_axes_is_built_in() {
        let lib = Library::load(Path::new("nope"));
        for (sub, axes) in [("palettes", &lib.palettes), ("fonts", &lib.fonts), ("glyphs", &lib.glyphs)] {
            let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join(sub);
            let files = std::fs::read_dir(dir).unwrap().filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|x| x == "toml")).count();
            assert_eq!(axes.len(), files, "assets/{sub} has a file missing from the built-in list");
        }
    }

    #[test]
    fn axes_swap_independently() {
        let lib = Library::load(Path::new("definitely-not-a-dir"));
        let a = Theme::compose(&lib, &Selection { glyphs: "Symbols".into(), ..Default::default() }, &[]);
        assert_eq!(a.str("font-glyph"), "Segoe UI Symbol");
        assert_eq!(a.color("accent").to_hex(), "#6ea8ff"); // palette untouched
    }
}
