use super::*;
use crate::native::{Native, Pick};

/// Log lines wait here because `App::log` needs `&mut App`, which the caller holds borrowed.
pub(super) struct AppHost<'a> {
    dir: &'a std::path::Path,
    native: &'a mut dyn Native,
    logs: Vec<String>,
}

impl<'a> AppHost<'a> {
    pub(super) fn new(dir: &'a std::path::Path, native: &'a mut dyn Native) -> Self {
        Self { dir, native, logs: Vec::new() }
    }

    pub(super) fn into_logs(self) -> Vec<String> {
        self.logs
    }
}

impl Host for AppHost<'_> {
    fn data_dir(&self) -> &std::path::Path {
        self.dir
    }

    fn pick_file(&mut self) -> Option<PathBuf> {
        self.native.pick(Pick::File)
    }

    fn pick_folder(&mut self) -> Option<PathBuf> {
        self.native.pick(Pick::Folder)
    }

    fn create_shortcut(&mut self, dir: &std::path::Path, target: &std::path::Path) -> bool {
        win32::create_shortcut(dir, target).is_some()
    }

    fn log(&mut self, msg: String) {
        self.logs.push(msg);
    }
}
