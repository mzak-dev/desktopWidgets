//! Edit Mode (decision 20): moving, resizing, snapping, nudging and undo of
//! Instance windows. The pure rectangle maths is `crate::edit`.

use super::*;

pub(super) struct UndoEntry {
    pub(super) id: String,
    pub(super) before: Rect,
}

impl App {
    pub(super) fn set_edit(&mut self, on: bool) {
        if self.edit == on {
            return;
        }
        self.edit = on;
        self.undo.clear();
        for i in 0..self.wins.len() {
            let click_through = self.ws.instances[i].click_through;
            if let Some(w) = &self.wins[i].window {
                let _ = w.set_cursor_hittest(on || !click_through);
                win32::set_no_activate(w, !on);
                if on {
                    w.focus_window();
                }
            }
            self.wins[i].drag = None;
            self.wins[i].redraw = true;
        }
        if !on {
            self.mark_save();
        }
        self.log(if on { "edit mode on (drag to move, drag edges/corners to resize, Ctrl+Z undo, Esc or the hotkey to finish)" } else { "edit mode off" });
    }

    pub(super) fn snap_for(&self, i: usize) -> Snap {
        let (mut xs, mut ys) = (Vec::new(), Vec::new());
        let scale = self.scale_of(i);
        let mut origin = (0, 0);
        if let Some(m) = self.monitor_of(&self.ws.instances[i]) {
            xs.extend([m.work.0, m.work.0 + m.work.2 as i32]);
            ys.extend([m.work.1, m.work.1 + m.work.3 as i32]);
            origin = (m.work.0, m.work.1);
        }
        for (j, other) in self.wins.iter().enumerate() {
            if j == i {
                continue;
            }
            // snap card edges (window minus gutter), not the transparent margin
            if let Some(c) = other.window.as_ref().and_then(|w| Self::outer_rect(w)).map(|r| self.card(j).card_of_window(r, scale)) {
                xs.extend([c.x, c.right()]);
                ys.extend([c.y, c.bottom()]);
            }
        }
        Snap { xs, ys, threshold: (8.0 * scale) as i32, grid: (self.ws.grid * scale as f32) as i32, origin }
    }

    pub(super) fn outer_rect(w: &Window) -> Option<Rect> {
        let p = w.outer_position().ok()?;
        let s = w.outer_size();
        Some(Rect::new(p.x, p.y, s.width as i32, s.height as i32))
    }

    pub(super) fn set_window_rect(&mut self, i: usize, r: Rect) {
        if let Some(w) = &self.wins[i].window {
            if let Some(h) = win32::hwnd_of(w) {
                win32::set_rect(h, r.x, r.y, r.w.max(1), r.h.max(1));
            }
        }
        self.wins[i].redraw = true;
    }

    /// Store where a window really is (anchored to its monitor) into the Workspace.
    pub(super) fn commit_rect(&mut self, i: usize) {
        let Some(w) = self.wins[i].window.clone() else { return };
        let Some(r) = Self::outer_rect(&w) else { return };
        if let Some((m, x, y)) = workspace::anchor((r.x, r.y), (r.w as u32, r.h as u32), &self.monitors) {
            let scale = self.monitors.iter().find(|mi| mi.name == m.name).map_or(1.0, |mi| mi.scale);
            let cfg = &mut self.ws.instances[i];
            cfg.monitor = m;
            cfg.x = x;
            cfg.y = y;
            cfg.w = (r.w as f64 / scale).round() as f32;
            cfg.h = (r.h as f64 / scale).round() as f32;
            self.wins[i].want = None;
            self.mark_save();
        }
    }

    /// The window grows or shrinks by the shadow gutter when blur turns off or
    /// on (`was` is the card before the change), so the visible card stays
    /// exactly where it was.
    pub(super) fn regutter(&mut self, i: usize, was: Card) {
        let Some(r) = self.wins[i].window.as_ref().and_then(|w| Self::outer_rect(w)) else { return };
        let s = self.scale_of(i);
        let now = self.card(i).window_of_card(was.card_of_window(r, s), s);
        self.set_window_rect(i, now);
        self.commit_rect(i);
    }

