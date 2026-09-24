//! Plugins (ADR-0007): packages of content, each a folder under `<data>/plugins/<id>/`
//! laid out like the data folder, with a `plugin.toml` manifest. An enabled Plugin is a
//! content root between the built-ins and the user's own files.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use crate::code::CodeSpec;
use crate::content::{self, Catalog, Contents, Item, Origin, Root};
use crate::code::fs::FsRoot;
use crate::code::launch::LaunchRule;
use crate::net::HostPattern;
use crate::value::Value;
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
    pub code: Option<Code>,
}

/// `[code]`: the module that serves this Plugin's Code Source (ADR-0008).
#[derive(Clone, Debug, PartialEq)]
pub struct Code {
    /// Relative to the Plugin's folder, e.g. `code/weather.wasm`.
    pub module: String,
    /// What its Widgets bind to: `{weather.temp}`.
    pub source: String,
    pub net: Vec<HostPattern>,
    /// Folders under home it may read: `~/.claude`.
    pub fs_read: Vec<FsRoot>,
    /// Params (by name) whose folder, as the user picked it, it may read.
    pub fs_read_params: Vec<String>,
    /// What its Widgets may open besides https:// links.
    pub launch: Vec<LaunchRule>,
    /// Values shown until the first sample.
    pub initial: Value,
}

impl Code {
    /// What it may read, in words: `~/.claude`, `folders you pick for its widgets`.
    pub fn reads(&self) -> Vec<String> {
        let mut r: Vec<String> = self.fs_read.iter().map(|f| f.to_string()).collect();
        if !self.fs_read_params.is_empty() {
            r.push("folders you pick for its widgets".into());
        }
        r
    }
}

const CODE_KEYS: &[&str] = &["module", "source", "net", "fs_read", "fs_read_params", "launch", "initial"];
/// Data Source names and repeat variables a Code Source must not shadow.
const RESERVED_SOURCES: &[&str] = &["clock", "sys", "shortcuts", "param", "state", "self", "item", "index"];

fn parse_code(t: &toml::Table) -> Result<Code, String> {
    for k in t.keys() {
        if !CODE_KEYS.contains(&k.as_str()) {
            return Err(format!("unknown key `code.{k}`{}", crate::format::suggest(k, &[CODE_KEYS])));
        }
    }
    let text = |k: &str| t.get(k).and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| format!("missing `code.{k}`"));
    let module = text("module")?.replace('\\', "/");
    let inside = !module.starts_with('/') && !module.contains(':') && Path::new(&module).components().all(|c| matches!(c, Component::Normal(_)));
    if !module.ends_with(".wasm") || !inside {
        return Err(format!("code.module `{module}` must be a .wasm file inside the plugin"));
    }
    let source = text("source")?.to_string();
    let ident = source.chars().next().is_some_and(|c| c.is_ascii_lowercase()) && source.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !ident {
        return Err(format!("code.source `{source}`: use lower-case letters, digits and `_`, starting with a letter"));
    }
    if RESERVED_SOURCES.contains(&source.as_str()) {
        return Err(format!("code.source `{source}` is taken by Wayfinder"));
    }
    let net = match t.get("net") {
        None => vec![],
        Some(v) => v.as_array().ok_or("code.net must be a list of hosts")?.iter().map(|h| h.as_str().ok_or("code.net must be a list of hosts".to_string()).and_then(HostPattern::parse)).collect::<Result<_, _>>()?,
    };
    let list = |k: &str| -> Result<Vec<String>, String> {
        match t.get(k) {
            None => Ok(vec![]),
            Some(v) => v.as_array().and_then(|a| a.iter().map(|x| x.as_str().map(String::from)).collect()).ok_or(format!("code.{k} must be a list of text")),
        }
    };
    let fs_read = list("fs_read")?.iter().map(|s| FsRoot::parse(s).map_err(|e| format!("code.fs_read {e}"))).collect::<Result<Vec<_>, _>>()?;
    let launch = list("launch")?.iter().map(|s| LaunchRule::parse(s)).collect::<Result<Vec<_>, _>>()?;
    let fs_read_params = list("fs_read_params")?;
    if let Some(p) = fs_read_params.iter().find(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')) {
        return Err(format!("code.fs_read_params: `{p}` is not a param name"));
    }
    let initial = match t.get("initial") {
        None => Value::Obj(BTreeMap::new()),
        Some(v @ toml::Value::Table(_)) => Value::from(v),
        Some(_) => return Err("code.initial must be a table".into()),
    };
    Ok(Code { module, source, net, fs_read, fs_read_params, launch, initial })
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
        let code = match t.get("code") {
            None => None,
            Some(toml::Value::Table(c)) => Some(parse_code(c)?),
            Some(_) => return Err("`code` must be a table".into()),
        };
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
        Ok(Manifest { id, name: need("name")?, version: need("version")?, author: text("author")?.unwrap_or_default(), description: text("description")?.unwrap_or_default(), homepage: text("homepage")?, wayfinder, code })
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

    /// Installs a `.wfplugin` (a zip) or a Plugin folder, replacing an installed one with
    /// the same id. It is unpacked next to the data folder and moved in only once complete.
    pub fn install(&self, src: &Path) -> Result<Manifest, String> {
        self.install_with(src, &Limits::default())
    }

    pub fn install_with(&self, src: &Path, limits: &Limits) -> Result<Manifest, String> {
        let staging = self.scratch("install");
        let done = (|| {
            if src.is_dir() {
                copy_folder(src, &staging, limits, &mut Budget::default())?;
            } else {
                unzip(src, &staging, limits)?;
            }
            let top = plugin_top(&staging)?;
            let m = std::fs::read_to_string(top.join(MANIFEST)).map_err(|e| e.to_string()).and_then(|s| Manifest::parse(&s)).map_err(|e| format!("{MANIFEST}: {e}"))?;
            self.put_in_place(&top, &m.id)?;
            Ok(m)
        })();
        let _ = std::fs::remove_dir_all(&staging);
        done
    }

    /// The old version moves out first and comes back if the new one cannot move in.
    fn put_in_place(&self, from: &Path, id: &str) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let target = self.dir.join(id);
        let old = match target.exists() {
            true => {
                let t = self.scratch("trash");
                rename_retrying(&target, &t).map_err(|e| format!("could not replace the installed {id}: {e}"))?;
                Some(t)
            }
            false => None,
        };
        if let Err(e) = rename_retrying(from, &target) {
            if let Some(t) = &old {
                let _ = rename_retrying(t, &target);
            }
            return Err(format!("could not install {id}: {e}"));
        }
        if let Some(t) = old {
            let _ = std::fs::remove_dir_all(t);
        }
        Ok(())
    }

    /// Every Plugin folder, sorted by id. Dot-folders are the store's own.
    pub fn list(&self) -> Vec<Plugin> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return vec![] };
        let mut dirs: Vec<(String, PathBuf)> = rd.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).filter_map(|e| Some((e.file_name().into_string().ok()?, e.path()))).filter(|(n, _)| !n.starts_with('.')).collect();
        dirs.sort();
        dirs.into_iter().map(|(id, dir)| Plugin { manifest: read_manifest(&id, &dir), contents: content::scan(&dir), id, dir }).collect()
    }
}

