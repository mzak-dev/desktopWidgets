//! Page objects: what a test says about Wayfinder's windows, in Wayfinder's words.

use raywright::Window;
use raywright::prelude::*;

/// One widget's window.
pub struct WidgetWindow {
    pub win: Window,
}

impl WidgetWindow {
    /// Whatever the widget has bound to `action` (`toggle expanded`, `media.play_pause`).
    pub fn action(&self, action: &str) -> Locator {
        self.win.get_by_action(action).first()
    }

    /// The window on screen, physical px.
    pub async fn rect(&self) -> raywright::Result<Rect> {
        Ok(self.win.info().await?.rect)
    }
}

/// The Settings window.
pub struct SettingsWindow {
    pub win: Window,
}

impl SettingsWindow {
    /// A page's tab in the title bar (`widgets`, `workspaces`, `appearance`, `plugins`, `general`, `log`).
    pub fn tab(&self, page: &str) -> Locator {
        self.win.get_by_action(&format!("nav:{page}"))
    }
}
