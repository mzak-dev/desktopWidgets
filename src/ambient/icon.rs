//! Where an app's own icon comes from, as the engine sees it. The Windows adapter asks the
//! shell; the fixed one draws a tile per file stem. The rest of the chain (an explicit path,
//! an Icon Pack, the generic tile) is `icons`' and needs no machine.

use std::path::{Path, PathBuf};

use crate::images::Decoded;

pub trait IconSource: Send + Sync {
    /// The icon of the file, shortcut or folder at `path`, 48 px where the source can.
    fn shell_icon(&self, path: &Path) -> Option<Decoded>;
    /// The file a target names: the path itself if it exists, or an app name found on `PATH`.
    /// `None` for a URL or for something not found.
    fn resolve_path(&self, target: &str) -> Option<PathBuf>;
}
