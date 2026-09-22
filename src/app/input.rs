//! Mouse and wheel input on an Instance: hover, click actions (the Widget
//! first, then the engine verbs), header drag and scrolling.

use super::*;

/// Height (logical px) of the strip at the top of a card that moves it when `header_drag` is on.
pub(super) const HEADER_H: f32 = 32.0;

impl App {
    pub(super) fn on_cursor(&mut self, i: usize, pos: PhysicalPosition<f64>) {
        let scale = self.wins[i].window.as_ref().map_or(1.0, |w| w.scale_factor());
        let (x, y) = ((pos.x / scale) as f32, (pos.y / scale) as f32);
        self.wins[i].mouse = (x, y);
        if !self.edit && self.wins[i].drag.is_some() {
            self.drag_update(i, win32::cursor_pos());
            return;
        }
        if self.edit {
            if self.wins[i].drag.is_some() {
                self.drag_update(i, win32::cursor_pos());
                return;
            }
            let card = self.card_rect(i);
            let h = edit::hit_handle(x, y, card, 16.0);
            if self.wins[i].grab != Some(h) {
                self.wins[i].grab = Some(h);
                if let Some(w) = &self.wins[i].window {
                    w.set_cursor(h.cursor());
                }
            }
            return;
        }
        let hit = self.wins[i].frame.as_ref().and_then(|f| f.hit_at(x, y)).map(|h| (h.key.clone(), h.action.is_some()));
        let (key, clickable) = match hit {
            Some((k, c)) => (Some(k), c),
            None => (None, false),
        };
        if self.wins[i].hover != key {
            self.wins[i].hover = key;
            self.wins[i].redraw = true;
            if let Some(w) = &self.wins[i].window {
                w.set_cursor(if clickable { CursorIcon::Pointer } else { CursorIcon::Default });
            }
        }
    }

    pub(super) fn card_rect(&self, i: usize) -> [f32; 4] {
        let Some(w) = &self.wins[i].window else { return [0.0; 4] };
        let s = w.scale_factor() as f32;
        let size = w.inner_size();
        self.card(i).rect_in((size.width as f32 / s, size.height as f32 / s))
    }

    pub(super) fn on_mouse(&mut self, i: usize, state: ElementState, button: MouseButton) {
        if button != MouseButton::Left {
            return;
        }
        if self.edit {
            match state {
                ElementState::Pressed => {
                    let (x, y) = self.wins[i].mouse;
                    let handle = edit::hit_handle(x, y, self.card_rect(i), 16.0);
                    self.begin_drag(i, handle, win32::cursor_pos());
                }
                ElementState::Released => self.end_drag(i),
            }
            return;
        }
        if state != ElementState::Pressed {
            return self.end_drag(i);
        }
        let (x, y) = self.wins[i].mouse;
        let action = self.wins[i].frame.as_ref().and_then(|f| f.hit_at(x, y)).and_then(|h| h.action.clone());
        if let Some(a) = action {
            self.run_action(i, &a);
        } else if self.ws.header_drag {
            let [cx, cy, cw, _] = self.card_rect(i);
            if x >= cx && x < cx + cw && y >= cy && y < cy + HEADER_H {
                self.begin_drag(i, Handle::Move, win32::cursor_pos());
            }
        }
    }

    pub(super) fn on_wheel(&mut self, i: usize, delta: MouseScrollDelta) {
        let scale = self.wins[i].window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
        let dy = match delta {
            MouseScrollDelta::LineDelta(_, y) => y * 48.0,
            MouseScrollDelta::PixelDelta(p) => p.y as f32 / scale,
        };
        let (mx, my) = self.wins[i].mouse;
        let Some(frame) = &self.wins[i].frame else { return };
        let region = frame.scrolls.iter().find(|s| {
            frame.rect_of(&s.key).is_some_and(|[x, y, w, h]| mx >= x && mx < x + w && my >= y && my < y + h)
        });
        let Some(region) = region else { return };
        let max = (region.content_h - region.view_h).max(0.0);
        let cur = self.wins[i].state.get("scroll").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        let next = (cur - dy).clamp(0.0, max);
        if (next - cur).abs() > 0.01 {
            self.wins[i].state.insert("scroll".into(), Value::Num(next as f64));
            self.wins[i].redraw = true;
        }
    }

    /// A click action: the Widget's own first (a Rust Widget's verbs), then the engine's.
    pub(super) fn run_action(&mut self, i: usize, action: &str) {
        let (verb, rest) = action.split_once(' ').unwrap_or((action, ""));
        if let Some(Ok(w)) = self.reg.get(&self.ws.instances[i].widget).cloned() {
            let hwnd = self.wins[i].window.as_ref().and_then(|w| win32::hwnd_of(w));
            let mut host = AppHost::new(&self.opts.dir, hwnd);
            let mut cx = ActionCx { cfg: &self.ws.instances[i], state: &mut self.wins[i].state, host: &mut host, sources: &self.sources, redraw: false };
            let handled = w.action(verb, rest, &mut cx);
            let redraw = cx.redraw;
            for l in host.into_logs() {
                self.log(l);
            }
            self.wins[i].redraw |= redraw;
            if handled {
                return;
            }
        }
        match verb {
            "launch" => {
                if !win32::open(rest.trim()) {
                    self.log(format!("could not open `{}`", rest.trim()));
                }
            }
            "toggle" => {
                let cur = self.wins[i].state.get(rest).map_or(false, |v| v.truthy());
                self.wins[i].state.insert(rest.to_string(), Value::Bool(!cur));
                self.wins[i].state.insert("scroll".into(), Value::Num(0.0));
                self.wins[i].redraw = true;
            }
            "set" => {
                if let Some((name, val)) = rest.split_once(' ') {
                    self.wins[i].state.insert(name.to_string(), Value::Str(val.to_string()));
                    self.wins[i].redraw = true;
                }
            }
            "settings" => {
                let _ = self.proxy.send_event(UserEvent::Menu("settings".into()));
            }
            other => self.log(format!("unknown action `{other}` in `{action}`")),
        }
    }
}