/// A plugin file's manifest and what it holds, read without installing it (for a prompt).
pub fn describe(src: &Path) -> Result<(Manifest, Contents), String> {
    if src.is_dir() {
        let m = std::fs::read_to_string(src.join(MANIFEST)).map_err(|_| format!("no {MANIFEST}"))?;
        return Ok((Manifest::parse(&m).map_err(|e| format!("{MANIFEST}: {e}"))?, content::scan(src)));
    }
    let f = std::fs::File::open(src).map_err(|e| format!("{}: {e}", src.display()))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| format!("not a plugin file ({e})"))?;
    let names: Vec<String> = z.file_names().map(|n| n.replace('\\', "/")).filter(|n| !n.split('/').any(clutter)).collect();
    let top = if names.iter().any(|n| n == MANIFEST) {
        String::new()
    } else {
        let firsts: BTreeSet<&str> = names.iter().filter_map(|n| n.split_once('/').map(|(a, _)| a)).collect();
        match firsts.into_iter().collect::<Vec<_>>().as_slice() {
            [one] if names.iter().any(|n| *n == format!("{one}/{MANIFEST}")) => format!("{one}/"),
            _ => return Err(format!("no {MANIFEST} at the top of the plugin")),
        }
    };
    let mut src_text = String::new();
    z.by_name(&format!("{top}{MANIFEST}")).map_err(|e| e.to_string())?.take(64 << 10).read_to_string(&mut src_text).map_err(|e| e.to_string())?;
    let m = Manifest::parse(&src_text).map_err(|e| format!("{MANIFEST}: {e}"))?;
    let mut c = Contents::default();
    for n in &names {
        let Some(rel) = n.strip_prefix(&top) else { continue };
        let parts: Vec<&str> = rel.split('/').collect();
        let stem = || Path::new(parts[1]).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match parts.as_slice() {
            ["widgets", f] if f.ends_with(".toml") => c.widgets.push(stem()),
            ["palettes", f] if f.ends_with(".toml") => c.palettes.push(stem()),
            ["fonts", f] if f.ends_with(".toml") => c.fonts.push(stem()),
            ["glyphs", f] if f.ends_with(".toml") => c.glyphs.push(stem()),
            ["iconpacks", pack, ..] if !pack.is_empty() && !c.icon_packs.contains(&pack.to_string()) => c.icon_packs.push(pack.to_string()),
            _ => {}
        }
    }
    Ok((m, c))
}

/// Caps on what one install may write, so a broken or hostile archive cannot fill the disk.
#[derive(Clone, Debug)]
pub struct Limits {
    pub entries: usize,
    pub file_bytes: u64,
    pub total_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { entries: 4000, file_bytes: 32 << 20, total_bytes: 256 << 20 }
    }
}

#[derive(Default)]
struct Budget {
    entries: usize,
    bytes: u64,
    names: HashSet<String>,
}

