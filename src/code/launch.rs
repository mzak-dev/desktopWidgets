//! What a Widget showing Code Source values may `launch` (ADR-0008). Text from code can come
//! from a server, so by default only `https://` links open. A Plugin may add URL schemes
//! (`vscode`) and folders under home (`~/.claude`) in `[code] launch`, shown when installing.

use std::path::{Component, Path};

use super::fs::{FsRoot, bad_part, under};

/// Schemes no plugin may add: they run code, reach the file system or search it.
const NEVER: &[&str] = &["file", "shell", "search", "search-ms", "javascript", "vbscript", "data", "mk", "its", "hcp", "res", "mhtml", "jar", "ldap", "smb"];

/// Files in a launch folder open only if they are documents; programs, scripts and links never do.
const DOCUMENTS: &[&str] = &["txt", "md", "log", "json", "jsonl", "csv", "pdf", "png", "jpg", "jpeg", "gif", "webp", "bmp"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchRule {
    /// Links like `vscode://…`.
    Scheme(String),
    /// Folders, and documents, inside this folder.
    Folder(FsRoot),
}

impl LaunchRule {
    pub fn parse(s: &str) -> Result<LaunchRule, String> {
        let s = s.trim();
        if s.starts_with('~') {
            return FsRoot::parse(s).map(LaunchRule::Folder).map_err(|e| format!("code.launch {e}"));
        }
        let scheme = s.trim_end_matches("//").trim_end_matches(':').to_ascii_lowercase();
        let valid = scheme.starts_with(|c: char| c.is_ascii_lowercase()) && scheme.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+.-".contains(c));
        if !valid {
            return Err(format!("code.launch `{s}`: name a URL scheme (`vscode`) or a folder under ~/"));
        }
        if NEVER.contains(&scheme.as_str()) || scheme.starts_with("ms-") {
            return Err(format!("code.launch `{s}`: Wayfinder never lets plugin data open {scheme}: links"));
        }
        Ok(LaunchRule::Scheme(scheme))
    }

    fn allows(&self, target: &str, home: Option<&Path>) -> bool {
        match self {
            LaunchRule::Scheme(s) => target.get(..s.len() + 1).is_some_and(|p| p.eq_ignore_ascii_case(&format!("{s}:"))),
            LaunchRule::Folder(root) => home.is_some_and(|h| in_folder(target, &root.under(h))),
        }
    }
}

impl std::fmt::Display for LaunchRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LaunchRule::Scheme(s) => write!(f, "{s}: links"),
            LaunchRule::Folder(r) => write!(f, "folders and documents in {r}"),
        }
    }
}

/// A folder, or a document, inside `root`, checked as written and again with links resolved.
fn in_folder(target: &str, root: &Path) -> bool {
    let p = Path::new(target);
    if !p.is_absolute() || p.components().any(|c| matches!(c, Component::ParentDir | Component::CurDir) || matches!(c, Component::Normal(s) if bad_part(&s.to_string_lossy()))) {
        return false;
    }
    if !under(p, root) {
        return false;
    }
    let (Ok(real), Ok(real_root)) = (std::fs::canonicalize(p), std::fs::canonicalize(root)) else { return false };
    if !under(&real, &real_root) {
        return false;
    }
    let document = || real.extension().and_then(|e| e.to_str()).is_some_and(|e| DOCUMENTS.contains(&e.to_ascii_lowercase().as_str()));
    real.is_dir() || (real.is_file() && document())
}

fn https(target: &str) -> bool {
    target.get(..8).is_some_and(|s| s.eq_ignore_ascii_case("https://"))
}

/// Whether a Widget may open `target`. `sources` holds the launch rules of each Code Source it
/// reads; every one must allow it, since any of them may have produced the text. Reading no
/// Code Source, anything goes (the user wrote the Widget or trusts its Plugin's TOML).
pub fn allowed(target: &str, sources: &[&[LaunchRule]], home: Option<&Path>) -> bool {
    let t = target.trim();
    // a URL's text goes to its handler whole; whitespace or quotes could become arguments
    let clean = !t.chars().any(|c| c.is_whitespace() || c.is_control() || c == '"');
    sources.is_empty() || (clean && sources.iter().all(|rules| https(t) || rules.iter().any(|r| r.allows(t, home))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn home(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wf-launch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(".claude").join("projects").join("app")).unwrap();
        std::fs::write(d.join(".claude").join("notes.md"), "x").unwrap();
        std::fs::write(d.join(".claude").join("hook.bat"), "x").unwrap();
        d
    }

    #[test]
    fn code_values_launch_web_links_only_by_default() {
        assert!(allowed("C:\\Windows\\notepad.exe", &[], None), "no code: as before");
        assert!(allowed("https://open-meteo.com/", &[&[]], None) && allowed(" HTTPS://x.com", &[&[]], None));
        for bad in ["\\\\host\\share\\x.exe", "ms-msdt:/id", "search-ms:query=x", "http://x.com", "C:\\x.exe", "file:///C:/x", "https://x.com/ \"-arg\""] {
            assert!(!allowed(bad, &[&[]], None), "{bad}");
        }
    }

    #[test]
    fn a_plugin_may_add_schemes_but_never_dangerous_ones() {
        assert_eq!(LaunchRule::parse("vscode://").unwrap(), LaunchRule::Scheme("vscode".into()));
        assert_eq!(LaunchRule::parse("Slack:").unwrap().to_string(), "slack: links");
        for bad in ["file", "ms-msdt", "ms-settings", "search-ms", "javascript:", "shell", "1abc", "C:\\x", ""] {
            assert!(LaunchRule::parse(bad).is_err(), "{bad}");
        }
        let vscode = [LaunchRule::parse("vscode").unwrap()];
        assert!(allowed("vscode://file/C:/src/app", &[&vscode], None));
        assert!(!allowed("vscodex://file", &[&vscode], None) && !allowed("ms-msdt:/id", &[&vscode], None));
        assert!(!allowed("vscode://file/C:/src/app", &[&vscode, &[]], None), "every source a widget reads must allow it");
    }

    #[test]
    fn a_launch_folder_opens_folders_and_documents_inside_it() {
        let h = home("folder");
        let rule = [LaunchRule::parse("~/.claude").unwrap()];
        let ok = |p: PathBuf| allowed(&p.to_string_lossy(), &[&rule], Some(&h));
        assert!(ok(h.join(".claude").join("projects").join("app")), "a folder");
        assert!(ok(h.join(".claude").join("notes.md")), "a document");
        assert!(!ok(h.join(".claude").join("hook.bat")), "never a program or script");
        assert!(!ok(h.join(".claude").join("..").join(".claude").join("notes.md")));
        assert!(!ok(h.clone()), "not the folder's parent");
        assert!(!ok(h.join(".claude").join("missing")));
        assert_eq!(rule[0].to_string(), "folders and documents in ~/.claude");
        std::fs::remove_dir_all(&h).ok();
    }
}
