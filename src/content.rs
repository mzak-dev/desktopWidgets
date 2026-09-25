//! Content: Widgets, palettes, font sets, glyph sets and Icon Packs. The built-ins
//! come first, then each content root in order (enabled Plugins, then the user's
//! data folder); every root has the data folder's layout and a later one wins.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::theme::{AxisKind, Library, icon_pack_dirs};
use crate::widgets::{Registry, widget_files};

/// Where a piece of content came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    BuiltIn,
    Plugin(String),
    User,
}

/// A folder laid out like the data folder: `widgets/ palettes/ fonts/ glyphs/ iconpacks/`.
#[derive(Clone, Debug, PartialEq)]
pub struct Root {
    pub origin: Origin,
    pub dir: PathBuf,
}

impl Root {
    pub fn user(data: &Path) -> Root {
        Root { origin: Origin::User, dir: data.to_path_buf() }
    }

    pub fn plugin(id: &str, dir: &Path) -> Root {
        Root { origin: Origin::Plugin(id.to_string()), dir: dir.to_path_buf() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Item {
    Widget,
    Palette,
    FontSet,
    GlyphSet,
    IconPack,
}

impl Item {
    fn of(kind: AxisKind) -> Item {
        match kind {
            AxisKind::Palette => Item::Palette,
            AxisKind::Fonts => Item::FontSet,
            AxisKind::Glyphs => Item::GlyphSet,
        }
    }
}

/// `name` from `loser` is replaced by the same name from `winner`, a later root.
#[derive(Clone, Debug, PartialEq)]
pub struct Shadow {
    pub item: Item,
    pub name: String,
    pub winner: Origin,
    pub loser: Origin,
}

/// What one root provides, by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Contents {
    pub widgets: Vec<String>,
    pub palettes: Vec<String>,
    pub fonts: Vec<String>,
    pub glyphs: Vec<String>,
    pub icon_packs: Vec<String>,
}

impl Contents {
    pub fn is_empty(&self) -> bool {
        *self == Contents::default()
    }

    /// "3 widgets · 2 palettes · 1 font set"
    pub fn summary(&self) -> String {
        let parts = [(self.widgets.len(), "widget"), (self.palettes.len(), "palette"), (self.fonts.len(), "font set"), (self.glyphs.len(), "glyph set"), (self.icon_packs.len(), "icon pack")];
        let v: Vec<String> = parts.iter().filter(|(n, _)| *n > 0).map(|(n, what)| format!("{n} {what}{}", if *n == 1 { "" } else { "s" })).collect();
        if v.is_empty() { "nothing yet".into() } else { v.join(" · ") }
    }

    fn get_mut(&mut self, item: Item) -> &mut Vec<String> {
        match item {
            Item::Widget => &mut self.widgets,
            Item::Palette => &mut self.palettes,
            Item::FontSet => &mut self.fonts,
            Item::GlyphSet => &mut self.glyphs,
            Item::IconPack => &mut self.icon_packs,
        }
    }
}

/// Everything the engine can show, and where each piece came from.
pub struct Catalog {
    pub registry: Registry,
    pub library: Library,
    /// Every font file to register: loose files under each root's `fonts/`, plus the
    /// `files` of every font set and glyph set. Sorted, no duplicates.
    pub font_files: Vec<PathBuf>,
    /// Icon Pack name to its folder; "Default" has none.
    pub icon_packs: BTreeMap<String, PathBuf>,
    pub origins: BTreeMap<(Item, String), Origin>,
    pub shadows: Vec<Shadow>,
}

const FONT_EXTENSIONS: &[&str] = &["ttf", "otf", "ttc", "otc"];

fn font_files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for p in rd.filter_map(|e| e.ok()).map(|e| e.path()) {
        if p.is_dir() {
            font_files_under(&p, out);
        } else if p.extension().and_then(|x| x.to_str()).is_some_and(|x| FONT_EXTENSIONS.contains(&x.to_ascii_lowercase().as_str())) {
            out.push(p);
        }
    }
}

impl Catalog {
    pub fn load(roots: &[Root]) -> Catalog {
        let registry = Registry::builtin();
        let library = Library::builtin();
        let mut origins = BTreeMap::new();
        for id in registry.ids() {
            origins.insert((Item::Widget, id), Origin::BuiltIn);
        }
        for kind in AxisKind::ALL {
            for a in library.axes(kind) {
                origins.insert((Item::of(kind), a.name.clone()), Origin::BuiltIn);
            }
        }
        let mut cat = Catalog { registry, library, font_files: Vec::new(), icon_packs: BTreeMap::new(), origins, shadows: Vec::new() };
        for root in roots {
            let ids = cat.registry.load_dir(&root.dir.join("widgets"));
            cat.claim(Item::Widget, ids, &root.origin);
            for kind in AxisKind::ALL {
                let names = cat.library.load_axes(kind, &root.dir.join(kind.folder()));
                cat.claim(Item::of(kind), names, &root.origin);
            }
            let packs = icon_pack_dirs(&root.dir);
            cat.claim(Item::IconPack, packs.iter().map(|(n, _)| n.clone()).collect(), &root.origin);
            cat.icon_packs.extend(packs);
            font_files_under(&root.dir.join("fonts"), &mut cat.font_files);
        }
        cat.library.icon_packs.extend(cat.icon_packs.keys().cloned());
        for kind in [AxisKind::Fonts, AxisKind::Glyphs] {
            cat.font_files.extend(cat.library.axes(kind).iter().flat_map(|a| a.files.iter().cloned()));
        }
        cat.font_files.sort();
        cat.font_files.dedup();
        cat
    }