impl Budget {
    /// Counts one file at `rel`; two names that differ only in case would overwrite each other on Windows.
    fn file(&mut self, rel: &Path, limits: &Limits) -> Result<(), String> {
        self.entries += 1;
        if self.entries > limits.entries {
            return Err(format!("more than {} files", limits.entries));
        }
        if !self.names.insert(rel.to_string_lossy().to_lowercase().replace('\\', "/")) {
            return Err(format!("{} appears twice", rel.display()));
        }
        Ok(())
    }

    fn bytes(&mut self, n: u64, rel: &Path, limits: &Limits) -> Result<(), String> {
        if n > limits.file_bytes {
            return Err(format!("{} is larger than {} MB", rel.display(), limits.file_bytes >> 20));
        }
        self.bytes += n;
        if self.bytes > limits.total_bytes {
            return Err(format!("larger than {} MB in all", limits.total_bytes >> 20));
        }
        Ok(())
    }
}

/// Content only: definitions, images, fonts and notes. Nothing a `launch` could run.
const ALLOWED: &[&str] = &["toml", "png", "jpg", "jpeg", "gif", "webp", "bmp", "ttf", "otf", "ttc", "otc", "md", "txt", "wasm"];
const ALLOWED_BARE: &[&str] = &["license", "licence", "readme", "notice", "copying", "authors"];

fn allowed_file(name: &str) -> bool {
    let p = Path::new(name);
    match p.extension().and_then(|e| e.to_str()) {
        Some(e) => ALLOWED.contains(&e.to_ascii_lowercase().as_str()),
        None => p.file_stem().and_then(|s| s.to_str()).is_some_and(|s| ALLOWED_BARE.contains(&s.to_ascii_lowercase().as_str())),
    }
}

/// Archive and OS clutter, skipped rather than refused.
fn clutter(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with('.') || n == "__macosx" || n == "thumbs.db" || n == "desktop.ini"
}

fn unzip(src: &Path, dest: &Path, limits: &Limits) -> Result<(), String> {
    let f = std::fs::File::open(src).map_err(|e| format!("{}: {e}", src.display()))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| format!("not a plugin file ({e})"))?;
    if z.len() > limits.entries {
        return Err(format!("more than {} files", limits.entries));
    }
    let mut budget = Budget::default();
    for i in 0..z.len() {
        let mut e = z.by_index(i).map_err(|e| e.to_string())?;
        let name = e.name().to_string();
        // enclosed_name quietly strips a root or drive; a name with either is refused instead
        let absolute = name.starts_with(['/', '\\']) || name.contains(':');
        let rel = e.enclosed_name().filter(|_| !absolute).ok_or_else(|| format!("`{name}` points outside the plugin"))?;
        let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        if parts.iter().any(|c| clutter(c)) || e.is_symlink() {
            continue;
        }
        let out = dest.join(&rel);
        if e.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if !allowed_file(&name) {
            return Err(format!("`{name}`: a plugin may only hold {} files", ALLOWED.join(", ")));
        }
        budget.file(&rel, limits)?;
        budget.bytes(e.size(), &rel, limits)?;
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut w = std::fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        // the header's size is only a claim: never read past the cap
        let n = std::io::copy(&mut (&mut e).take(limits.file_bytes + 1), &mut w).map_err(|e| format!("{name}: {e}"))?;
        budget.bytes(n.saturating_sub(e.size()), &rel, limits)?;
        if n > limits.file_bytes {
            return Err(format!("{} is larger than {} MB", rel.display(), limits.file_bytes >> 20));
        }
    }
    Ok(())
}

fn copy_folder(src: &Path, dest: &Path, limits: &Limits, budget: &mut Budget) -> Result<(), String> {
    fn walk(root: &Path, dir: &Path, dest: &Path, limits: &Limits, budget: &mut Budget) -> Result<(), String> {
        let rd = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for e in rd.filter_map(|e| e.ok()) {
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(meta) = std::fs::symlink_metadata(e.path()) else { continue };
            if clutter(&name) || meta.file_type().is_symlink() {
                continue;
            }
            let rel = e.path().strip_prefix(root).map(Path::to_path_buf).unwrap_or_default();
            if meta.is_dir() {
                walk(root, &e.path(), dest, limits, budget)?;
                continue;
            }
            if !allowed_file(&name) {
                return Err(format!("`{}`: a plugin may only hold {} files", rel.display(), ALLOWED.join(", ")));
            }
            budget.file(&rel, limits)?;
            budget.bytes(meta.len(), &rel, limits)?;
            let out = dest.join(&rel);
            std::fs::create_dir_all(out.parent().unwrap_or(dest)).map_err(|e| e.to_string())?;
            std::fs::copy(e.path(), &out).map_err(|e| format!("{}: {e}", rel.display()))?;
        }
        Ok(())
    }
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    walk(src, src, dest, limits, budget)
}

