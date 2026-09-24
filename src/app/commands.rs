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

    pub(super) fn remove_instance(&mut self, id: &str) {
        let Some(i) = self.ws.instances.iter().position(|c| c.id == id) else { return };
        self.ws.instances.remove(i);
        self.wins.remove(i);
        self.remove_armed = None;
        self.retain_code();
        self.sync_watchers();
        self.mark_save();
        if let Some(s) = &mut self.settings {
            s.invalidate();
        }
        self.log(format!("removed {id}"));
    }

    /// Saves a param of Instance `i`, from Settings, a source or a drop.
    pub(super) fn set_param(&mut self, i: usize, name: &str, v: &Value) {
        if self.ws.instances[i].params.get(name).map(Value::from).as_ref() == Some(v) {
            return;
        }
        let was = self.card(i);
        self.ws.instances[i].set_param(name, v);
        if self.card(i).blur != was.blur {
            self.refit_window_around_card(i, was);
        }
        self.sync_watchers(); // a param may name a path a source watches
        self.sources.invalidate();
        self.wins[i].redraw = true;
        self.mark_save();
        if let Some(s) = &mut self.settings {
            s.invalidate();
        }
    }

    pub(super) fn apply(&mut self, el: &ActiveEventLoop, cmd: Cmd) {
        let find = |s: &Self, id: &str| s.ws.instances.iter().position(|c| c.id == id);
        match cmd {
            Cmd::Add(w) => self.add_instance(el, &w),
            Cmd::Remove(id) => self.remove_instance(&id),
            Cmd::Param(id, name, v) => {
                if let Some(i) = find(self, &id) {
                    self.set_param(i, &name, &v);
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
            Cmd::SizeLimit(id, on) => {
                if let Some(i) = find(self, &id) {
                    self.ws.instances[i].size_limit = on;
                    let (card, s) = (self.card(i), self.scale_of(i));
                    // switching it back on shrinks an oversized widget, keeping its top-left
                    if let (Some(max), Some(r)) = (self.max_size_phys(i), self.wins[i].window.as_ref().and_then(|w| Self::outer_rect(w))) {
                        let g = card.gutter_px(s);
                        let mut c = card.card_of_window(r, s);
                        (c.w, c.h) = (c.w.min(max.0 - 2 * g), c.h.min(max.1 - 2 * g));
                        if card.window_of_card(c, s) != r {
                            self.glide_window(i, card.window_of_card(c, s));
                            self.save_rect_to_workspace(i);
                        }
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
                        self.glide_window(i, Rect::new(p.0, p.1, (w as f64 * s) as i32, (h as f64 * s) as i32));
                    }
                    self.sync_windows(el);
                    self.mark_save();
                }
            }
            Cmd::Theme(sel) => self.restyle(|ws| ws.theme = sel),
            Cmd::Style(scope, token, value) => self.restyle(|ws| {
                let map = match &scope {
                    Scope::Global => &mut ws.style,
                    Scope::Instance(id) => match ws.instances.iter_mut().find(|c| &c.id == id) {
                        Some(c) => &mut c.style,
                        None => return,
                    },
                };
                match value {
                    Some(v) => map.insert(token, (&v).into()),
                    None => map.remove(&token),
                };
            }),
            Cmd::ThemePick(id, axis, name) => self.restyle(|ws| {
                let Some(c) = ws.instances.iter_mut().find(|c| c.id == id) else { return };
                match axis.as_str() {
                    "palette" => c.theme.palette = name,
                    "fonts" => c.theme.fonts = name,
                    "glyphs" => c.theme.glyphs = name,
                    "pack" => c.theme.icon_pack = name,
                    _ => {}
                }
            }),
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
                self.ws.set_flag(flag, on);
                self.mark_save();
            }
            Cmd::Grid(g) => {
                self.ws.grid = g;
                self.mark_save();
            }
            Cmd::Edit(on) => self.set_edit(on),
            Cmd::Reload => self.reload(el),
            Cmd::OpenFolder => {
                let d = self.opts.dir.join("widgets");
                let _ = std::fs::create_dir_all(&d);
                win32::open(&d.to_string_lossy());
            }
            Cmd::InstallPlugin(path) => {
                let file = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                // code gets the same question Explorer asks, naming the hosts it may reach
                if let Ok((m, contents)) = plugins::describe(&path) {
                    if m.code.is_some() {
                        let installed = self.plugins.iter().find(|p| p.id == m.id).and_then(|p| p.manifest.as_ref().ok());
                        if !crate::dialog::confirm("Install a Wayfinder plugin", &plugins::install_question(&m, &contents, installed)) {
                            return;
                        }
                    }
                }
                match PluginStore::new(&self.opts.dir).install(&path) {
                    Ok(m) => {
                        self.plugin_note = format!("Installed {} {}", m.name, m.version);
                        self.log(format!("installed plugin {} {} from {}", m.id, m.version, path.display()));
                        self.reload(el);
                    }
                    Err(e) => {
                        self.plugin_note = format!("Could not install {file}: {e}");
                        self.log(format!("could not install {}: {e}", path.display()));
                    }
                }
                if let Some(s) = &mut self.settings {
                    s.invalidate();
                }
            }
            Cmd::PluginEnabled(id, on) => {
                if on {
                    self.ws.disabled_plugins.remove(&id);
                } else {
                    self.ws.disabled_plugins.insert(id.clone());
                }
                self.log(format!("plugin {id} switched {}", if on { "on" } else { "off" }));
                self.mark_save();
                self.reload(el);
            }
            Cmd::RemovePlugin(id) => {
                let orphans = self.plugin_rows.iter().find(|r| r.id == id).map(|r| r.orphans(&self.ws)).unwrap_or_default();
                match PluginStore::new(&self.opts.dir).remove(&id) {
                    Err(e) => self.log(e),
                    Ok(()) => {
                        for inst in orphans {
                            self.remove_instance(&inst);
                        }
                        // its saved data goes too, and a dying worker can no longer write it
                        match self.stores.remove(&id) {
                            Some(store) => store.purge(),
                            None => {
                                let _ = std::fs::remove_file(plugins::data_file(&self.opts.dir, &id));
                            }
                        }
                        self.ws.disabled_plugins.remove(&id);
                        self.log(format!("removed plugin {id}"));
                        self.mark_save();
                        self.reload(el);
                    }
                }
            }
            Cmd::OpenPluginsFolder => {
                let store = PluginStore::new(&self.opts.dir);
                let _ = std::fs::create_dir_all(store.dir());
                win32::open(&store.dir().to_string_lossy());
            }
            Cmd::Quit => el.exit(),
            Cmd::Close => self.settings = None,
            Cmd::Minimize => {} // handled by the settings window itself
        }
    }
}
