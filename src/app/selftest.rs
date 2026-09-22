//! `--selftest`: a scripted end-to-end run of the interactive paths with
//! synthetic input (never the real mouse). Results go to the log and `selftest.txt`.

use super::*;

pub(super) struct SelfTest {
    pub(super) step: u32,
    pub(super) at: Instant,
    pub(super) checks: Vec<(String, bool)>,
    pub(super) rect: Option<Rect>,
    pub(super) collapsed: Option<Rect>,
    pub(super) configures0: u32,
    pub(super) fake: Option<win32::Sentinel>,
}

impl App {
    pub(super) fn idx(&self, id: &str) -> Option<usize> {
        self.ws.instances.iter().position(|c| c.id == id)
    }

    pub(super) fn first_hit_with(&self, i: usize, action: &str) -> Option<(f32, f32)> {
        let f = self.wins[i].frame.as_ref()?;
        f.hits.iter().find(|h| h.action.as_deref() == Some(action)).map(|h| (h.rect[0] + h.rect[2] / 2.0, h.rect[1] + h.rect[3] / 2.0))
    }

    pub(super) fn click_at(&mut self, i: usize, p: (f32, f32)) {
        self.wins[i].mouse = p;
        self.on_mouse(i, ElementState::Pressed, MouseButton::Left);
    }

    pub(super) fn rect_now(&self, i: usize) -> Option<Rect> {
        self.wins[i].window.as_ref().and_then(|w| Self::outer_rect(w))
    }