/// The folder holding `plugin.toml`: the top, or its one folder ("Send to > Compressed
/// folder" zips the folder itself).
fn plugin_top(staging: &Path) -> Result<PathBuf, String> {
    if staging.join(MANIFEST).is_file() {
        return Ok(staging.to_path_buf());
    }
    let entries: Vec<PathBuf> = std::fs::read_dir(staging).map_err(|e| e.to_string())?.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    match entries.as_slice() {
        [one] if one.join(MANIFEST).is_file() => Ok(one.clone()),
        _ => Err(format!("no {MANIFEST} at the top of the plugin")),
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
    /// "Runs code as `weather` · can reach api.open-meteo.com"; empty without code.
    pub code: String,
    /// Its Code Source's name while it runs, to match the live status to the row.
    pub code_source: Option<String>,
    /// How the code is doing, kept current by the app.
    pub status: String,
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
    let (_, code_lost) = code_specs(list, disabled);
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
            let code = m.and_then(|m| m.code.as_ref());
            if let (Some(c), Some(winner)) = (code, code_lost.get(&p.id)) {
                problems.push(format!("Its data source `{}` is hidden: {} has one too, and wins", c.source, named(&Origin::Plugin(winner.clone()))));
            }
            let code_line = code.map_or(String::new(), |c| {
                let reach = if c.net.is_empty() { "no network".to_string() } else { format!("can reach {}", c.net.iter().map(|h| h.to_string()).collect::<Vec<_>>().join(", ")) };
                let reads = c.reads();
                let reads = if reads.is_empty() { String::new() } else { format!(" · reads {}", reads.join(", ")) };
                let opens = if c.launch.is_empty() { String::new() } else { format!(" · opens {}", c.launch.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(", ")) };
                format!("Runs code as `{}` · {reach}{reads}{opens}", c.source)
            });
            let runs = code.is_some() && !disabled.contains(&p.id) && !code_lost.contains_key(&p.id);
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
                code: code_line,
                code_source: code.filter(|_| runs).map(|c| c.source.clone()),
                status: String::new(),
            }
        })
        .collect()
}

const CONTENT_EXTENSIONS: &[&str] = &["toml", "png", "jpg", "jpeg", "gif", "webp", "bmp", "ttf", "otf", "ttc", "otc", "wasm"];

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

/// A `launch` target inside the plugins folder or their saved data: content must never
/// start a program.
pub fn inside_plugins(data: &Path, target: &str) -> bool {
    let lower = |p: &Path| PathBuf::from(lexical(p).to_string_lossy().to_lowercase().replace('/', "\\"));
    let t = lower(Path::new(target.trim()));
    t.starts_with(lower(&data.join("plugins"))) || t.starts_with(lower(&data.join(DATA_DIR)))
}

/// Where Plugins keep their saved data: `<data>/plugin-data/<id>.json`.
pub const DATA_DIR: &str = "plugin-data";

pub fn data_file(data: &Path, id: &str) -> PathBuf {
    data.join(DATA_DIR).join(format!("{id}.json"))
}

/// The Code Sources to run: enabled Plugins with a valid `[code]`, keyed so that an unchanged
/// one keeps running. Two with one source name follow ADR-0007: the later id wins. Returns
/// (key, spec) pairs and, per losing Plugin id, the winner's id.
pub fn code_specs(list: &[Plugin], disabled: &BTreeSet<String>) -> (Vec<(String, CodeSpec)>, BTreeMap<String, String>) {
    let mut by_source: BTreeMap<String, (String, CodeSpec)> = BTreeMap::new();
    let mut lost = BTreeMap::new();
    for p in list.iter().filter(|p| !disabled.contains(&p.id)) {
        let Some(code) = p.manifest.as_ref().ok().and_then(|m| m.code.as_ref()) else { continue };
        let module = p.dir.join(&code.module);
        let stamp = std::fs::metadata(&module).map(|m| (m.len(), m.modified().ok())).ok();
        let key = format!("{}|{:?}|{:?}", p.id, code, stamp);
        let spec = CodeSpec { plugin: p.id.clone(), source: code.source.clone(), module, hosts: code.net.clone(), fs_read: code.fs_read.clone(), fs_read_params: code.fs_read_params.clone(), launch: code.launch.clone(), initial: code.initial.clone() };
        if let Some((_, old)) = by_source.insert(code.source.clone(), (key, spec)) {
            lost.insert(old.plugin, p.id.clone());
        }
    }
    (by_source.into_values().collect(), lost)
}

