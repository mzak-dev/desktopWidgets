//! `AppHost`, the App's adapter for `widgets::Host`.

use super::*;

/// The engine side of `widgets::Host`: native dialogs parented to an
/// Instance's window, shortcuts through the shell, log lines kept for `App::log`.
pub(super) struct AppHost<'a> {
    dir: &'a std::path::Path,
    hwnd: Option<windows::Win32::Foundation::HWND>,
    logs: Vec<String>,
}

impl<'a> AppHost<'a> {
    pub(super) fn new(dir: &'a std::path::Path, hwnd: Option<windows::Win32::Foundation::HWND>) -> Self {
        Self { dir, hwnd, logs: Vec::new() }
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
        crate::dialog::pick_file(self.hwnd)
    }

    fn create_shortcut(&mut self, dir: &std::path::Path, target: &std::path::Path) -> bool {
        win32::create_shortcut(dir, target).is_some()
    }

    fn log(&mut self, msg: String) {
        self.logs.push(msg);
    }
}
