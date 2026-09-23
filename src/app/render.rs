use super::*;

impl App {
    pub(super) fn render(&mut self, i: usize) {
        let now = Instant::now();
        let theme = self.theme_of(i);
        let card = Card::new(&theme);
        let App { gpu, text, icons, theme: chrome, reg, sources, ws, wins, edit, .. } = self;
        let (Some(gpu), Some(iw)) = (gpu.as_mut(), wins.get_mut(i)) else { return };
        let (Some(window), Some(target)) = (iw.window.clone(), iw.target.as_mut()) else { return };
        let cfg = &ws.instances[i];

        // before measuring the window
        if let Some(tw) = &iw.tween {
            let r = tw.rect_at(now);
            if let Some(h) = win32::hwnd_of(&window) {
                win32::set_rect(h, r.x, r.y, r.w, r.h);
            }
            if tw.finished_at(now) {
                iw.tween = None;
            }
        }
        let phys = window.inner_size();
        if target.view() != (phys.width.max(1), phys.height.max(1)) {
            gpu.fit(target, phys.width, phys.height);
        }
        let scale = window.scale_factor() as f32;
        // a roundness change under blur re-applies too: DWM clips the blur to the corners
        let blur = card.blur.then(|| card.blur_corner_pref());
        if let Some(h) = win32::hwnd_of(&window).filter(|_| blur != iw.blur_applied) {
            win32::set_blur(h, blur.is_some(), blur.unwrap_or(0));
            iw.blur_applied = blur;
        }
        let size = (phys.width as f32 / scale, phys.height as f32 / scale);
        let missing: Def = Err(format!("unknown widget `{}`", cfg.widget));
        let def = reg.get(&cfg.widget).unwrap_or(&missing);
        let tm = data::now_local();
        let pack = ws.theme.icon_pack.clone();
        iw.anim.duration_factor = anim::duration_factor(&theme.str("anim-speed"));
        let v = View { cfg, state: &iw.state, window_size: size, theme: &theme, icon_pack: &pack, tm, hover: iw.hover.as_deref(), scale, now, card };
        let mut sv = Services { gpu, icons, text, anim: &mut iw.anim, sources };
        let mut p = widgets::prepare(def, &v, &mut sv);

        if *edit {
            let (cw, ch) = card.card_size(size);
            let label = format!("{}, {}   {}x{}", cfg.x as i32, cfg.y as i32, cw as i32, ch as i32);
            let ov = edit::overlay(&cfg.id, size, card.gutter, &label, chrome, iw.drag.as_ref().map(|d| d.handle));
            let mut env = Env { text, anim: &mut iw.ov_anim, hover: None, now, scale };
            let of = ui::layout(&ov, size, &mut env);
            let [l0, _] = of.list.layers;
            p.frame.list.layers[1].shapes.extend(l0.shapes);
            p.frame.list.layers[1].images.extend(l0.images);
            p.frame.list.layers[1].texts.extend(l0.texts);
            p.frame.animating |= of.animating;
        }
        let mut lost = None;
        match gpu.render(target, &p.frame.list, text) {
            Ok(()) => iw.error = None,
            Err(RenderError::Skip(e)) => {
                if iw.error.as_deref() != Some(e.as_str()) {
                    eprintln!("wayfinder: render {}: {e}", cfg.id);
                    iw.error = Some(e);
                }
            }
            Err(RenderError::Lost(e)) => lost = Some(format!("{}: {e}", cfg.id)),
        }
        for w in &p.warnings {
            eprintln!("wayfinder: {}: {w}", cfg.id);
        }
        let continuous = sources.needs_every_frame(&p.deps);
        iw.next_tick = if continuous { None } else { sources.next_wake(&p.deps, &tm).map(|d| now + d) };
        iw.widget_error = p.error.clone();
        iw.animating = p.frame.animating || continuous || iw.tween.is_some();
        iw.deps = p.deps;
        iw.frame = Some(p.frame);
        iw.redraw = false;
        iw.requested = false;
        iw.last_render = now;
        let expand = p.expand;
        if lost.is_some() && self.gpu_lost_reason.is_none() {
            self.gpu_lost_reason = lost;
        }
        self.apply_expand(i, expand);
    }

    /// Decision 23; an open expand also sits above sibling widgets.
    pub(super) fn apply_expand(&mut self, i: usize, expand: Option<ExpandInfo>) {
        let Some(window) = self.wins[i].window.clone() else { return };
        let cfg = self.ws.instances[i].clone();
        let Some(mon) = self.monitor_of(&cfg).cloned() else { return };
        let active = expand.is_some_and(|e| e.active);
        let scale = mon.scale;
        let Some(pos) = workspace::resolve(&cfg, &self.monitors) else { return };
        let collapsed = Rect::new(pos.0, pos.1, (cfg.w as f64 * scale).round() as i32, (cfg.h as f64 * scale).round() as i32);
        let target = expand_target(expand, (cfg.w, cfg.h), collapsed, mon.work, scale);
        if self.wins[i].want == Some(target) {
            return;
        }
        let first = self.wins[i].want.is_none();
        let hwnd = win32::hwnd_of(&window);
        let cur = window.outer_position().ok().map(|p| Rect::new(p.x, p.y, window.outer_size().width as i32, window.outer_size().height as i32)).unwrap_or(collapsed);
        self.wins[i].want = Some(target);
        if first && !active {
            return; // first layout: nothing to animate
        }
        self.wins[i].tween = Some(SizeTween { from: cur, to: target, start: Instant::now() });
        // one reconfigure up front; the animation itself then only moves the window
        if let (Some(g), Some(t)) = (self.gpu.as_ref(), self.wins[i].target.as_mut()) {
            g.fit(t, target.w.max(1) as u32, target.h.max(1) as u32);
        }
        self.wins[i].redraw = true;
        let siblings = self.hwnds();
        if let Some(h) = hwnd {
            let mode = ZMode::parse(&cfg.z).unwrap_or(ZMode::Desktop);
            let grows = target.w > collapsed.w || target.h > collapsed.h;
            if active && grows && matches!(mode, ZMode::Desktop | ZMode::Bottom) && !self.wins[i].raised {
                win32::raise_above(h, &siblings);
                self.wins[i].raised = true;
            } else if !(active && grows) && self.wins[i].raised {
                win32::set_zmode(h, mode);
                self.wins[i].raised = false;
            }
        }
    }
}