    /// Scripted end-to-end check of the interactive paths, using synthetic
    /// input (never the real mouse). Results go to the log and `selftest.txt`.
    pub(super) fn selftest_tick(&mut self, el: &ActiveEventLoop, now: Instant) {
        let Some(mut st) = self.selftest.take() else { return };
        if now < st.at {
            self.selftest = Some(st);
            return;
        }
        let check = |st: &mut SelfTest, name: &str, ok: bool, detail: String| {
            eprintln!("selftest: {} {name} {detail}", if ok { "PASS" } else { "FAIL" });
            st.checks.push((format!("{name} {detail}"), ok));
        };
        let both = 0x80u32 | 0x0800_0000; // WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE
        let clock = self.idx("clock-1");
        let folder = self.idx("icon_folder-1");
        let mut next = 350u64;
        match st.step {
            0 => {
                let hw = self.hwnds();
                check(&mut st, "every instance has a window", hw.len() == self.ws.instances.len() && hw.len() >= 4, format!("({} windows)", hw.len()));
                check(&mut st, "widgets are tool + no-activate windows", hw.iter().all(|h| win32::ex_style(*h) & both == both), String::new());
                let host = win32::desktop_icon_host().and_then(win32::z_index);
                let z: Vec<usize> = hw.iter().filter_map(|h| win32::z_index(*h)).collect();
                check(&mut st, "widgets sit directly above the desktop layer", host.is_some_and(|hz| z.iter().all(|zi| *zi < hz && hz - *zi <= hw.len() + 2)), format!("(z {z:?}, desktop {host:?})"));
                self.set_edit(true);
            }
            1 => {
                let hw = self.hwnds();
                check(&mut st, "edit mode drops no-activate so keys arrive", hw.iter().all(|h| win32::ex_style(*h) & 0x0800_0000 == 0 && win32::ex_style(*h) & 0x80 != 0), String::new());
            }
            2 => {
                let Some(i) = clock else { return self.selftest_abort(el, st, "no clock-1 instance") };
                let r0 = self.rect_now(i).unwrap();
                st.rect = Some(r0);
                self.begin_drag(i, Handle::Move, (1000, 500));
                self.drag_update(i, (900, 560));
                let mid = self.rect_now(i).unwrap();
                check(&mut st, "drag moves the window live, before release", (mid.x - r0.x + 100).abs() <= 10 && (mid.y - r0.y - 60).abs() <= 10, format!("(dx {}, dy {})", mid.x - r0.x, mid.y - r0.y));
                self.end_drag(i);
                next = 900;
            }
            3 => {
                let i = clock.unwrap();
                let saved = std::fs::read_to_string(Workspace::path(&self.opts.dir)).ok().and_then(|t| serde_json::from_str::<Workspace>(&t).ok());
                let on_disk = saved.and_then(|w| w.instances.into_iter().find(|c| c.id == "clock-1")).map(|c| (c.x, c.y));
                let mem = (self.ws.instances[i].x, self.ws.instances[i].y);
                let anchored = self.ws.instances[i].monitor.name.contains("DISPLAY");
                check(&mut st, "moved position is persisted, anchored to a monitor", on_disk == Some(mem) && anchored, format!("(disk {on_disk:?}, memory {mem:?})"));
                let r0 = self.rect_now(i).unwrap();
                self.begin_drag(i, Handle::SE, (0, 0));
                self.drag_update(i, (70, 50));
                self.end_drag(i);
                let r1 = self.rect_now(i).unwrap();
                check(&mut st, "SE handle resizes live, top-left fixed", r1.x == r0.x && r1.y == r0.y && r1.w >= r0.w + 60 && r1.h >= r0.h + 40, format!("({}x{} -> {}x{})", r0.w, r0.h, r1.w, r1.h));
                self.render(i);
                let sz = self.wins[i].window.as_ref().unwrap().inner_size();
                let (tw, th) = self.wins[i].target.as_ref().unwrap().size();
                check(&mut st, "render target covers the resized window", tw >= sz.width && th >= sz.height, format!("(swapchain {tw}x{th}, window {}x{})", sz.width, sz.height));
            }
            4 => {
                self.undo_last();
                self.undo_last();
                let i = clock.unwrap();
                let r = self.rect_now(i).unwrap();
                let want = st.rect;
                check(&mut st, "two undos restore the original rect", Some(r) == want, format!("({r:?} vs {want:?})"));
                self.set_edit(false);
            }
            5 => {
                let hw = self.hwnds();
                check(&mut st, "leaving edit mode restores no-activate", hw.iter().all(|h| win32::ex_style(*h) & both == both), String::new());
                let Some(i) = folder else { return self.selftest_abort(el, st, "no icon_folder-1 instance") };
                st.collapsed = self.rect_now(i);
                st.configures0 = self.gpu.as_ref().map_or(0, |g| g.configures.get());
                match self.first_hit_with(i, "toggle expanded") {
                    Some(p) => self.click_at(i, p),
                    None => check(&mut st, "folder tile is clickable", false, "(no toggle hit region)".into()),
                }
                next = 700;
            }
            6 => {
                let i = folder.unwrap();
                let c = st.collapsed.unwrap();
                let r = self.rect_now(i).unwrap();
                check(&mut st, "clicking the folder expands its window in place", r.w > c.w + 100 && r.h > c.h + 40, format!("({}x{} -> {}x{})", c.w, c.h, r.w, r.h));
                let n = self.gpu.as_ref().map_or(0, |g| g.configures.get()) - st.configures0;
                check(&mut st, "the expand animation reconfigures the swapchain at most once", n <= 1, format!("({n} reconfigures over ~12 frames)"));
                let m = self.monitor_of(&self.ws.instances[i]).unwrap().work;
                check(&mut st, "expanded window stays inside the work area", r.x >= m.0 && r.y >= m.1 && r.right() <= m.0 + m.2 as i32 && r.bottom() <= m.1 + m.3 as i32, format!("({r:?} in {m:?})"));
                let me = win32::hwnd_of(self.wins[i].window.as_ref().unwrap()).and_then(win32::z_index);
                let others: Vec<usize> = self.wins.iter().enumerate().filter(|(j, _)| *j != i).filter_map(|(_, w)| w.window.as_ref()).filter_map(|w| win32::hwnd_of(w)).filter_map(win32::z_index).collect();
                check(&mut st, "expanded folder is above sibling widgets", me.is_some_and(|m| others.iter().all(|o| m < *o)), format!("(z {me:?} vs {others:?})"));
                match self.first_hit_with(i, "toggle expanded") {
                    Some(p) => self.click_at(i, p),
                    None => check(&mut st, "folder has a close control", false, String::new()),
                }
                next = 700;
            }
            7 => {
                let i = folder.unwrap();
                let c = st.collapsed.unwrap();
                let r = self.rect_now(i).unwrap();
                check(&mut st, "closing the folder restores its exact size and place", r == c, format!("({r:?} vs {c:?})"));
                // hot reload: a broken user override must show an in-place error, never a fallback
                let wd = self.opts.dir.join("widgets");
                let _ = std::fs::create_dir_all(&wd);
                let _ = std::fs::write(wd.join("clock.toml"), "[root]\ntype='text'\ncolour='#fff'\n");
                next = 1100;
            }
            8 => {
                let i = clock.unwrap();
                let bad = self.reg.get("clock").is_some_and(|d| d.is_err());
                let shown = self.wins[i].widget_error.clone();
                check(&mut st, "editing a widget file hot-reloads it", bad, String::new());
                check(&mut st, "a broken definition shows an error card in place", shown.as_deref().is_some_and(|e| e.contains("colour")), format!("({:?})", shown.as_deref().map(|e| e.chars().take(60).collect::<String>())));
                let _ = std::fs::remove_file(self.opts.dir.join("widgets").join("clock.toml"));
                next = 1100;
            }
            9 => {
                let i = clock.unwrap();
                check(&mut st, "fixing the file heals the widget", self.reg.get("clock").is_some_and(|d| d.is_ok()) && self.wins[i].widget_error.is_none(), String::new());
                // settings-window commands: the same path the UI takes
                self.apply(el, Cmd::Param("clock-1".into(), "ticks".into(), Value::Bool(false)));
                self.apply(el, Cmd::Add("digital_clock".into()));
                self.apply(el, Cmd::Z("clock-1".into(), "normal".into()));
                self.apply(el, Cmd::Theme(crate::theme::Selection { palette: "Daylight".into(), ..Default::default() }));
                next = 500;
            }
            10 => {
                let i = clock.unwrap();
                check(&mut st, "a Param command reaches the instance's saved config", self.ws.instances[i].params.get("ticks") == Some(&serde_json::Value::Bool(false)), String::new());
                check(&mut st, "adding a widget creates a window for it", self.idx("digital_clock-2").is_some_and(|j| self.wins[j].window.is_some()) && self.hwnds().len() == 5, format!("({} windows)", self.hwnds().len()));
                let z = self.wins[i].window.as_ref().and_then(|w| win32::hwnd_of(w)).map(|h| win32::ex_style(h) & 0x8 != 0);
                check(&mut st, "a Layer command re-applies z-order (normal is not topmost)", z == Some(false), String::new());
                check(&mut st, "a Theme command re-themes every widget", self.ws.theme.palette == "Daylight" && self.theme.color("text").to_hex() == "#141a2a", String::new());
                self.apply(el, Cmd::Remove("digital_clock-2".into()));
                self.open_settings(el);
                next = 900;
            }
            11 => {
                check(&mut st, "removing a widget closes its window", self.idx("digital_clock-2").is_none() && self.hwnds().len() == 4, format!("({} windows)", self.hwnds().len()));
                check(&mut st, "the settings window opens", self.settings.is_some(), String::new());
                if let Some(s) = &self.settings {
                    s.window.request_redraw();
                }
                next = 700;
            }
            12 => {
                check(&mut st, "the settings window survives rendering frames", self.settings.is_some() && self.gpu_lost.is_none(), String::new());
                self.apply(el, Cmd::Close);
                next = 300;
            }
            13 => {
                check(&mut st, "the settings window closes", self.settings.is_none(), String::new());
                // ---- Show Desktop (ADR-002), without touching the real desktop ----
                let host = win32::desktop_icon_host();
                check(&mut st, "the desktop-icon host is found (Progman on 24H2+)", host.is_some(), format!("({host:?})"));
                if let Some(s) = self.sentinel.as_ref() {
                    check(&mut st, "normal state: the sentinel sits above the icon host", win32::desktop_state(s) == Some(false), format!("({:?})", win32::desktop_state(s)));
                    // detection against a fake host, raised and lowered around the sentinel
                    if let Some(fake) = win32::Sentinel::new() {
                        fake.show();
                        let f = fake.hwnd();
                        let (a, b) = (win32::desktop_state_with(f, s), { fake.raise(); win32::desktop_state_with(f, s) });
                        fake.sink();
                        let c = win32::desktop_state_with(f, s);
                        check(&mut st, "a host raised above the sentinel reads as Show Desktop, and back", (a, b, c) == (Some(false), Some(true), Some(false)), format!("({a:?} -> {b:?} -> {c:?})"));
                        // now play Explorer: the fake host goes to the top of the normal band, as in Show Desktop
                        fake.raise();
                        self.test_host = Some(f);
                        st.fake = Some(fake);
                    }
                } else {
                    check(&mut st, "the z-order sentinel exists", false, String::new());
                }
                self.apply(el, Cmd::Z("icon_list-1".into(), "bottom".into()));
                self.desktop_shown = true; // force the state: Explorer is not asked to hide anything
                self.apply_show_desktop();
                next = 350;
            }
            14 => {
                let topmost = |s: &Self, id: &str| s.idx(id).and_then(|i| s.wins[i].window.as_ref()).and_then(|w| win32::hwnd_of(w)).map(|h| win32::ex_style(h) & 0x8 != 0);
                check(&mut st, "Desktop-layer widgets float above the desktop while it is shown", topmost(self, "icon_folder-1") == Some(true) && topmost(self, "digital_clock-1") == Some(true), String::new());
                check(&mut st, "Bottom-layer widgets stay under it, hidden by design", topmost(self, "icon_list-1") == Some(false), String::new());
                let order = win32::z_order();
                let me_pid = std::process::id();
                let me = self.idx("icon_folder-1").and_then(|i| self.wins[i].window.as_ref()).and_then(|w| win32::hwnd_of(w)).and_then(win32::z_index);
                let fz = st.fake.as_ref().and_then(|f| win32::z_index(f.hwnd()));
                // What the walk-up guarantees, independent of the environment: the widget is above
                // the raised desktop, and no foreign topmost window is left between the two, so it
                // sits directly under the backmost foreign topmost window (the taskbar layer).
                // (The taskbar's own z-index is no yardstick: Windows demotes it below normal
                // windows while a fullscreen app is in front.)
                let between: Vec<String> = match (me, fz) {
                    (Some(m), Some(f)) => order.iter().enumerate().filter(|(i, e)| *i > m && *i < f && e.topmost && e.pid != me_pid).map(|(_, e)| e.class.clone()).collect(),
                    _ => vec![],
                };
                let detail = format!("(widget z {me:?}, raised desktop z {fz:?}, foreign topmost windows in between: {between:?})");
                check(&mut st, "floating widgets sit directly under the backmost topmost window, above the raised desktop", matches!((me, fz), (Some(m), Some(f)) if m < f) && between.is_empty(), detail);
                self.desktop_shown = false;
                self.apply_show_desktop();
                next = 350;
            }
            15 => {
                let hosts = win32::desktop_icon_host().and_then(win32::z_index);
                let f = self.idx("icon_folder-1").and_then(|i| self.wins[i].window.as_ref()).and_then(|w| win32::hwnd_of(w));
                let topmost = f.map(|h| win32::ex_style(h) & 0x8 != 0);
                let me = f.and_then(win32::z_index);
                check(&mut st, "leaving Show Desktop sinks widgets back above the desktop layer", topmost == Some(false) && matches!((me, hosts), (Some(m), Some(h)) if m < h), format!("(widget z {me:?}, host z {hosts:?}, topmost {topmost:?})"));
                if let Some(h) = f {
                    let before = win32::z_index(h);
                    win32::try_raise(h); // a foreign attempt to bring the widget to the front
                    let after = win32::z_index(h);
                    check(&mut st, "the z-order guard vetoes a foreign raise", before == after, format!("(z {before:?} -> {after:?})"));
                    win32::float_over_desktop(h, win32::desktop_icon_host().unwrap_or(h));
                    let ok = win32::ex_style(h) & 0x8 != 0;
                    win32::sink_to_desktop(h);
                    check(&mut st, "our own z-order calls pass the guard (NOSENDCHANGING)", ok && win32::ex_style(h) & 0x8 == 0, String::new());
                }
                self.apply(el, Cmd::Z("icon_list-1".into(), "desktop".into()));
                self.test_host = None;
                st.fake = None; // drops the fake host window
                next = 200;
            }
            16 => {
                let pass = st.checks.iter().filter(|c| c.1).count();
                let total = st.checks.len();
                let report: Vec<String> = st.checks.iter().map(|(n, ok)| format!("{} {n}", if *ok { "PASS" } else { "FAIL" })).collect();
                let _ = std::fs::write(self.opts.dir.join("selftest.txt"), format!("{pass}/{total}\n{}\n", report.join("\n")));
                self.log(format!("selftest: {pass}/{total} passed"));
                el.exit();
                return;
            }
            _ => {}
        }
        st.step += 1;
        st.at = now + Duration::from_millis(next);
        self.selftest = Some(st);
    }

    pub(super) fn selftest_abort(&mut self, el: &ActiveEventLoop, st: SelfTest, why: &str) {
        self.log(format!("selftest aborted: {why}"));
        let lines: Vec<String> = st.checks.iter().map(|(n, ok)| format!("{} {n}", if *ok { "PASS" } else { "FAIL" })).collect();
        let _ = std::fs::write(self.opts.dir.join("selftest.txt"), format!("aborted: {why}\n{}", lines.join("\n")));
        el.exit();
    }
}
