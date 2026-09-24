//! Plugins (ADR-0007): packages of content, each a folder under `<data>/plugins/<id>/`
//! laid out like the data folder, with a `plugin.toml` manifest. An enabled Plugin is a
//! content root between the built-ins and the user's own files.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use crate::content::{self, Catalog, Contents, Item, Origin, Root};
use crate::widgets::Registry;
use crate::workspace::Workspace;

pub const MANIFEST: &str = "plugin.toml";
const KEYS: &[&str] = &["id", "name", "version", "author", "description", "homepage", "wayfinder", "code"];

#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub homepage: Option<String>,
    /// The oldest Wayfinder that can load it.
    pub wayfinder: Option<String>,
}

fn version_parts(v: &str) -> Vec<u64> {
    v.split('.').map(|p| p.chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0)).collect()
}

/// `a` is a later version than `b` (`1.10` > `1.9`, `1.2` == `1.2.0`).
fn newer(a: &str, b: &str) -> bool {
    let (mut x, mut y) = (version_parts(a), version_parts(b));
    let n = x.len().max(y.len());
    x.resize(n, 0);
    y.resize(n, 0);
    x > y
}

impl Manifest {
    pub fn parse(src: &str) -> Result<Manifest, String> {
        Self::parse_for(src, env!("CARGO_PKG_VERSION"))
    }

    fn parse_for(src: &str, engine: &str) -> Result<Manifest, String> {
        let t: toml::Table = src.parse().map_err(|e| format!("{e}"))?;
        for k in t.keys() {
            if !KEYS.contains(&k.as_str()) {
                return Err(format!("unknown key `{k}`{}", crate::format::suggest(k, &[KEYS])));
            }
        }
        if t.contains_key("code") {
            return Err("it runs code, which needs a newer Wayfinder".into());
        }
        let text = |k: &str| -> Result<Option<String>, String> {
            match t.get(k) {
                None => Ok(None),
                Some(v) => v.as_str().map(|s| Some(s.trim().to_string())).ok_or_else(|| format!("`{k}` must be text")),
            }
        };
        let need = |k: &str| -> Result<String, String> { text(k)?.filter(|s| !s.is_empty()).ok_or_else(|| format!("missing `{k}`")) };
        let id = need("id")?;
        valid_id(&id)?;
        let wayfinder = text("wayfinder")?;
        if let Some(w) = &wayfinder {
            if newer(w, engine) {
                return Err(format!("needs Wayfinder {w} or newer (this is {engine})"));
            }
        }
        Ok(Manifest { id, name: need("name")?, version: need("version")?, author: text("author")?.unwrap_or_default(), description: text("description")?.unwrap_or_default(), homepage: text("homepage")?, wayfinder })
    }
}

/// Lower-case letters, digits, `-` and `_`, so an id is a safe folder name and never
/// clashes with the separators in Settings actions.
pub fn valid_id(id: &str) -> Result<(), String> {
    let ok_chars = id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    let first_ok = id.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    if id.is_empty() || id.len() > 64 || !ok_chars || !first_ok {
        return Err(format!("id `{id}`: use 1 to 64 lower-case letters, digits, `-` or `_`, starting with a letter or digit"));
    }
    let reserved = matches!(id, "con" | "prn" | "aux" | "nul") || ((id.starts_with("com") || id.starts_with("lpt")) && id.len() == 4 && id.as_bytes()[3].is_ascii_digit());
    if reserved {
        return Err(format!("id `{id}` is a reserved Windows name"));
    }
    Ok(())
}

/// One folder under `plugins/`. A Plugin whose manifest is broken is listed but never loaded.
#[derive(Clone, Debug)]
pub struct Plugin {
    /// The folder name.
    pub id: String,
    pub dir: PathBuf,
    pub manifest: Result<Manifest, String>,
    pub contents: Contents,
}

