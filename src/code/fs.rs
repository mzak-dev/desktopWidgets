//! Read-only files for plugin code (ADR-0008): list, stat and read text under the folders its
//! `plugin.toml` declares (`fs_read = ["~/.claude"]`) and the folders the user picked in its
//! widgets' settings (`fs_read_params`). A path is checked twice: against the folders as
//! written, before anything touches the disk, so code cannot probe for files elsewhere; and
//! again once links are resolved, so a link inside a folder cannot lead out of it.
//! Wayfinder's own data folder, which holds every plugin's saved data, is never readable.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

/// The most one `read` returns.
pub const READ_CAP: u64 = 1 << 20;
/// The most entries one `list` returns.
pub const LIST_CAP: usize = 2000;
/// Per call to the module: bytes read and operations, so a call cannot scan a whole disk.
pub const CALL_BYTES: u64 = 16 << 20;
pub const CALL_OPS: usize = 5000;

/// A folder under the user's home that `plugin.toml` may declare: `~/.claude`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FsRoot(String);

fn bad_part(p: &str) -> bool {
    // Windows drops trailing dots and spaces, so `.. ` would mean `..`
    p == "." || p == ".." || p.ends_with('.') || p.ends_with(' ') || p.chars().any(|c| c.is_control() || "<>:\"|?*".contains(c))
}

impl FsRoot {
    pub fn parse(s: &str) -> Result<FsRoot, String> {
        let s = s.trim().replace('\\', "/");
        let Some(rest) = s.strip_prefix("~/") else {
            return Err(format!("code.fs_read `{s}`: start it with ~/ (the user's home folder)"));
        };
        let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
        if parts.is_empty() {
            return Err("code.fs_read: name a folder inside ~/, not the whole home folder".into());
        }
        if let Some(p) = parts.iter().find(|p| bad_part(p)) {
            return Err(format!("code.fs_read `{s}`: `{p}` is not a plain folder name"));
        }
        Ok(FsRoot(format!("~/{}", parts.join("/"))))
    }

    pub fn under(&self, home: &Path) -> PathBuf {
        self.0[2..].split('/').fold(home.to_path_buf(), |p, c| p.join(c))
    }
}

impl std::fmt::Display for FsRoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where the user's files are, from the app.
#[derive(Clone, Debug, Default)]
pub struct Places {
    pub home: Option<PathBuf>,
    /// Never readable, whatever a root says: Wayfinder's data folder.
    pub private: Vec<PathBuf>,
}

/// A path's components for comparing, case-insensitively as Windows does.
fn key(p: &Path) -> Vec<String> {
    p.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect()
}

fn under(p: &Path, root: &Path) -> bool {
    let (p, r) = (key(p), key(root));
    p.len() >= r.len() && p[..r.len()] == r[..]
}

fn ms(t: std::io::Result<std::time::SystemTime>) -> i64 {
    t.ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64)
}

#[derive(serde::Deserialize)]
struct Req {
    op: String,
    path: String,
    #[serde(default)]
    offset: i64,
    max: Option<u64>,
}

/// One Instance's view of the disk for one call.
pub struct Fs {
    home: Option<PathBuf>,
    roots: Vec<PathBuf>,
    private: Vec<PathBuf>,
    ops: usize,
    bytes: u64,
}

fn err(e: impl std::fmt::Display) -> Vec<u8> {
    serde_json::json!({ "error": e.to_string() }).to_string().into_bytes()
}

impl Fs {
    /// `picked` are folders the user chose for this Instance.
    pub fn new(places: &Places, declared: &[FsRoot], picked: &[PathBuf]) -> Fs {
        let mut roots: Vec<PathBuf> = places.home.iter().flat_map(|h| declared.iter().map(move |r| r.under(h))).collect();
        roots.extend(picked.iter().filter(|p| p.is_absolute()).cloned());
        Fs { home: places.home.clone(), roots, private: places.private.clone(), ops: 0, bytes: 0 }
    }

    /// A request (JSON) to its answer (JSON, errors included).
    pub fn handle(&mut self, req: &[u8]) -> Vec<u8> {
        let req: Req = match serde_json::from_slice(req) {
            Ok(r) => r,
            Err(e) => return err(format!("bad file request: {e}")),
        };
        self.ops += 1;
        if self.ops > CALL_OPS || self.bytes > CALL_BYTES {
            return err("it read too much in one call");
        }
        let path = match self.resolve(&req.path) {
            Ok(p) => p,
            Err(e) => return err(e),
        };
        match req.op.as_str() {
            "list" => self.list(&path),
            "stat" => match std::fs::metadata(&path) {
                Ok(m) => serde_json::json!({ "dir": m.is_dir(), "size": m.len(), "modified_ms": ms(m.modified()) }).to_string().into_bytes(),
                Err(e) => err(e),
            },
            "read" => self.read(&path, req.offset, req.max.unwrap_or(READ_CAP).min(READ_CAP)),
            op => err(format!("unknown file operation `{op}`")),
        }
    }

