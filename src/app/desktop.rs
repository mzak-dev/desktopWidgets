//! Show Desktop handling (ADR-002, from Rainmeter's System.cpp).

use super::*;

impl App {
    /// Desktop widgets float under the taskbar while the desktop is shown; Bottom
    /// ones stay hidden under it; Normal and Topmost are not ours to move.
    pub(super) fn apply_show_desktop(&mut self) {
        let host = self.fake_icon_host.or_else(win32::desktop_icon_host);
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
        // leaving Show Desktop can happen without a foreground event (Rainmeter polls too)
        if self.desktop_shown && self.show_desktop_checks.is_empty() {
            self.show_desktop_checks.push(Instant::now() + Duration::from_millis(250));
        }
    }

    pub(super) fn update_z_guard(&self, i: usize) {
        if let Some(w) = &self.wins[i].window {
            let mode = ZMode::parse(&self.ws.instances[i].z).unwrap_or(ZMode::Desktop);
            win32::set_z_guard(w, matches!(mode, ZMode::Desktop | ZMode::Bottom));
        }
    }
}