impl Plugin {
    pub fn name(&self) -> &str {
        self.manifest.as_ref().map_or(&self.id, |m| &m.name)
    }
}

pub struct PluginStore {
    dir: PathBuf,
}

impl PluginStore {
    pub fn new(data: &Path) -> PluginStore {
        PluginStore { dir: data.join("plugins") }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The data folder's sibling `<data>.<what>-<pid>-<n>`: on the same volume, so a
    /// rename is atomic, but outside the watched folder.
    fn scratch(&self, what: &str) -> PathBuf {
        let data = self.dir.parent().unwrap_or(&self.dir);
        let name = data.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "wayfinder".into());
        (0..).map(|n| data.with_file_name(format!("{name}.{what}-{}-{n}", std::process::id()))).find(|p| !p.exists()).expect("a free name")
    }

    /// Moves the Plugin out first, so a locked file fails the whole removal before
    /// anything is deleted; then deletes it (leftovers go at the next `sweep`).
    pub fn remove(&self, id: &str) -> Result<(), String> {
        valid_id(id)?;
        let dir = self.dir.join(id);
        if !dir.is_dir() {
            return Err(format!("{id} is not installed"));
        }
        let trash = self.scratch("trash");
        rename_retrying(&dir, &trash).map_err(|e| format!("could not remove {id}: {e}"))?;
        let _ = std::fs::remove_dir_all(&trash);
        Ok(())
    }

    /// Deletes what an interrupted install or removal left next to the data folder.
    pub fn sweep(&self) {
        let data = self.dir.parent().unwrap_or(&self.dir);
        let (Some(name), Some(parent)) = (data.file_name().map(|n| n.to_string_lossy().into_owned()), data.parent()) else { return };
        let Ok(rd) = std::fs::read_dir(parent) else { return };
        for e in rd.filter_map(|e| e.ok()) {
            let n = e.file_name().to_string_lossy().into_owned();
            if [".install-", ".trash-"].iter().any(|w| n.starts_with(&format!("{name}{w}"))) {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }

    /// Every Plugin folder, sorted by id. Dot-folders are the store's own.
    pub fn list(&self) -> Vec<Plugin> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return vec![] };
        let mut dirs: Vec<(String, PathBuf)> = rd.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).filter_map(|e| Some((e.file_name().into_string().ok()?, e.path()))).filter(|(n, _)| !n.starts_with('.')).collect();
        dirs.sort();
        dirs.into_iter().map(|(id, dir)| Plugin { manifest: read_manifest(&id, &dir), contents: content::scan(&dir), id, dir }).collect()
    }
}

/// Antivirus and indexers hold new files for a moment; a rename retries for half a second.
fn rename_retrying(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::rename(from, to) {
            Err(_) if tries < 10 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            other => return other,
        }
    }
}

fn read_manifest(folder: &str, dir: &Path) -> Result<Manifest, String> {
    let src = std::fs::read_to_string(dir.join(MANIFEST)).map_err(|_| format!("no {MANIFEST}"))?;
    let m = Manifest::parse(&src).map_err(|e| format!("{MANIFEST}: {e}"))?;
    if m.id != folder {
        return Err(format!("{MANIFEST} says id `{}`, but its folder is `{folder}`", m.id));
    }
    Ok(m)
}

/// The content roots of the Plugins that load: a valid manifest and not switched off.
pub fn roots(list: &[Plugin], disabled: &BTreeSet<String>) -> Vec<Root> {
    list.iter().filter(|p| p.manifest.is_ok() && !disabled.contains(&p.id)).map(|p| Root::plugin(&p.id, &p.dir)).collect()
}

/// Instances to hide, with the name of the Plugin to switch on: their Widget is missing
/// and a switched-off Plugin provides it.
pub fn hidden_instances(ws: &Workspace, reg: &Registry, list: &[Plugin]) -> BTreeMap<String, String> {
    let off: Vec<&Plugin> = list.iter().filter(|p| ws.disabled_plugins.contains(&p.id)).collect();
    ws.instances
        .iter()
        .filter(|c| reg.get(&c.widget).is_none())
        .filter_map(|c| off.iter().find(|p| p.contents.widgets.contains(&c.widget)).map(|p| (c.id.clone(), p.name().to_string())))
        .collect()
}

