use super::*;
use super::tray::set_autostart;

impl App {
    pub(super) fn default_size(&self, widget: &str, card: Card) -> (f32, f32) {
        card.window_size(match self.reg.get(widget) {
            Some(Ok(d)) => d.meta().default_card_size,
            _ => (200.0, 120.0),
        })
    }

    pub(super) fn add_instance(&mut self, el: &ActiveEventLoop, widget: &str) {
        let (w, h) = self.default_size(widget, self.new_card());
        let mon = self.monitors.first().cloned();
        let n = self.ws.instances.len() as f32;
        let mut cfg = InstanceCfg {
            id: self.ws.next_id(widget),
            widget: widget.to_string(),
            monitor: mon.map(|m| m.reference()).unwrap_or_default(),
            x: 80.0 + 28.0 * (n % 8.0),
            y: 80.0 + 28.0 * (n % 8.0),
            w,
            h,
            ..Default::default()
        };
        if let Some(Ok(w)) = self.reg.get(widget).cloned() {
            let mut host = AppHost::new(&self.opts.dir, None);
            widgets::set_up_instance(&*w, &mut cfg, &mut host);
            for l in host.into_logs() {
                self.log(l);
            }
        }
        self.log(format!("added {}", cfg.id));
        self.ws.instances.push(cfg);
        self.sync_windows(el);
        self.sync_watchers();
        self.mark_save();
    }

    pub(super) fn apply(&mut self, el: &ActiveEventLoop, cmd: Cmd) {
        let find = |s: &Self, id: &str| s.ws.instances.iter().position(|c| c.id == id);
        match cmd {
            Cmd::Add(w) => self.add_instance(el, &w),
            Cmd::Remove(id) => {
                if let Some(i) = find(self, &id) {
                    self.ws.instances.remove(i);
                    self.wins.remove(i);
                    self.sync_watchers();
                    self.mark_save();
                    self.log(format!("removed {id}"));
                }
            }
            Cmd::Param(id, name, v) => {
                if let Some(i) = find(self, &id) {
                    let was = self.card(i);
                    self.ws.instances[i].set_param(&name, &v);
                    if self.card(i).blur != was.blur {
                        self.refit_window_around_card(i, was);
                    }
                    self.sync_watchers(); // a param may name a path a source watches
                    self.sources.invalidate();
                    self.wins[i].redraw = true;
                    self.mark_save();
                }
            }
            Cmd::Items(id, items) => {
                if let Some(i) = find(self, &id) {
                    self.ws.instances[i].set_items(&items);
                    self.wins[i].redraw = true;
                    self.mark_save();
                }
            }
            Cmd::Z(id, z) => {
                if let Some(i) = find(self, &id) {
                    self.ws.instances[i].z = z.clone();
                    if let Some(h) = self.wins[i].window.as_ref().and_then(|w| win32::hwnd_of(w)) {
                        win32::set_zmode(h, ZMode::parse(&z).unwrap_or(ZMode::Desktop));
                    }
                    self.wins[i].raised = false;
                    self.update_z_guard(i);
                    if self.desktop_shown {
                        self.apply_show_desktop();
                    }
                    self.mark_save();
                }
            }
            Cmd::ClickThrough(id, on) => {
                if let Some(i) = find(self, &id) {
                    self.ws.instances[i].click_through = on;
                    if let Some(w) = &self.wins[i].window {
                        let _ = w.set_cursor_hittest(!on || self.edit);
                        win32::set_no_activate(w, !self.edit);
                    }
                    self.mark_save();
                }
            }
            Cmd::ResetPos(id) => {
                if let Some(i) = find(self, &id) {
                    let (w, h) = self.default_size(&self.ws.instances[i].widget, self.card(i));
                    let cfg = &mut self.ws.instances[i];
                    (cfg.x, cfg.y, cfg.w, cfg.h) = (60.0, 60.0, w, h);
                    if let Some(m) = self.monitors.first() {
                        cfg.monitor = m.reference();
                    }
                    if let Some(p) = workspace::resolve(&self.ws.instances[i], &self.monitors) {
                        let s = self.scale_of(i);
                        self.set_window_rect(i, Rect::new(p.0, p.1, (w as f64 * s) as i32, (h as f64 * s) as i32));
                    }
                    self.sync_windows(el);
                    self.mark_save();
                }
            }
            Cmd::Theme(sel) => {
                self.ws.theme = sel;
                self.rebuild_theme();
                self.mark_save();
            }
            Cmd::Override(k, v) => {
                match v {
                    Some(v) => self.ws.overrides.insert(k, v),
                    None => self.ws.overrides.remove(&k),
                };
                self.rebuild_theme();
                self.mark_save();
            }
            Cmd::Gpu(g) => {
                self.ws.gpu = g;
                self.mark_save();
                self.log("GPU preference saved: restart Wayfinder to apply it");
            }
            Cmd::Autostart(on) => {
                self.ws.autostart = on;
                if let Err(e) = set_autostart(on) {
                    self.log(format!("autostart: {e}"));
                }
                self.mark_save();
            }
            Cmd::Flag(flag, on) => {
                let was: Vec<Card> = (0..self.wins.len()).map(|i| self.card(i)).collect();
                self.ws.set_flag(flag, on);
                for (i, w) in was.into_iter().enumerate() {
                    if self.card(i).blur != w.blur {
                        self.refit_window_around_card(i, w);
                    }
                    self.wins[i].redraw = true;
                }
                self.mark_save();
            }
            Cmd::Grid(g) => {
                self.ws.grid = g;
                self.mark_save();
            }
            Cmd::Edit(on) => self.set_edit(on),
            Cmd::Reload => self.reload(),
            Cmd::OpenFolder => {
                let d = self.opts.dir.join("widgets");
                let _ = std::fs::create_dir_all(&d);
                win32::open(&d.to_string_lossy());
            }
            Cmd::Quit => el.exit(),
            Cmd::Close => self.settings = None,
            Cmd::Minimize => {} // handled by the settings window itself
        }
    }
}
