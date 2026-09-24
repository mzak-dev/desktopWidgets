//! `wayfinder --install <file>`: what double-clicking a `.wfplugin` in Explorer runs.

use std::path::Path;

use crate::dialog;
use crate::plugins::{self, PluginStore};

/// Asks (naming any hosts its code may reach), installs, and says how it went. With
/// another copy of Wayfinder running, its data-folder watcher picks the Plugin up;
/// otherwise this one goes on to start. Returns whether it was installed.
pub fn install_from_explorer(data: &Path, file: &Path, running: bool) -> bool {
    let title = "Install a Wayfinder plugin";
    let (m, contents) = match plugins::describe(file) {
        Ok(d) => d,
        Err(e) => {
            dialog::tell(title, &format!("{} is not a plugin Wayfinder can install:\n\n{e}", file.display()), true);
            return false;
        }
    };
    let installed = PluginStore::new(data).list().into_iter().find(|p| p.id == m.id).and_then(|p| p.manifest.ok());
    let question = plugins::install_question(&m, &contents, installed.as_ref());
    if !dialog::confirm(title, &question) {
        return false;
    }
    match PluginStore::new(data).install(file) {
        Ok(m) => {
            if running {
                dialog::tell(title, &format!("{} {} is installed. Find it in Settings > Plugins.", m.name, m.version), false);
            }
            true
        }
        Err(e) => {
            dialog::tell(title, &format!("Could not install {}:\n\n{e}", m.name), true);
            false
        }
    }
}