/// What the Plugins page shows for one Plugin.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PluginRow {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    /// "3 widgets · 2 palettes"
    pub summary: String,
    pub enabled: bool,
    pub notes: Vec<String>,
    pub problems: Vec<String>,
    /// Widget ids nothing else provides: removing the Plugin removes their Instances.
    pub sole_widgets: Vec<String>,
}

impl PluginRow {
    /// The Instances that go with it when it is removed.
    pub fn orphans(&self, ws: &Workspace) -> Vec<String> {
        ws.instances.iter().filter(|c| self.sole_widgets.contains(&c.widget)).map(|c| c.id.clone()).collect()
    }
}

fn item_word(item: Item) -> &'static str {
    match item {
        Item::Widget => "widget",
        Item::Palette => "palette",
        Item::FontSet => "font set",
        Item::GlyphSet => "glyph set",
        Item::IconPack => "icon pack",
    }
}

/// The Plugins page's rows, from what the Catalog made of the enabled ones.
pub fn rows(list: &[Plugin], disabled: &BTreeSet<String>, cat: &Catalog) -> Vec<PluginRow> {
    let errors: Vec<String> = cat.registry.errors().into_iter().chain(cat.library.errors.iter().cloned()).collect();
    let builtin = Registry::builtin(); // the names of what a Plugin restyles
    let widget_name = |reg: &Registry, id: &str| reg.get(id).and_then(|d| d.as_ref().ok()).map_or(id.to_string(), |w| w.meta().name.clone());
    let label = |item: Item, name: &str| if item == Item::Widget { widget_name(&cat.registry, name) } else { name.to_string() };
    list.iter()
        .map(|p| {
            let me = Origin::Plugin(p.id.clone());
            let named = |o: &Origin| match o {
                Origin::Plugin(id) => list.iter().find(|q| &q.id == id).map_or(id.clone(), |q| q.name().to_string()),
                Origin::BuiltIn => "Wayfinder".into(),
                Origin::User => "your own files".into(),
            };
            let mut problems: Vec<String> = p.manifest.as_ref().err().cloned().into_iter().collect();
            let dir = p.dir.display().to_string();
            problems.extend(errors.iter().filter(|e| e.starts_with(&dir)).map(|e| e[dir.len()..].trim_start_matches(['\\', '/']).to_string()));
            let mut restyles = Vec::new();
            let mut notes = Vec::new();
            for sh in &cat.shadows {
                let what = format!("{} {}", item_word(sh.item), label(sh.item, &sh.name));
                if sh.winner == me && sh.loser == Origin::BuiltIn {
                    restyles.push(if sh.item == Item::Widget { widget_name(&builtin, &sh.name) } else { sh.name.clone() });
                } else if sh.loser == me && sh.winner == Origin::User {
                    notes.push(format!("Your own files replace its {what}"));
                } else if sh.loser == me {
                    problems.push(format!("Its {what} is hidden: {} has one too, and wins", named(&sh.winner)));
                }
            }
            if !restyles.is_empty() {
                notes.insert(0, format!("Restyles {}", restyles.join(", ")));
            }
            let providers = |w: &str| -> Vec<&Origin> { cat.origins.get(&(Item::Widget, w.to_string())).into_iter().chain(cat.shadows.iter().filter(|s| s.item == Item::Widget && s.name == w).map(|s| &s.loser)).collect() };
            let sole_widgets = p.contents.widgets.iter().filter(|w| providers(w).iter().all(|o| **o == me)).cloned().collect();
            let m = p.manifest.as_ref().ok();
            PluginRow {
                id: p.id.clone(),
                name: p.name().to_string(),
                version: m.map(|m| m.version.clone()).unwrap_or_default(),
                author: m.map(|m| m.author.clone()).unwrap_or_default(),
                description: m.map(|m| m.description.clone()).unwrap_or_default(),
                summary: p.contents.summary(),
                enabled: !disabled.contains(&p.id),
                notes,
                problems,
                sole_widgets,
            }
        })
        .collect()
}

