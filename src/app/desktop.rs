//! Show Desktop (ADR-002, from Rainmeter's System.cpp): keeping Desktop and
//! Bottom widgets on the desktop layer and floating Desktop ones over a shown desktop.

use super::*;

impl App {
    /// Re-place every widget for the current Show Desktop state:
    /// Desktop-mode widgets float just under the taskbar while the desktop is
    /// shown and sink back after; Bottom-mode widgets stay under the desktop
    /// (hidden by it, by definition); Normal and Topmost are not ours to move.
    pub(super) fn apply_show_desktop(&mut self) {
        let host = self.test_host.or_else(win32::desktop_icon_host);
        for i in 0..self.wins.len() {
            let Some(h) = self.wins[i].window.as_ref().and_then(|w| win32::hwnd_of(w)) else { continue };
            match (ZMode::parse(&self.ws.instances[i].z).unwrap_or(ZMode::Desktop), self.desktop_shown) {
                (ZMode::Desktop, true) => {
                    if let Some(host) = host {
                        win32::float_over_desktop(h, host);
                    }
                }
                (ZMode::Desktop | ZMode::Bottom, false) => win32::sink_to_desktop(h),
                _ => {}
            }
        }
        // an open folder sits above its sibling widgets; sinking undid that
        let siblings = self.hwnds();
        for i in 0..self.wins.len() {
            if self.wins[i].raised {
                if let Some(h) = self.wins[i].window.as_ref().and_then(|w| win32::hwnd_of(w)) {
                    win32::raise_above(h, &siblings);
                }
            }
        }
    }

    pub(super) fn check_show_desktop(&mut self) {
        let Some(shown) = self.sentinel.as_ref().and_then(win32::desktop_state) else { return };
        if shown != self.desktop_shown {
            self.desktop_shown = shown;
            self.log(format!("desktop {}", if shown { "shown: Desktop-layer widgets float above it" } else { "hidden: widgets back on the desktop layer" }));
            self.apply_show_desktop();
        }
        // Leaving Show Desktop can happen without a foreground event; keep an eye
        // on it only while it lasts (Rainmeter polls at 100 ms in this state)
        if self.desktop_shown && self.shell_due.is_empty() {
            self.shell_due.push(Instant::now() + Duration::from_millis(250));
        }
    }

    /// Foreign z-order changes are vetoed for widgets that must stay put.
    pub(super) fn guard_z(&self, i: usize) {
        if let Some(w) = &self.wins[i].window {
            let mode = ZMode::parse(&self.ws.instances[i].z).unwrap_or(ZMode::Desktop);
            win32::set_z_guard(w, matches!(mode, ZMode::Desktop | ZMode::Bottom));
        }
    }
}
