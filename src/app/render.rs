//! Drawing an Instance: build its Widget, dress and lay it out, add the Edit
//! Mode overlay, present, schedule the next frame, and grow or shrink the
//! window for an `[expand]` (decision 23).

use super::*;

impl App {
    pub(super) fn render(&mut self, i: usize) {
        let now = Instant::now();
        let card = self.card(i);
        let App { gpu, text, icons, theme, reg, sources, ws, wins, edit, .. } = self;
        let (Some(gpu), Some(iw)) = (gpu.as_mut(), wins.get_mut(i)) else { return };
        let (Some(window), Some(target)) = (iw.window.clone(), iw.target.as_mut()) else { return };
        let cfg = &ws.instances[i];

        // advance an expand/collapse size tween before measuring the window
        if let Some(tw) = &iw.tween {
            let t = (now.saturating_duration_since(tw.start).as_secs_f32() / 0.20).clamp(0.0, 1.0);
            let k = Ease::Out.apply(t);
            let l = |a: i32, b: i32| a + ((b - a) as f32 * k).round() as i32;
            if let Some(h) = win32::hwnd_of(&window) {
                win32::set_rect(h, l(tw.from.x, tw.to.x), l(tw.from.y, tw.to.y), l(tw.from.w, tw.to.w).max(1), l(tw.from.h, tw.to.h).max(1));
            }
            if t >= 1.0 {
                iw.tween = None;
            }
        }
        let phys = window.inner_size();
        if target.view() != (phys.width.max(1), phys.height.max(1)) {
            gpu.fit(target, phys.width, phys.height);
        }
        let scale = window.scale_factor() as f32;
        let blur = card.blur;
        if blur != iw.blur {
            if let Some(h) = win32::hwnd_of(&window) {
                win32::set_blur(h, blur);
                iw.blur = blur;
            }
        }
        let size = (phys.width as f32 / scale, phys.height as f32 / scale);
        let missing: Def = Err(format!("unknown widget `{}`", cfg.widget));
        let def = reg.get(&cfg.widget).unwrap_or(&missing);
        let tm = data::now_local();
        let pack = ws.theme.icon_pack.clone();
        let v = View { cfg, state: &iw.state, size, theme, pack: &pack, tm, hover: iw.hover.as_deref(), scale, now, card };
        let mut sv = Services { gpu, icons, text, anim: &mut iw.anim, sources };
        let mut p = widgets::prepare(def, &v, &mut sv);

        if *edit {
            let (cw, ch) = card.card_size(size);
            let label = format!("{}, {}   {}x{}", cfg.x as i32, cfg.y as i32, cw as i32, ch as i32);
            let ov = edit::overlay(&cfg.id, size, card.gutter, &label, theme, iw.drag.as_ref().map(|d| d.handle));
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
        let continuous = sources.is_continuous(&p.deps);
        iw.next_tick = if continuous { None } else { sources.next_wake(&p.deps, &tm).map(|d| now + d) };
        iw.widget_error = p.error.clone();
        iw.animating = p.frame.animating || continuous || iw.tween.is_some();
        iw.deps = p.deps;
        iw.frame = Some(p.frame);
        iw.redraw = false;
        iw.requested = false;
        iw.last_render = now;
        let expand = p.expand;
        if lost.is_some() && self.gpu_lost.is_none() {
            self.gpu_lost = lost;
        }
        self.drive_expand(i, expand);
    }

    /// Grow or shrink the window when the widget's `[expand]` state changes
    /// (decision 23): grow away from the nearest screen edge, clamp to the
    /// work area, sit above sibling widgets while open.
    pub(super) fn drive_expand(&mut self, i: usize, expand: Option<ExpandInfo>) {
        let Some(window) = self.wins[i].window.clone() else { return };
        let cfg = self.ws.instances[i].clone();
        let Some(mon) = self.monitor_of(&cfg).cloned() else { return };
        let active = expand.is_some_and(|e| e.active);
        let scale = mon.scale;
        let Some(pos) = workspace::resolve(&cfg, &self.monitors) else { return };
        let collapsed = Rect::new(pos.0, pos.1, (cfg.w as f64 * scale).round() as i32, (cfg.h as f64 * scale).round() as i32);
        let target = if active {
            let e = expand.unwrap();
            let (tw, th) = (e.width.unwrap_or(cfg.w), e.height.unwrap_or(cfg.h));
            let (w, h) = (((tw as f64 * scale).round() as i32).min(mon.work.2 as i32), ((th as f64 * scale).round() as i32).min(mon.work.3 as i32));
            let (wl, wt, wr, wb) = (mon.work.0, mon.work.1, mon.work.0 + mon.work.2 as i32, mon.work.1 + mon.work.3 as i32);
            let x = if collapsed.x + w > wr { (collapsed.right() - w).max(wl) } else { collapsed.x };
            let y = if collapsed.y + h > wb { (collapsed.bottom() - h).max(wt) } else { collapsed.y };
            Rect::new(x, y, w, h)
        } else {
            collapsed
        };
        if self.wins[i].want == Some(target) {
            return;
        }
        let first = self.wins[i].want.is_none();
        let hwnd = win32::hwnd_of(&window);
        let cur = window.outer_position().ok().map(|p| Rect::new(p.x, p.y, window.outer_size().width as i32, window.outer_size().height as i32)).unwrap_or(collapsed);
        self.wins[i].want = Some(target);
        if first && !active {
            return; // initial layout at the stored size; nothing to animate
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