    /// The real path behind `p` (`~/…` or absolute), if it lies inside a root.
    fn resolve(&self, p: &str) -> Result<PathBuf, String> {
        let outside = || format!("`{p}` is outside the folders this plugin may read");
        let path = match p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
            Some(rest) => self.home.as_ref().ok_or("there is no home folder")?.join(rest),
            None => PathBuf::from(p),
        };
        if !path.is_absolute() {
            return Err(format!("`{p}`: use a full path or one starting with ~/"));
        }
        for c in path.components() {
            match c {
                Component::Normal(s) if bad_part(&s.to_string_lossy()) => return Err(outside()),
                Component::ParentDir | Component::CurDir => return Err(outside()),
                _ => {}
            }
        }
        // as written: before the disk is touched
        if !self.roots.iter().any(|r| under(&path, r)) {
            return Err(outside());
        }
        let real = std::fs::canonicalize(&path).map_err(|_| format!("`{p}` was not found"))?;
        // as resolved: a link must not lead out
        let inside = self.roots.iter().filter_map(|r| std::fs::canonicalize(r).ok()).any(|r| under(&real, &r));
        let private = self.private.iter().filter_map(|d| std::fs::canonicalize(d).ok()).any(|d| under(&real, &d));
        if !inside || private {
            return Err(outside());
        }
        Ok(real)
    }

    fn list(&mut self, dir: &Path) -> Vec<u8> {
        let rd = match std::fs::read_dir(dir) {
            Ok(r) => r,
            Err(e) => return err(e),
        };
        let mut entries = vec![];
        let mut truncated = false;
        for e in rd.flatten() {
            if entries.len() == LIST_CAP {
                truncated = true;
                break;
            }
            // the entry itself, never what a link points at
            let Ok(m) = e.metadata() else { continue };
            entries.push(serde_json::json!({ "name": e.file_name().to_string_lossy(), "dir": m.is_dir(), "size": m.len(), "modified_ms": ms(m.modified()) }));
        }
        self.ops += entries.len() / 100;
        entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        serde_json::json!({ "entries": entries, "truncated": truncated }).to_string().into_bytes()
    }

    /// Up to `max` bytes from `offset` (negative: from the end), as text.
    fn read(&mut self, p: &Path, offset: i64, max: u64) -> Vec<u8> {
        let mut f = match std::fs::File::open(p) {
            Ok(f) => f,
            Err(e) => return err(e),
        };
        let size = match f.metadata() {
            Ok(m) if m.is_file() => m.len(),
            Ok(_) => return err("it is a folder; list it instead"),
            Err(e) => return err(e),
        };
        let start = if offset < 0 { size.saturating_sub(offset.unsigned_abs()) } else { (offset as u64).min(size) };
        let n = max.min(size - start);
        let mut buf = Vec::with_capacity(n as usize);
        if let Err(e) = f.seek(SeekFrom::Start(start)).and_then(|_| f.take(n).read_to_end(&mut buf)) {
            return err(e);
        }
        self.bytes += buf.len() as u64;
        let truncated = start + (buf.len() as u64) < size;
        serde_json::json!({ "text": String::from_utf8_lossy(&buf), "offset": start, "size": size, "truncated": truncated }).to_string().into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value as J, json};

    fn home(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wf-fs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(".claude").join("projects")).unwrap();
        std::fs::create_dir_all(d.join("secret")).unwrap();
        std::fs::write(d.join(".claude").join("projects").join("a.jsonl"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(d.join("secret").join("key.txt"), "hunter2").unwrap();
        d
    }

    fn ask(fs: &mut Fs, req: J) -> J {
        serde_json::from_slice(&fs.handle(req.to_string().as_bytes())).unwrap()
    }

    fn claude_only(h: &Path) -> Fs {
        Fs::new(&Places { home: Some(h.to_path_buf()), private: vec![] }, &[FsRoot::parse("~/.claude").unwrap()], &[])
    }

    #[test]
    fn roots_are_folders_under_home() {
        assert_eq!(FsRoot::parse(" ~\\.claude\\ ").unwrap().to_string(), "~/.claude");
        assert_eq!(FsRoot::parse("~/.config//app").unwrap().under(Path::new("C:\\Users\\a")), Path::new("C:\\Users\\a").join(".config").join("app"));
        for (bad, why) in [("~", "~/"), ("~/", "whole home"), ("C:\\Users", "~/"), ("/etc", "~/"), ("~/../x", "plain"), ("~/a/.. ", "plain"), ("~/a:b", "plain"), ("~/*", "plain")] {
            let e = FsRoot::parse(bad).unwrap_err();
            assert!(e.contains(why), "{bad}: {e}");
        }
    }

    #[test]
    fn lists_stats_and_reads_inside_a_root() {
        let h = home("read");
        let mut fs = claude_only(&h);
        let l = ask(&mut fs, json!({ "op": "list", "path": "~/.claude/projects" }));
        assert_eq!((l["entries"][0]["name"].as_str(), l["entries"][0]["size"].as_u64(), &l["truncated"]), (Some("a.jsonl"), Some(14), &json!(false)));
        let s = ask(&mut fs, json!({ "op": "stat", "path": "~/.claude/projects" }));
        assert_eq!(s["dir"], true);
        let r = ask(&mut fs, json!({ "op": "read", "path": "~/.claude/projects/a.jsonl" }));
        assert_eq!((r["text"].as_str(), r["truncated"].as_bool()), (Some("one\ntwo\nthree\n"), Some(false)));
        let tail = ask(&mut fs, json!({ "op": "read", "path": "~/.claude/projects/a.jsonl", "offset": -6, "max": 4 }));
        assert_eq!((tail["text"].as_str(), tail["offset"].as_u64(), tail["truncated"].as_bool()), (Some("thre"), Some(8), Some(true)));
        let full = h.join(".claude").join("projects").join("a.jsonl");
        assert_eq!(ask(&mut fs, json!({ "op": "read", "path": full.to_string_lossy() }))["text"], "one\ntwo\nthree\n", "a full path works too");
        std::fs::remove_dir_all(&h).ok();
    }

    #[test]
    fn nothing_outside_the_roots_is_touched_or_told_apart() {
        let h = home("outside");
        let mut fs = claude_only(&h);
        for p in ["~/secret/key.txt", "~/secret/missing.txt", "~/.claude/../secret/key.txt", "~/.claude/.. /secret/key.txt", "~/.claude/projects/a.jsonl:x", "relative.txt", "~/.claudette/x"] {
            let e = ask(&mut fs, json!({ "op": "read", "path": p }))["error"].as_str().unwrap_or("").to_string();
            assert!(e.contains("outside") || e.contains("full path"), "{p}: {e}");
        }
        let full = h.join("secret").join("key.txt");
        assert!(ask(&mut fs, json!({ "op": "read", "path": full.to_string_lossy() }))["error"].as_str().unwrap().contains("outside"));
        std::fs::remove_dir_all(&h).ok();
    }

    #[test]
    fn a_picked_folder_is_readable_and_private_folders_never_are() {
        let h = home("picked");
        let pics = h.join("secret");
        let places = Places { home: Some(h.clone()), private: vec![h.join(".claude").join("projects")] };
        let mut fs = Fs::new(&places, &[FsRoot::parse("~/.claude").unwrap()], &[pics.clone()]);
        assert_eq!(ask(&mut fs, json!({ "op": "read", "path": pics.join("key.txt").to_string_lossy() }))["text"], "hunter2");
        assert!(ask(&mut fs, json!({ "op": "list", "path": "~/.claude/projects" }))["error"].as_str().unwrap().contains("outside"), "Wayfinder's own folder");
        assert!(ask(&mut fs, json!({ "op": "list", "path": "~/.claude" }))["entries"].is_array());
        std::fs::remove_dir_all(&h).ok();
    }

    #[test]
    fn a_call_cannot_read_without_end() {
        let h = home("budget");
        let mut fs = claude_only(&h);
        let big = h.join(".claude").join("big.txt");
        std::fs::write(&big, vec![b'x'; (READ_CAP + 10) as usize]).unwrap();
        let r = ask(&mut fs, json!({ "op": "read", "path": "~/.claude/big.txt", "max": READ_CAP * 4 }));
        assert_eq!((r["text"].as_str().unwrap().len() as u64, r["truncated"].as_bool()), (READ_CAP, Some(true)), "one read is capped");
        let spent = (0..40).map(|_| ask(&mut fs, json!({ "op": "read", "path": "~/.claude/big.txt" }))).filter(|r| r["error"].as_str().is_some_and(|e| e.contains("too much"))).count();
        assert!(spent > 0, "16 MB per call");
        std::fs::remove_dir_all(&h).ok();
    }
}
