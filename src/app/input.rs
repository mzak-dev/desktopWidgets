use super::*;

/// Logical px.
pub(super) const HEADER_DRAG_HEIGHT: f32 = 32.0;

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
            if edit::over_remove_button(x, y, card) {
                self.wins[i].grab = None; // so leaving the button restores the grip cursor
                if let Some(w) = &self.wins[i].window {
                    w.set_cursor(CursorIcon::Pointer);
                }
                return;
            }
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
        self.card(i).card_rect_in((size.width as f32 / s, size.height as f32 / s))
    }

    pub(super) fn on_mouse(&mut self, i: usize, state: ElementState, button: MouseButton) {
        if button != MouseButton::Left {
            return;
        }
        if self.edit {
            match state {
                ElementState::Pressed => {
                    let (x, y) = self.wins[i].mouse;
                    let id = self.ws.instances[i].id.clone();
                    let armed = self.remove_armed.take();
                    if let Some(j) = armed.as_ref().and_then(|a| self.ws.instances.iter().position(|c| &c.id == a)) {
                        self.wins[j].redraw = true;
                    }
                    if edit::over_remove_button(x, y, self.card_rect(i)) {
                        // a second click on the armed button removes; the first only asks
                        if armed.as_deref() == Some(id.as_str()) {
                            self.remove_instance(&id);
                        } else {
                            self.remove_armed = Some(id);
                            self.wins[i].redraw = true;
                        }
                        return;
                    }
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
            if x >= cx && x < cx + cw && y >= cy && y < cy + HEADER_DRAG_HEIGHT {
                self.begin_drag(i, Handle::Move, win32::cursor_pos());
            }
        }
    }

    pub(super) fn on_wheel(&mut self, i: usize, delta: MouseScrollDelta) {
        let scale = self.wins[i].window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
        let (dx, dy) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (x * 48.0, y * 48.0),
            MouseScrollDelta::PixelDelta(p) => (p.x as f32 / scale, p.y as f32 / scale),
        };
        // Shift turns the wheel sideways, as in browsers
        let (dx, dy) = if self.mods.shift_key() { (dx + dy, 0.0) } else { (dx, dy) };
        let Some(frame) = &self.wins[i].frame else { return };
        let Some((region, d)) = wheel_target(frame, self.wins[i].mouse, dx, dy) else { return };
        let key = if region.horizontal { "scroll_x" } else { "scroll" };
        let cur = self.wins[i].state.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        if let Some(next) = scrolled_offset(cur, d, region.view, region.content) {
            self.wins[i].state.insert(key.into(), Value::Num(next as f64));
            self.wins[i].redraw = true;
        }
    }

    /// A file dropped on a widget: the `on_drop` action of what is under the cursor gets its
    /// path, or else a Rust Widget's `drop` verb.
    pub(super) fn on_drop(&mut self, i: usize, path: &std::path::Path) {
        let p = path.to_string_lossy().into_owned();
        let at = self.wins[i].window.as_ref().and_then(|w| {
            let origin = w.inner_position().ok()?;
            let (cx, cy) = win32::cursor_pos();
            let s = w.scale_factor() as f32;
            Some(((cx - origin.x) as f32 / s, (cy - origin.y) as f32 / s))
        });
        let action = at.and_then(|(x, y)| self.wins[i].frame.as_ref()?.drop_at(x, y).map(String::from));
        match on_drop(action.as_deref(), &p) {
            OnDrop::SaveParam(name, v) => self.set_param(i, &name, &v),
            OnDrop::Run(a) => self.run_action(i, &a),
            OnDrop::Widget if self.widget_action(i, "drop", &p) => {}
            OnDrop::Widget => self.log(format!("{}: nothing there takes dropped files", self.ws.instances[i].id)),
        }
    }

    /// The Widget's own verbs. True when it handled `verb`.
    fn widget_action(&mut self, i: usize, verb: &str, rest: &str) -> bool {
        let Some(Ok(w)) = self.reg.get(&self.ws.instances[i].widget).cloned() else { return false };
        let hwnd = self.wins[i].window.as_ref().and_then(|w| win32::hwnd_of(w));
        let mut host = AppHost::new(&self.opts.dir, hwnd);
        let mut cx = ActionCx { cfg: &self.ws.instances[i], state: &mut self.wins[i].state, host: &mut host, sources: &self.sources, wants_redraw: false };
        let handled = w.handle_action(verb, rest, &mut cx);
        let redraw = cx.wants_redraw;
        for l in host.into_logs() {
            self.log(l);
        }
        self.wins[i].redraw |= redraw;
        handled
    }

    pub(super) fn run_action(&mut self, i: usize, action: &str) {
        let (verb, rest) = action.split_once(' ').unwrap_or((action, ""));
        if self.widget_action(i, verb, rest) {
            return;
        }
        // `weather.refresh`: a Code Source's own verb
        if let Some((source, v)) = verb.split_once('.') {
            if self.with_source_cx(i, |cx| self.sources.act(source, v, rest, cx)) {
                return;
            }
        }
        if verb == "param" {
            match click_param(rest, &self.sources.file_params()) {
                Ok((name, v)) => self.set_param(i, &name, &v),
                Err(e) => self.log(format!("{}: {e}", self.ws.instances[i].id)),
            }
            return;
        }
        match engine_action(&mut self.wins[i].state, verb, rest) {
            VerbOutcome::Launch(target) if !crate::code::launch::allowed(&target, &self.sources.launch_rules(&self.wins[i].deps), std::env::var_os("USERPROFILE").map(PathBuf::from).as_deref()) => {
                self.log(format!("refused to open `{target}`: a widget showing plugin data may only open https:// links and what its plugin lists under [code] launch"));
            }
            VerbOutcome::Redraw => self.wins[i].redraw = true,
            VerbOutcome::Launch(target) if crate::plugins::inside_plugins(&self.opts.dir, &target) => {
                self.log(format!("refused to open `{target}`: plugins never start programs"));
            }
            VerbOutcome::Launch(target) => {
                if !win32::open(&target) {
                    self.log(format!("could not open `{target}`"));
                }
            }
            VerbOutcome::OpenSettings => {
                let _ = self.proxy.send_event(UserEvent::Menu("settings".into()));
            }
            VerbOutcome::Nothing => {}
            VerbOutcome::Unknown => self.log(format!("unknown action `{verb}` in `{action}`")),
        }
    }
}