/// The question before installing: what it adds and, for code, what it may reach.
pub fn install_question(m: &Manifest, contents: &Contents, installed: Option<&Manifest>) -> String {
    let by = if m.author.is_empty() { String::new() } else { format!(" by {}", m.author) };
    let verb = match installed {
        Some(old) => format!("Replace {} {} with {}", old.name, old.version, m.version),
        None => format!("Install {} {}{by}", m.name, m.version),
    };
    let about = if m.description.is_empty() { String::new() } else { format!("\n{}", m.description) };
    let mut q = format!("{verb}?\n\n{}{about}\n\n", contents.summary());
    match &m.code {
        None => q += "Plugins add widgets, themes, fonts and icons, and never run programs.",
        Some(code) => {
            let reads = code.reads();
            q += if reads.is_empty() { "This plugin runs sandboxed code. It cannot read your files or start programs." } else { "This plugin runs sandboxed code. It cannot change your files or start programs." };
            if !reads.is_empty() {
                q += &format!(" It can read files in: {}.", reads.join(", "));
            }
            let hosts: Vec<String> = code.net.iter().map(|h| h.to_string()).collect();
            if !hosts.is_empty() {
                q += &format!(" It can connect to: {}.", hosts.join(", "));
            }
            if !code.launch.is_empty() {
                q += &format!(" Its widgets can open {}.", code.launch.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(", "));
            }
            let reach = |c: &Code| c.net.iter().map(|h| h.to_string()).chain(c.reads()).chain(c.launch.iter().map(|l| l.to_string())).collect::<Vec<_>>();
            let before = installed.and_then(|o| o.code.as_ref()).map(reach).unwrap_or_default();
            let new: Vec<String> = reach(code).into_iter().filter(|h| !before.contains(h)).collect();
            if installed.is_some() && !new.is_empty() {
                q += &format!(" New in this version: {}.", new.join(", "));
            }
        }
    }
    q + " Only install plugins you trust."
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
    fn manifest_reads_a_code_table() {
        let src = format!("{OK}\n[code]\nmodule = 'code\\weather.wasm'\nsource = 'weather'\nnet = ['api.open-meteo.com', '*.example.org']\n[code.initial]\ntemp = 0\nsky = '...'");
        let c = Manifest::parse(&src).unwrap().code.expect("a code table");
        assert_eq!((c.module.as_str(), c.source.as_str()), ("code/weather.wasm", "weather"));
        assert_eq!(c.net.iter().map(|h| h.to_string()).collect::<Vec<_>>(), ["api.open-meteo.com", "*.example.org"]);
        assert_eq!(c.initial.get("sky").map(|v| v.to_string()), Some("...".into()));
        assert_eq!(Manifest::parse(OK).unwrap().code, None);
    }

    #[test]
    fn code_may_read_declared_folders_and_picked_ones() {
        let src = format!("{OK}\n[code]\nmodule = 'a.wasm'\nsource = 'agents'\nfs_read = ['~/.claude', '~\\.copilot']\nfs_read_params = ['folder']");
        let c = Manifest::parse(&src).unwrap().code.unwrap();
        assert_eq!(c.reads(), ["~/.claude", "~/.copilot", "folders you pick for its widgets"]);
        let with = |code: &str| Manifest::parse(&format!("{OK}\n[code]\nmodule = 'a.wasm'\nsource = 'a'\n{code}")).unwrap_err();
        assert!(with("fs_read = ['~']").contains("~/"));
        assert!(with("fs_read = ['C:\\Windows']").contains("~/"));
        assert!(with("fs_read = ['~/../x']").contains("plain folder"));
        assert!(with("fs_read = '~/.claude'").contains("list"));
        assert!(with("fs_read_params = ['a b']").contains("param name"));
        let data = tmp("coderead");
        put(&data, "plugins/agents/plugin.toml", &src.replace("sunset", "agents"));
        let list = PluginStore::new(&data).list();
        let cat = Catalog::load(&roots(&list, &BTreeSet::new()));
        assert_eq!(rows(&list, &BTreeSet::new(), &cat)[0].code, "Runs code as `agents` · no network · reads ~/.claude, ~/.copilot, folders you pick for its widgets");
        let spec = &code_specs(&list, &BTreeSet::new()).0[0].1;
        assert_eq!((spec.fs_read.len(), spec.fs_read_params.as_slice()), (2, &["folder".to_string()][..]));
        std::fs::remove_dir_all(&data).ok();
    }

    #[test]
    fn install_question_names_what_code_may_read() {
        let c = Contents::default();
        let v1 = Manifest::parse(&format!("{OK}\n[code]\nmodule = 'w.wasm'\nsource = 'w'\nfs_read = ['~/.claude']")).unwrap();
        let q = install_question(&v1, &c, None);
        assert!(q.contains("cannot change your files") && q.contains("can read files in: ~/.claude.") && !q.contains("connect"), "{q}");
        let v2 = Manifest::parse(&format!("{OK}\n[code]\nmodule = 'w.wasm'\nsource = 'w'\nfs_read = ['~/.claude', '~/.gemini']")).unwrap();
        assert!(install_question(&v2, &c, Some(&v1)).contains("New in this version: ~/.gemini."));
    }

    #[test]
    fn code_may_let_its_widgets_open_more_than_web_links() {
        let v1 = Manifest::parse(&format!("{OK}\n[code]\nmodule = 'a.wasm'\nsource = 'agents'\nlaunch = ['vscode', '~/.claude']")).unwrap();
        assert_eq!(v1.code.as_ref().unwrap().launch.iter().map(|l| l.to_string()).collect::<Vec<_>>(), ["vscode: links", "folders and documents in ~/.claude"]);
        let q = install_question(&v1, &Contents::default(), None);
        assert!(q.contains("Its widgets can open vscode: links, folders and documents in ~/.claude."), "{q}");
        let v2 = Manifest::parse(&format!("{OK}\n[code]\nmodule = 'a.wasm'\nsource = 'agents'\nlaunch = ['vscode', '~/.claude', 'slack']")).unwrap();
        assert!(install_question(&v2, &Contents::default(), Some(&v1)).contains("New in this version: slack: links."));
        let bad = Manifest::parse(&format!("{OK}\n[code]\nmodule = 'a.wasm'\nsource = 'a'\nlaunch = ['ms-msdt']")).unwrap_err();
        assert!(bad.contains("never"), "{bad}");
    }

    #[test]
    fn code_table_rejects_bad_sources_hosts_and_paths() {
        let with = |code: &str| Manifest::parse(&format!("{OK}\n[code]\n{code}")).unwrap_err();
        assert!(with("source = 'weather'").contains("code.module"));
        assert!(with("module = 'w.wasm'").contains("code.source"));
        for (bad, why) in [
            ("module = '../w.wasm'\nsource = 'w'", "inside"),
            ("module = 'C:/w.wasm'\nsource = 'w'", "inside"),
            ("module = 'w.dll'\nsource = 'w'", ".wasm"),
            ("module = 'w.wasm'\nsource = 'Weather'", "lower-case"),
            ("module = 'w.wasm'\nsource = 'my-weather'", "lower-case"),
            ("module = 'w.wasm'\nsource = 'clock'", "taken"),
            ("module = 'w.wasm'\nsource = 'item'", "taken"),
            ("module = 'w.wasm'\nsource = 'w'\nnet = ['127.0.0.1']", "IP"),
            ("module = 'w.wasm'\nsource = 'w'\nnet = ['*']", "wildcard"),
            ("module = 'w.wasm'\nsource = 'w'\nnet = 'x.com'", "list"),
            ("module = 'w.wasm'\nsource = 'w'\nmodul = 'x'", "did you mean `module`"),
        ] {
            let e = with(bad);
            assert!(e.contains(why), "{bad}: {e}");
        }
    }

    fn code_plugin(data: &Path, id: &str, source: &str) {
        put(data, &format!("plugins/{id}/plugin.toml"), &format!("{}\n[code]\nmodule = 'code/m.wasm'\nsource = '{source}'\nnet = ['api.open-meteo.com']", OK.replace("sunset", id)));
        put(data, &format!("plugins/{id}/code/m.wasm"), "(module)");
    }

    #[test]
    fn code_specs_skip_disabled_and_collide_like_content() {
        let data = tmp("specs");
        code_plugin(&data, "alpha", "weather");
        code_plugin(&data, "beta", "weather");
        code_plugin(&data, "gamma", "news");
        put(&data, "plugins/plain/plugin.toml", &OK.replace("sunset", "plain"));
        let list = PluginStore::new(&data).list();
        let (specs, lost) = code_specs(&list, &BTreeSet::from(["gamma".to_string()]));
        let who: Vec<(&str, &str)> = specs.iter().map(|(_, s)| (s.source.as_str(), s.plugin.as_str())).collect();
        assert_eq!(who, [("weather", "beta")], "gamma is off; beta, the later id, wins weather");
        assert_eq!(lost, BTreeMap::from([("alpha".to_string(), "beta".to_string())]));
        assert_eq!(specs[0].1.module, data.join("plugins/beta/code/m.wasm"));
        let key = specs[0].0.clone();
        assert_eq!(code_specs(&list, &BTreeSet::from(["gamma".to_string()])).0[0].0, key, "same files, same key");
        std::thread::sleep(std::time::Duration::from_millis(20));
        put(&data, "plugins/beta/code/m.wasm", "(module) ;; rebuilt");
        assert_ne!(code_specs(&PluginStore::new(&data).list(), &BTreeSet::new()).0.iter().find(|(_, s)| s.source == "weather").unwrap().0, key, "a rebuilt module restarts");
        std::fs::remove_dir_all(&data).ok();
    }

    #[test]
    fn rows_say_what_code_runs_and_reaches() {
        let data = tmp("coderows");
        code_plugin(&data, "alpha", "weather");
        code_plugin(&data, "beta", "weather");
        let list = PluginStore::new(&data).list();
        let cat = Catalog::load(&roots(&list, &BTreeSet::new()));
        let r = rows(&list, &BTreeSet::new(), &cat);
        assert_eq!(r[1].code, "Runs code as `weather` · can reach api.open-meteo.com");
        assert_eq!((r[0].code_source.as_deref(), r[1].code_source.as_deref()), (None, Some("weather")));
        assert!(r[0].problems.iter().any(|p| p.contains("data source `weather` is hidden") && p.contains("wins")), "{:?}", r[0].problems);
        std::fs::remove_dir_all(&data).ok();
    }

    #[test]
    fn install_question_names_hosts_and_new_hosts() {
        let plain = Manifest::parse(OK).unwrap();
        let c = Contents { widgets: vec!["weather".into()], ..Default::default() };
        let q = install_question(&plain, &c, None);
        assert!(q.starts_with("Install Sunset 1.2.0 by Ada?") && q.contains("1 widget") && q.contains("never run programs"), "{q}");
        let v1 = Manifest::parse(&format!("{OK}\n[code]\nmodule = 'w.wasm'\nsource = 'w'\nnet = ['api.open-meteo.com']")).unwrap();
        let q = install_question(&v1, &c, None);
        assert!(q.contains("runs sandboxed code") && q.contains("can connect to: api.open-meteo.com") && q.contains("trust"), "{q}");
        let mut v2 = Manifest::parse(&format!("{OK}\n[code]\nmodule = 'w.wasm'\nsource = 'w'\nnet = ['api.open-meteo.com', 'news.example.com']")).unwrap();
        v2.version = "1.3.0".into();
        let q = install_question(&v2, &c, Some(&v1));
        assert!(q.starts_with("Replace Sunset 1.2.0 with 1.3.0?") && q.contains("New in this version: news.example.com."), "{q}");
    }

    #[test]
    fn plugin_data_is_never_launched() {
        let data = Path::new("C:\\Users\\a\\AppData\\Roaming\\Wayfinder");
        assert!(inside_plugins(data, "C:\\Users\\a\\AppData\\Roaming\\Wayfinder\\plugin-data\\x.exe"));
        assert_eq!(data_file(data, "sunset"), data.join("plugin-data").join("sunset.json"));
    }

    #[test]
    fn wasm_changes_reload() {
        let data = Path::new("C:\\Users\\a\\AppData\\Roaming\\Wayfinder");
        assert!(is_content_change(data, &data.join("plugins\\sunset\\code\\weather.wasm")));
        assert!(is_content_change(data, &data.join("plugins\\sunset\\images\\dusk.GIF")), "any image a widget can show");
        assert!(!is_content_change(data, &data.join("plugin-data\\sunset.json")), "saved data is not content");
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

    fn zip_of(path: &Path, files: &[(&str, &str)]) {
        use std::io::Write;
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, text) in files {
            z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(text.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }

    /// A data folder with room around it, and a way to see what an install left next to it.
    fn install_area(name: &str) -> (PathBuf, PluginStore) {
        let data = tmp(name).join("Wayfinder");
        std::fs::create_dir_all(&data).unwrap();
        let store = PluginStore::new(&data);
        (data, store)
    }

    fn leftovers(data: &Path) -> Vec<String> {
        std::fs::read_dir(data.parent().unwrap()).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n != "Wayfinder" && !n.ends_with(".wfplugin") && !n.ends_with(".zip")).collect()
    }

    #[test]
    fn installs_a_zip_into_plugins_id() {
        let (data, store) = install_area("inst");
        let file = data.with_file_name("sunset.wfplugin");
        zip_of(&file, &[("plugin.toml", OK), ("widgets/weather.toml", "[root]\ntype = 'box'"), ("fonts/Sunset.ttf", "font"), ("README", "hi")]);
        let m = store.install(&file).unwrap();
        assert_eq!((m.id.as_str(), m.version.as_str()), ("sunset", "1.2.0"));
        assert!(data.join("plugins/sunset/widgets/weather.toml").is_file());
        assert_eq!(store.list()[0].contents.widgets, ["weather"]);
        assert!(leftovers(&data).is_empty(), "{:?}", leftovers(&data));
        std::fs::remove_dir_all(data.parent().unwrap()).ok();
    }

    #[test]
    fn accepts_one_top_level_folder() {
        let (data, store) = install_area("top");
        let file = data.with_file_name("Sunset.zip");
        zip_of(&file, &[("Sunset/plugin.toml", OK), ("Sunset/palettes/p.toml", "name = 'Sunset'"), ("__MACOSX/Sunset/._plugin.toml", "junk"), ("Sunset/.DS_Store", "junk")]);
        store.install(&file).unwrap();
        assert!(data.join("plugins/sunset/palettes/p.toml").is_file(), "installed by its id, not the zip's folder name");
        assert!(!data.join("plugins/sunset/.DS_Store").exists());
        std::fs::remove_dir_all(data.parent().unwrap()).ok();
    }

    #[test]
    fn installs_a_folder_too() {
        let (data, store) = install_area("folder");
        let src = data.with_file_name("dev-sunset");
        put(&src, "plugin.toml", OK);
        put(&src, "widgets/w.toml", "[root]\ntype = 'box'");
        put(&src, ".git/config", "not copied");
        store.install(&src).unwrap();
        assert!(data.join("plugins/sunset/widgets/w.toml").is_file() && !data.join("plugins/sunset/.git").exists());
        std::fs::remove_dir_all(data.parent().unwrap()).ok();
    }

    #[test]
    fn rejects_zip_slip_absolute_and_ads_names() {
        let (data, store) = install_area("slip");
        for bad in ["../evil.toml", "widgets/../../evil.toml", "/evil.toml", "C:/evil.toml", "widgets/x.toml:hidden"] {
            let file = data.with_file_name("bad.zip");
            zip_of(&file, &[("plugin.toml", OK), (bad, "x")]);
            assert!(store.install(&file).is_err(), "{bad}");
        }
        assert!(!data.parent().unwrap().join("evil.toml").exists() && !data.join("evil.toml").exists());
        assert!(!data.join("plugins/sunset").exists());
        std::fs::remove_dir_all(data.parent().unwrap()).ok();
    }

    #[test]
    fn rejects_disallowed_file_types() {
        let (data, store) = install_area("types");
        for good in ["images/a.jpg", "images/b.JPEG", "images/c.gif", "images/d.webp", "images/e.bmp"] {
            assert!(allowed_file(good), "{good}");
        }
        for bad in ["tools/run.exe", "widgets/app.lnk", "x.bat", "script", "images/x.svg"] {
            let file = data.with_file_name("bad.zip");
            zip_of(&file, &[("plugin.toml", OK), (bad, "x")]);
            let e = store.install(&file).unwrap_err();
            assert!(e.contains(bad), "{bad}: {e}");
        }
        let file = data.with_file_name("dup.zip");
        zip_of(&file, &[("plugin.toml", OK), ("widgets/A.toml", "x"), ("widgets/a.toml", "y")]);
        assert!(store.install(&file).unwrap_err().contains("twice"));
        std::fs::remove_dir_all(data.parent().unwrap()).ok();
    }

    #[test]
    fn rejects_archives_over_the_limits() {
        let (data, store) = install_area("limits");
        let file = data.with_file_name("big.zip");
        zip_of(&file, &[("plugin.toml", OK), ("a.txt", "0123456789"), ("b.txt", "0123456789")]);
        assert!(store.install_with(&file, &Limits { entries: 2, ..Limits::default() }).unwrap_err().contains("more than 2"));
        assert!(store.install_with(&file, &Limits { file_bytes: 5, ..Limits::default() }).is_err());
        assert!(store.install_with(&file, &Limits { total_bytes: 25, ..Limits::default() }).unwrap_err().contains("in all"));
        assert!(store.install_with(&file, &Limits::default()).is_ok());
        let not_zip = data.with_file_name("notes.wfplugin");
        std::fs::write(&not_zip, "hello").unwrap();
        assert!(store.install(&not_zip).unwrap_err().contains("not a plugin file"));
        assert!(leftovers(&data).is_empty(), "{:?}", leftovers(&data));
        std::fs::remove_dir_all(data.parent().unwrap()).ok();
    }

    #[test]
    fn describe_reads_a_plugin_file_without_installing_it() {
        let (data, store) = install_area("describe");
        let file = data.with_file_name("sunset.wfplugin");
        zip_of(&file, &[("Sunset/plugin.toml", OK), ("Sunset/widgets/weather.toml", "x"), ("Sunset/palettes/a.toml", "x"), ("Sunset/iconpacks/Neon/chrome.png", "x"), ("Sunset/iconpacks/Neon/edge.png", "x")]);
        let (m, c) = describe(&file).unwrap();
        assert_eq!((m.name.as_str(), c.summary().as_str()), ("Sunset", "1 widget · 1 palette · 1 icon pack"));
        assert!(store.list().is_empty(), "nothing installed");
        let bare = data.with_file_name("bare.zip");
        zip_of(&bare, &[("a/plugin.toml", OK), ("b/x.toml", "x")]);
        assert!(describe(&bare).is_err());
        std::fs::remove_dir_all(data.parent().unwrap()).ok();
    }

    #[test]
    fn upgrade_replaces_files_and_leaves_no_staging_or_trash() {
        let (data, store) = install_area("upgrade");
        let v1 = data.with_file_name("v1.zip");
        zip_of(&v1, &[("plugin.toml", OK), ("widgets/old.toml", "[root]\ntype = 'box'")]);
        let v2 = data.with_file_name("v2.zip");
        zip_of(&v2, &[("plugin.toml", &OK.replace("1.2.0", "1.3.0")), ("widgets/new.toml", "[root]\ntype = 'box'")]);
        store.install(&v1).unwrap();
        assert_eq!(store.install(&v2).unwrap().version, "1.3.0");
        assert!(!data.join("plugins/sunset/widgets/old.toml").exists() && data.join("plugins/sunset/widgets/new.toml").is_file());
        assert!(leftovers(&data).is_empty(), "{:?}", leftovers(&data));
        std::fs::remove_dir_all(data.parent().unwrap()).ok();
    }

    #[test]
    fn a_failed_install_keeps_the_old_version() {
        let (data, store) = install_area("keep");
        let v1 = data.with_file_name("v1.zip");
        zip_of(&v1, &[("plugin.toml", OK), ("widgets/old.toml", "[root]\ntype = 'box'")]);
        store.install(&v1).unwrap();
        let bad = data.with_file_name("bad.zip");
        zip_of(&bad, &[("plugin.toml", &OK.replace("1.2.0", "2.0")), ("run.exe", "MZ")]);
        assert!(store.install(&bad).is_err());
        let no_manifest = data.with_file_name("none.zip");
        zip_of(&no_manifest, &[("widgets/x.toml", "[root]\ntype = 'box'")]);
        assert!(store.install(&no_manifest).unwrap_err().contains("no plugin.toml"));
        assert!(data.join("plugins/sunset/widgets/old.toml").is_file());
        assert_eq!(store.list()[0].manifest.as_ref().unwrap().version, "1.2.0");
        std::fs::remove_dir_all(data.parent().unwrap()).ok();
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(newer("1.10", "1.9") && newer("2", "1.9.9") && !newer("1.2", "1.2.0") && !newer("0.1", "0.1.0"));
    }
}