    pub(super) fn min_size_phys(&self, i: usize) -> (i32, i32) {
        let cfg = &self.ws.instances[i];
        let min = match self.reg.get(&cfg.widget) {
            Some(Ok(d)) => d.meta().min_size,
            _ => (48.0, 48.0),
        };
        self.card(i).min_window_px(min, self.scale_of(i))
    }

    pub(super) fn begin_drag(&mut self, i: usize, handle: Handle, cursor0: (i32, i32)) {
        if let Some(r) = self.wins[i].window.as_ref().and_then(|w| Self::outer_rect(w)) {
            self.wins[i].tween = None;
            self.wins[i].drag = Some(Drag { handle, cursor0, rect0: r, before: r });
            self.wins[i].redraw = true;
        }
    }

    /// Live move/resize: apply the drag for the current (screen, physical) cursor.
    pub(super) fn drag_update(&mut self, i: usize, cursor: (i32, i32)) {
        let Some(d) = &self.wins[i].drag else { return };
        let (handle, cursor0, rect0) = (d.handle, d.cursor0, d.rect0);
        let snap = self.snap_for(i);
        let min = self.min_size_phys(i);
        // shift+drag disables snapping
        let snap = if self.mods.shift_key() { Snap { threshold: 0, grid: 0, ..snap } } else { snap };
        // snap the visible card, not the transparent shadow margin around it
        let (card, s) = (self.card(i), self.scale_of(i));
        let g = card.gutter_px(s);
        let c = edit::apply(handle, card.card_of_window(rect0, s), cursor.0 - cursor0.0, cursor.1 - cursor0.1, (min.0 - 2 * g, min.1 - 2 * g), &snap);
        self.set_window_rect(i, card.window_of_card(c, s));
        self.wins[i].tween = None;
    }

    pub(super) fn end_drag(&mut self, i: usize) {
        if let Some(d) = self.wins[i].drag.take() {
            let changed = self.wins[i].window.as_ref().and_then(|w| Self::outer_rect(w)).is_some_and(|r| r != d.before);
            if changed {
                self.undo.push(UndoEntry { id: self.ws.instances[i].id.clone(), before: d.before });
                self.commit_rect(i);
            }
            self.wins[i].redraw = true;
        }
    }

    pub(super) fn on_key(&mut self, i: usize, key: &Key, state: ElementState) {
        if !self.edit || state != ElementState::Pressed {
            return;
        }
        let ctrl = self.mods.control_key();
        match key {
            Key::Named(NamedKey::Escape) => self.set_edit(false),
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("z") => self.undo_last(),
            Key::Named(n @ (NamedKey::ArrowLeft | NamedKey::ArrowRight | NamedKey::ArrowUp | NamedKey::ArrowDown)) => {
                let step = if self.mods.shift_key() { 10 } else { 1 };
                let (dx, dy) = match n {
                    NamedKey::ArrowLeft => (-step, 0),
                    NamedKey::ArrowRight => (step, 0),
                    NamedKey::ArrowUp => (0, -step),
                    _ => (0, step),
                };
                if let Some(r) = self.wins[i].window.as_ref().and_then(|w| Self::outer_rect(w)) {
                    self.undo.push(UndoEntry { id: self.ws.instances[i].id.clone(), before: r });
                    self.set_window_rect(i, Rect { x: r.x + dx, y: r.y + dy, ..r });
                    self.commit_rect(i);
                }
            }
            _ => {}
        }
    }

    pub(super) fn undo_last(&mut self) {
        let Some(u) = self.undo.pop() else { return };
        if let Some(i) = self.ws.instances.iter().position(|c| c.id == u.id) {
            self.set_window_rect(i, u.before);
            self.commit_rect(i);
        }
    }
}
