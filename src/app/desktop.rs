//! Show Desktop handling (ADR-002, from Rainmeter's System.cpp), and widgets behind the
//! desktop icons (ADR-0013).

use super::*;

impl App {
    /// Desktop widgets float under the taskbar while the desktop is shown; Bottom
    /// ones stay hidden under it; Normal and Topmost are not ours to move. Those behind
    /// the icons are part of the desktop and stay where they are.
    pub(super) fn apply_show_desktop(&mut self) {
        let host = self.fake_icon_host.or_else(win32::desktop_icon_host);
        for i in 0..self.wins.len() {
            if self.wins[i].behind {
                continue;
            }
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
        let siblings = self.front_hwnds();
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

    /// The widget windows that share the top-level z-order; those behind the icons have their own.
    pub(super) fn front_hwnds(&self) -> Vec<windows::Win32::Foundation::HWND> {
        self.wins.iter().filter(|w| !w.behind).filter_map(|w| w.window.as_ref()).filter_map(|w| win32::hwnd_of(w)).collect()
    }

    /// Clicks reach a widget unless it is click-through or behind the icons; in Edit Mode, always.
    pub(super) fn update_hittest(&self, i: usize) {
        let Some(w) = &self.wins[i].window else { return };
        let through = self.ws.instances[i].click_through || self.wins[i].behind;
        let _ = w.set_cursor_hittest(self.edit || !through);
        // winit rebuilds WS_EX_* from its own flags on every change: ours go back on after it
        win32::set_no_activate(w, !self.edit);
    }

    /// Whether Instance `i` belongs behind the icons now: its Style says so, its Layer is one
    /// of the desktop's, and Edit Mode, which needs the mouse, is off.
    fn wants_behind(&self, i: usize) -> bool {
        let desktop_layer = matches!(ZMode::parse(&self.ws.instances[i].z).unwrap_or(ZMode::Desktop), ZMode::Desktop | ZMode::Bottom);
        !self.edit && desktop_layer && self.theme_of(i).flag("behind-icons")
    }

    /// Moves every widget into the icon layer or out of it as `wants_behind` says; those
    /// already where they belong are left alone, so it is safe to call after any change.
    pub(super) fn place_layers(&mut self) {
        let mut layer: Option<Option<win32::IconLayer>> = None;
        let (mut moved, mut sunk) = (false, Vec::new());
        for i in 0..self.wins.len() {
            let want = self.wants_behind(i);
            if want == self.wins[i].behind || self.wins[i].window.is_none() {
                continue;
            }
            if !want {
                moved |= self.move_layer(i, None);
                continue;
            }
            let id = self.ws.instances[i].id.clone();
            match *layer.get_or_insert_with(win32::icon_layer) {
                Some(l) if self.move_layer(i, Some(l)) => sunk.push(id),
                Some(_) => {}
                None => self.log(format!("{id} stays in front of the icons: the desktop's icon layer was not found")),
            }
        }
        if !sunk.is_empty() {
            self.log(format!("behind the desktop icons: {}", sunk.join(", ")));
        }
        if (moved || !sunk.is_empty()) && self.desktop_shown {
            self.apply_show_desktop();
        }
    }

    /// Puts window `i` into `layer`, behind the icons, or with `None` back in front as a
    /// top-level window on its Layer. False if it stayed where it was.
    fn move_layer(&mut self, i: usize, layer: Option<win32::IconLayer>) -> bool {
        let Some(h) = self.wins[i].window.as_ref().and_then(|w| win32::hwnd_of(w)) else { return false };
        let id = self.ws.instances[i].id.clone();
        match &layer {
            Some(l) => {
                if !win32::sink_behind_icons(h, l) {
                    self.log(format!("Windows did not let {id} go behind the desktop icons"));
                    return false;
                }
            }
            None => {
                win32::lift_from_icons(h);
                win32::set_zmode(h, ZMode::parse(&self.ws.instances[i].z).unwrap_or(ZMode::Desktop));
            }
        }
        self.wins[i].behind = layer.is_some();
        self.wins[i].raised = false;
        self.update_hittest(i);
        self.wins[i].redraw = true;
        if let Err(e) = self.rebind_target(i) {
            if layer.is_some() {
                self.log(format!("{id} cannot draw behind the desktop icons ({e}): back in front of them"));
                self.move_layer(i, None);
            } else {
                self.log(format!("{id} could not draw again after leaving the icon layer: {e}"));
            }
            return false;
        }
        true
    }

    /// A new swapchain for window `i` after it changed parent, as after a GPU loss: its
    /// DirectComposition target was made for the window where it was.
    fn rebind_target(&mut self, i: usize) -> Result<(), String> {
        let (Some(win), Some(g)) = (self.wins[i].window.clone(), self.gpu.as_mut()) else { return Ok(()) };
        self.wins[i].target = None; // one DirectComposition target per window: the old one goes first
        self.wins[i].target = Some(g.target_for(&win)?);
        Ok(())
    }

    /// Explorer started again: a widget still in an icon layer that is gone comes out, and
    /// every widget that belongs behind the icons goes into the new one.
    pub(super) fn relayer(&mut self) {
        let parent = win32::icon_layer().map(|l| l.parent);
        for i in 0..self.wins.len() {
            let Some(h) = self.wins[i].window.as_ref().and_then(|w| win32::hwnd_of(w)) else { continue };
            if self.wins[i].behind && (parent.is_none() || win32::parent_of(h) != parent) {
                self.move_layer(i, None);
            }
        }
        self.place_layers();
    }
}