    /// Records `origin` as the provider of `names`, and whom it replaced.
    fn claim(&mut self, item: Item, names: Vec<String>, origin: &Origin) {
        for name in names {
            if let Some(loser) = self.origins.insert((item, name.clone()), origin.clone()) {
                if loser != *origin {
                    self.shadows.push(Shadow { item, name, winner: origin.clone(), loser });
                }
            }
        }
    }

    pub fn origin(&self, item: Item, name: &str) -> Option<&Origin> {
        self.origins.get(&(item, name.to_string()))
    }
}

/// Names only, without loading anything: what a root would provide (a disabled Plugin's
/// Widgets, for instance).
pub fn scan(dir: &Path) -> Contents {
    let mut c = Contents::default();
    c.widgets = widget_files(&dir.join("widgets")).into_iter().map(|(id, _)| id).collect();
    for kind in AxisKind::ALL {
        let mut lib = Library::default();
        *c.get_mut(Item::of(kind)) = lib.load_axes(kind, &dir.join(kind.folder()));
    }
    c.icon_packs = icon_pack_dirs(dir).into_iter().map(|(n, _)| n).collect();
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wf-content-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn put(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    const WIDGET: &str = "name = 'Mine'\n[root]\ntype = 'box'";

    #[test]
    fn later_roots_replace_widgets_and_record_what_they_shadow() {
        let (a, user) = (tmp("shadow-a"), tmp("shadow-user"));
        put(&a, "widgets/clock.toml", "name = 'Sunset Clock'\n[root]\ntype = 'box'");
        put(&a, "widgets/weather.toml", WIDGET);
        put(&user, "widgets/weather.toml", "name = 'My Weather'\n[root]\ntype = 'box'");
        let cat = Catalog::load(&[Root::plugin("a", &a), Root::user(&user)]);
        let name = |id: &str| cat.registry.get(id).unwrap().as_ref().unwrap().meta().name.clone();
        assert_eq!((name("clock"), name("weather")), ("Sunset Clock".to_string(), "My Weather".to_string()));
        assert_eq!(cat.origin(Item::Widget, "clock"), Some(&Origin::Plugin("a".into())));
        assert_eq!(cat.origin(Item::Widget, "weather"), Some(&Origin::User));
        assert_eq!(cat.origin(Item::Widget, "digital_clock"), Some(&Origin::BuiltIn));
        assert!(cat.shadows.contains(&Shadow { item: Item::Widget, name: "clock".into(), winner: Origin::Plugin("a".into()), loser: Origin::BuiltIn }));
        assert!(cat.shadows.contains(&Shadow { item: Item::Widget, name: "weather".into(), winner: Origin::User, loser: Origin::Plugin("a".into()) }));
        for d in [a, user] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    #[test]
    fn a_broken_plugin_widget_is_an_error_not_a_fallback() {
        let a = tmp("broken");
        put(&a, "widgets/clock.toml", "[root]\ntype='text'\ncolour='#fff'");
        let cat = Catalog::load(&[Root::plugin("a", &a)]);
        let e = cat.registry.get("clock").unwrap().as_ref().err().expect("an error, not the built-in");
        assert!(e.contains("colour") && e.contains("clock.toml"), "{e}");
        assert_eq!(cat.origin(Item::Widget, "clock"), Some(&Origin::Plugin("a".into())));
        std::fs::remove_dir_all(a).ok();
    }

    #[test]
    fn two_plugins_with_one_widget_is_a_collision_and_the_later_wins() {
        let (a, b) = (tmp("coll-a"), tmp("coll-b"));
        put(&a, "widgets/weather.toml", "name = 'A'\n[root]\ntype = 'box'");
        put(&b, "widgets/weather.toml", "name = 'B'\n[root]\ntype = 'box'");
        put(&a, "palettes/p.toml", "name = 'Sunset'\n[tokens]\naccent = '#111111'");
        put(&b, "palettes/p.toml", "name = 'Sunset'\n[tokens]\naccent = '#222222'");
        let cat = Catalog::load(&[Root::plugin("a", &a), Root::plugin("b", &b)]);
        assert_eq!(cat.registry.get("weather").unwrap().as_ref().unwrap().meta().name, "B");
        assert_eq!(cat.library.palette("Sunset").tokens.get("accent").map(|v| v.to_string()), Some("#222222".into()));
        let lost: Vec<_> = cat.shadows.iter().filter(|s| s.loser == Origin::Plugin("a".into())).map(|s| (s.item, s.name.as_str())).collect();
        assert_eq!(lost, [(Item::Widget, "weather"), (Item::Palette, "Sunset")]);
        assert_eq!(cat.library.palettes.iter().filter(|p| p.name == "Sunset").count(), 1);
        for d in [a, b] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    #[test]
    fn icon_packs_map_to_the_winning_root() {
        let (a, user) = (tmp("packs-a"), tmp("packs-user"));
        std::fs::create_dir_all(a.join("iconpacks").join("Neon")).unwrap();
        std::fs::create_dir_all(a.join("iconpacks").join("Default")).unwrap();
        std::fs::create_dir_all(user.join("iconpacks").join("Neon")).unwrap();
        std::fs::create_dir_all(user.join("iconpacks").join("Paper")).unwrap();
        let cat = Catalog::load(&[Root::plugin("a", &a), Root::user(&user)]);
        assert_eq!(cat.icon_packs.get("Neon"), Some(&user.join("iconpacks").join("Neon")));
        assert_eq!(cat.library.icon_packs, ["Default", "Neon", "Paper"], "listed once, Default first and never a folder");
        for d in [a, user] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    #[test]
    fn font_files_are_listed_once_across_roots() {
        let (a, user) = (tmp("fonts-a"), tmp("fonts-user"));
        put(&a, "fonts/Sunset.ttf", "not really a font");
        put(&a, "fonts/set.toml", "name = 'Sunset'\nfiles = ['Sunset.ttf']\n[tokens]\nfont-body = 'Sunset'");
        put(&a, "glyphs/g.toml", "name = 'Neon'\nfiles = ['../icons/neon.otf']\n[tokens]\nfont-glyph = 'Neon'");
        put(&user, "fonts/deep/Mine.OTF", "");
        put(&user, "fonts/readme.txt", "");
        let cat = Catalog::load(&[Root::plugin("a", &a), Root::user(&user)]);
        let mut want = vec![a.join("fonts").join("Sunset.ttf"), a.join("glyphs").join("../icons/neon.otf"), user.join("fonts").join("deep").join("Mine.OTF")];
        want.sort();
        assert_eq!(cat.font_files, want);
        for d in [a, user] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    #[test]
    fn a_broken_axis_is_skipped_and_reported_with_its_file() {
        let a = tmp("broken-axis");
        put(&a, "palettes/bad.toml", "[tokens]\naccent = '#fff'");
        let cat = Catalog::load(&[Root::plugin("a", &a)]);
        assert!(cat.library.errors.iter().any(|e| e.contains("bad.toml") && e.contains("name")), "{:?}", cat.library.errors);
        assert_eq!(cat.library.palettes.len(), Library::builtin().palettes.len());
        std::fs::remove_dir_all(a).ok();
    }

    #[test]
    fn scan_names_what_a_root_provides() {
        let a = tmp("scan");
        put(&a, "widgets/weather.toml", WIDGET);
        put(&a, "palettes/p.toml", "name = 'Sunset'\n[tokens]\naccent = '#111111'");
        std::fs::create_dir_all(a.join("iconpacks").join("Neon")).unwrap();
        let c = scan(&a);
        assert_eq!((c.widgets.clone(), c.palettes.clone(), c.icon_packs.clone()), (vec!["weather".to_string()], vec!["Sunset".to_string()], vec!["Neon".to_string()]));
        assert_eq!(c.summary(), "1 widget · 1 palette · 1 icon pack");
        assert_eq!(Contents::default().summary(), "nothing yet");
        std::fs::remove_dir_all(a).ok();
    }
}
