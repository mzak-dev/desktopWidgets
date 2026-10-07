//! `wayfinder --install <file>`: what double-clicking a `.wfplugin` in Explorer runs.

use std::path::Path;

use crate::native::Prompt;
use crate::plugins::{self, PluginStore};

/// Asks (naming any hosts its code may reach), installs, and says how it went. With
/// another copy of Wayfinder running, its data-folder watcher picks the Plugin up;
/// otherwise this one goes on to start. Returns whether it was installed.
pub fn install_from_explorer(data: &Path, file: &Path, running: bool, prompt: &mut dyn Prompt) -> bool {
    let title = "Install a Wayfinder plugin";
    let (m, contents) = match plugins::describe(file) {
        Ok(d) => d,
        Err(e) => {
            prompt.tell(title, &format!("{} is not a plugin Wayfinder can install:\n\n{e}", file.display()), true);
            return false;
        }
    };
    let installed = PluginStore::new(data).list().into_iter().find(|p| p.id == m.id).and_then(|p| p.manifest.ok());
    let question = plugins::install_question(&m, &contents, installed.as_ref());
    if !prompt.confirm(title, &question) {
        return false;
    }
    match PluginStore::new(data).install(file) {
        Ok(m) => {
            if running {
                prompt.tell(title, &format!("{} {} is installed. Find it in Settings > Plugins.", m.name, m.version), false);
            }
            true
        }
        Err(e) => {
            prompt.tell(title, &format!("Could not install {}:\n\n{e}", m.name), true);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::Scripted;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("wf-explorer-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn plugin(root: &Path) -> std::path::PathBuf {
        let src = root.join("sunset");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join(plugins::MANIFEST), "id = 'sunset'\nname = 'Sunset'\nversion = '1.2.0'\nauthor = 'Ada'").unwrap();
        src
    }

    #[test]
    fn a_file_that_is_not_a_plugin_is_explained_not_asked_about() {
        let root = tmp("bad");
        let mut say = Scripted::new();
        assert!(!install_from_explorer(&root.join("data"), &root.join("missing.wfplugin"), true, &mut say));
        assert!(say.asked.is_empty());
        assert!(matches!(say.told.as_slice(), [(title, text, true)] if title == "Install a Wayfinder plugin" && text.contains("is not a plugin Wayfinder can install")), "{:?}", say.told);
    }

    #[test]
    fn declining_the_question_installs_nothing() {
        let root = tmp("no");
        let data = root.join("data");
        let mut say = Scripted::new().answer_next(false);
        assert!(!install_from_explorer(&data, &plugin(&root), true, &mut say));
        assert_eq!(say.asked.len(), 1);
        assert!(say.told.is_empty());
        assert!(PluginStore::new(&data).list().is_empty());
    }

    #[test]
    fn accepting_installs_and_a_running_copy_is_told() {
        let root = tmp("yes");
        let data = root.join("data");
        let mut say = Scripted::new().answer_next(true);
        assert!(install_from_explorer(&data, &plugin(&root), true, &mut say));
        assert_eq!(PluginStore::new(&data).list().len(), 1);
        assert_eq!(say.told, [("Install a Wayfinder plugin".to_string(), "Sunset 1.2.0 is installed. Find it in Settings > Plugins.".to_string(), false)]);
        // a copy that is about to start says nothing
        let root = tmp("yes2");
        let mut quiet = Scripted::new().answer_next(true);
        assert!(install_from_explorer(&root.join("data"), &plugin(&root), false, &mut quiet));
        assert!(quiet.told.is_empty());
    }
}