const CONTENT_EXTENSIONS: &[&str] = &["toml", "png", "ttf", "otf", "ttc", "otc"];

/// Whether a change under the data folder can change content. Our own writes
/// (`workspace.json`, the log) and dot-folders (the store's staging) do not.
pub fn is_content_change(data: &Path, p: &Path) -> bool {
    let Ok(rel) = p.strip_prefix(data) else { return false };
    let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    if parts.iter().any(|c| c.starts_with('.')) {
        return false;
    }
    if parts.len() == 2 && parts[0] == "plugins" {
        return true; // a Plugin folder appeared, was renamed or went away
    }
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| CONTENT_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// A `launch` target inside the plugins folder: content must never start a program.
pub fn inside_plugins(data: &Path, target: &str) -> bool {
    let lower = |p: &Path| PathBuf::from(lexical(p).to_string_lossy().to_lowercase().replace('/', "\\"));
    lower(Path::new(target.trim())).starts_with(lower(&data.join("plugins")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::InstanceCfg;

    const OK: &str = "id = 'sunset'\nname = 'Sunset'\nversion = '1.2.0'\nauthor = 'Ada'";

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wf-plugins-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn manifest_needs_id_name_version() {
        let m = Manifest::parse(OK).unwrap();
        assert_eq!((m.id.as_str(), m.name.as_str(), m.version.as_str(), m.author.as_str()), ("sunset", "Sunset", "1.2.0", "Ada"));
        for key in ["id", "name", "version"] {
            let src: String = OK.lines().filter(|l| !l.starts_with(key)).collect::<Vec<_>>().join("\n");
            assert_eq!(Manifest::parse(&src).unwrap_err(), format!("missing `{key}`"));
        }
    }

    #[test]
    fn manifest_rejects_unknown_keys_with_a_suggestion() {
        let e = Manifest::parse(&format!("{OK}\nautor = 'x'")).unwrap_err();
        assert!(e.contains("`autor`") && e.contains("did you mean `author`"), "{e}");
    }

    #[test]
    fn manifest_rejects_code_in_v1() {
        let e = Manifest::parse(&format!("{OK}\n[code]\nmodule = 'x.wasm'")).unwrap_err();
        assert!(e.contains("newer Wayfinder"), "{e}");
    }

    #[test]
    fn manifest_rejects_a_newer_engine() {
        let src = format!("{OK}\nwayfinder = '0.10'");
        assert!(Manifest::parse_for(&src, "0.9.3").unwrap_err().contains("0.10 or newer"));
        assert!(Manifest::parse_for(&src, "0.10.0").is_ok());
        assert!(Manifest::parse_for(&src, "1.0").is_ok());
    }

    #[test]
    fn ids_reject_separators_uppercase_and_reserved_names() {
        for good in ["sunset", "neon-pack", "a1", "x_y"] {
            assert!(valid_id(good).is_ok(), "{good}");
        }
        let long = "x".repeat(65);
        for bad in ["", "Sunset", "a:b", "a|b", "a b", "-lead", "../x", "con", "com1", "lpt9", long.as_str()] {
            assert!(valid_id(bad).is_err(), "{bad}");
        }
        assert!(valid_id("common").is_ok(), "only the exact device names are reserved");
    }

    #[test]
    fn list_skips_dot_dirs_and_reports_a_missing_manifest() {
        let data = tmp("list");
        put(&data, "plugins/sunset/plugin.toml", OK);
        put(&data, "plugins/sunset/widgets/weather.toml", "[root]\ntype = 'box'");
        put(&data, "plugins/bare/widgets/x.toml", "[root]\ntype = 'box'");
        put(&data, "plugins/moved/plugin.toml", OK);
        put(&data, "plugins/.staging-1/plugin.toml", OK);
        let list = PluginStore::new(&data).list();
        let ids: Vec<&str> = list.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["bare", "moved", "sunset"]);
        assert_eq!(list[0].manifest.as_ref().unwrap_err(), "no plugin.toml");
        assert!(list[1].manifest.as_ref().unwrap_err().contains("its folder is `moved`"));
        assert_eq!(list[2].contents.widgets, ["weather"]);
        assert_eq!(list[2].name(), "Sunset");
        std::fs::remove_dir_all(&data).ok();
    }

    #[test]
    fn roots_are_sorted_by_id_and_skip_disabled() {
        let data = tmp("roots");
        for id in ["b", "a", "c"] {
            put(&data, &format!("plugins/{id}/plugin.toml"), &OK.replace("sunset", id));
        }
        put(&data, "plugins/broken/plugin.toml", "id = 'broken'");
        let list = PluginStore::new(&data).list();
        let roots = roots(&list, &BTreeSet::from(["b".to_string()]));
        let ids: Vec<String> = roots.iter().map(|r| format!("{:?}", r.origin)).collect();
        assert_eq!(ids, [r#"Plugin("a")"#, r#"Plugin("c")"#]);
        std::fs::remove_dir_all(&data).ok();
    }

    #[test]
    fn an_instance_is_hidden_only_when_just_a_disabled_plugin_provides_its_widget() {
        let data = tmp("hidden");
        put(&data, "plugins/sunset/plugin.toml", OK);
        put(&data, "plugins/sunset/widgets/weather.toml", "[root]\ntype = 'box'");
        put(&data, "plugins/sunset/widgets/clock.toml", "[root]\ntype = 'box'");
        let list = PluginStore::new(&data).list();
        let inst = |id: &str, w: &str| InstanceCfg { id: id.into(), widget: w.into(), ..Default::default() };
        let mut ws = Workspace::default();
        ws.instances = vec![inst("weather-1", "weather"), inst("clock-1", "clock"), inst("gone-1", "gone")];
        let off = |ws: &Workspace| {
            let reg = content::Catalog::load(&roots(&list, &ws.disabled_plugins)).registry;
            hidden_instances(ws, &reg, &list)
        };
        assert!(off(&ws).is_empty(), "enabled: nothing hidden");
        ws.disabled_plugins.insert("sunset".into());
        assert_eq!(off(&ws), BTreeMap::from([("weather-1".to_string(), "Sunset".to_string())]), "the built-in clock still shows; an unknown widget stays an error card");
        std::fs::remove_dir_all(&data).ok();
    }

    #[test]
    fn content_changes_include_plugin_folders_not_staging() {
        let data = Path::new("C:\\Users\\a\\AppData\\Roaming\\Wayfinder");
        let yes = ["plugins\\sunset", "plugins\\sunset\\widgets\\w.toml", "widgets\\x.toml", "fonts\\a.TTF", "iconpacks\\Neon\\chrome.png"];
        let no = ["workspace.json", "wayfinder.log", "plugins\\.staging-3\\plugin.toml", ".claude\\skills\\x\\SKILL.md", "drawers\\drawer-1\\app.lnk", "plugins"];
        for p in yes {
            assert!(is_content_change(data, &data.join(p)), "{p}");
        }
        for p in no {
            assert!(!is_content_change(data, &data.join(p)), "{p}");
        }
        assert!(!is_content_change(data, Path::new("D:\\elsewhere\\x.toml")));
    }

    #[test]
    fn a_plugin_can_never_launch_its_own_files() {
        let data = Path::new("C:\\Users\\a\\AppData\\Roaming\\Wayfinder");
        assert!(inside_plugins(data, "C:\\Users\\a\\AppData\\Roaming\\Wayfinder\\plugins\\evil\\run.exe"));
        assert!(inside_plugins(data, "c:\\users\\A\\appdata\\roaming\\wayfinder\\PLUGINS\\evil\\run.bat"));
        assert!(inside_plugins(data, "C:\\Users\\a\\AppData\\Roaming\\Wayfinder\\widgets\\..\\plugins\\evil\\x.lnk"));
        assert!(!inside_plugins(data, "C:\\Windows\\notepad.exe"));
        assert!(!inside_plugins(data, "https://example.com"));
        assert!(!inside_plugins(data, "C:\\Users\\a\\AppData\\Roaming\\Wayfinder\\drawers\\drawer-1\\app.lnk"));
    }

    #[test]
    fn rows_say_what_a_plugin_restyles_loses_and_takes_with_it() {
        let data = tmp("rows");
        put(&data, "plugins/sunset/plugin.toml", OK);
        put(&data, "plugins/sunset/widgets/clock.toml", "name = 'Sunset Clock'\n[root]\ntype = 'box'");
        put(&data, "plugins/sunset/widgets/weather.toml", "[root]\ntype = 'box'");
        put(&data, "plugins/sunset/widgets/radar.toml", "[root]\ntype = 'box'");
        put(&data, "plugins/sunset/widgets/bad.toml", "[root]\ntype='text'\ncolour='#fff'");
        put(&data, "plugins/zeta/plugin.toml", &OK.replace("sunset", "zeta").replace("'Sunset'", "'Zeta'"));
        put(&data, "plugins/zeta/widgets/radar.toml", "[root]\ntype = 'box'");
        put(&data, "widgets/weather.toml", "[root]\ntype = 'box'");
        let list = PluginStore::new(&data).list();
        let mut roots = roots(&list, &BTreeSet::new());
        roots.push(Root::user(&data));
        let cat = Catalog::load(&roots);
        let r = &rows(&list, &BTreeSet::new(), &cat)[0];
        assert_eq!((r.name.as_str(), r.version.as_str(), r.summary.as_str(), r.enabled), ("Sunset", "1.2.0", "4 widgets", true));
        assert_eq!(r.notes, ["Restyles Analog Clock", "Your own files replace its widget weather"]);
        assert!(r.problems.iter().any(|p| p.starts_with("widgets") && p.contains("colour")), "{:?}", r.problems);
        assert!(r.problems.iter().any(|p| p.contains("Zeta has one too")), "{:?}", r.problems);
        assert_eq!(r.sole_widgets, ["bad"], "clock is built in, weather is the user's, radar is Zeta's too");
        let mut ws = Workspace::default();
        ws.instances = vec![InstanceCfg { id: "bad-1".into(), widget: "bad".into(), ..Default::default() }, InstanceCfg { id: "clock-1".into(), widget: "clock".into(), ..Default::default() }];
        assert_eq!(r.orphans(&ws), ["bad-1"]);
        std::fs::remove_dir_all(&data).ok();
    }

    #[test]
    fn remove_moves_the_folder_out_then_deletes_it() {
        let data = tmp("remove").join("Wayfinder");
        put(&data, "plugins/sunset/plugin.toml", OK);
        let store = PluginStore::new(&data);
        store.remove("sunset").unwrap();
        assert!(!data.join("plugins").join("sunset").exists());
        assert!(store.remove("sunset").is_err(), "already gone");
        assert!(store.remove("../x").is_err(), "never outside the store");
        let leftover = data.with_file_name("Wayfinder.trash-1-0");
        std::fs::create_dir_all(leftover.join("x")).unwrap();
        store.sweep();
        assert!(!leftover.exists());
        std::fs::remove_dir_all(data.parent().unwrap()).ok();
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(newer("1.10", "1.9") && newer("2", "1.9.9") && !newer("1.2", "1.2.0") && !newer("0.1", "0.1.0"));
    }
}
