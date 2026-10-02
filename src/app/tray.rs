use super::*;

fn tray_icon_image() -> tray_icon::Icon {
    let n = 32u32;
    let mut px = vec![0u8; (n * n * 4) as usize];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 + 0.5 - 16.0, y as f32 + 0.5 - 16.0);
            let r = (fx * fx + fy * fy).sqrt();
            let disc = (0.5 - (r - 14.5)).clamp(0.0, 1.0);
            let ring = (1.0 - ((r - 12.0).abs() - 0.6).max(0.0)).clamp(0.0, 1.0);
            // a needle pointing up-right: two thin triangles along the diagonal
            let k = std::f32::consts::FRAC_1_SQRT_2;
            let (u, v) = ((fx - fy) * k, (fx + fy) * k); // rotate 45 deg
            let width = (8.0 - v.abs()).max(0.0) * 0.0 + (0.6 * (9.0 - u.abs())).max(0.0);
            let needle = if u.abs() < 9.0 && v.abs() < width { 1.0 } else { 0.0 };
            let i = ((y * n + x) * 4) as usize;
            let base = [58.0, 110.0, 240.0];
            let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
            let t = (ring * 0.35 + needle).min(1.0);
            let a = (disc * 255.0) as u8;
            px[i..i + 4].copy_from_slice(&[mix(base[0], 255.0, t) as u8, mix(base[1], 255.0, t) as u8, mix(base[2], 255.0, t) as u8, a]);
        }
    }
    tray_icon::Icon::from_rgba(px, n, n).expect("tray icon")
}

pub(super) fn set_autostart(on: bool) -> Result<(), String> {
    use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW};
    use windows::core::w;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    unsafe {
        let mut key = HKEY::default();
        RegOpenKeyExW(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"), None, KEY_SET_VALUE, &mut key).ok().map_err(|e| e.to_string())?;
        let r = if on {
            let v: Vec<u16> = format!("\"{}\"", exe.display()).encode_utf16().chain([0]).collect();
            RegSetValueExW(key, w!("Wayfinder"), None, REG_SZ, Some(std::slice::from_raw_parts(v.as_ptr().cast(), v.len() * 2)))
        } else {
            RegDeleteValueW(key, w!("Wayfinder"))
        };
        let _ = RegCloseKey(key);
        if on { r.ok().map_err(|e| e.to_string()) } else { Ok(()) } // deleting a missing value is fine
    }
}

impl App {
    pub(super) fn menu_data(&self) -> crate::menu::Data {
        crate::menu::Data { workspaces: self.ws.workspaces.iter().map(|w| w.name.clone()).collect(), active: self.ws.active.clone() }
    }

    /// After a Workspace is added, renamed, removed or shown.
    pub(super) fn refresh_tray(&mut self) {
        let d = self.menu_data();
        if let Some(m) = &mut self.menu {
            m.set_data(d);
        }
    }

    /// A right-click on the tray icon, `at` in physical screen px.
    pub(super) fn show_menu(&mut self, el: &ActiveEventLoop, at: PhysicalPosition<f64>) {
        if self.menu.is_none() {
            let power = self.power();
            match MenuWin::new(el, &mut self.gpu, power) {
                Ok(m) => self.menu = Some(m),
                Err(e) => return self.log(format!("tray menu: {e}")),
            }
        }
        let mon = self.monitors.iter().find(|m| at.x >= m.x as f64 && at.y >= m.y as f64 && at.x < (m.x + m.w as i32) as f64 && at.y < (m.y + m.h as i32) as f64).or(self.monitors.first()).cloned();
        let d = self.menu_data();
        let App { menu: Some(menu), gpu: Some(gpu), text, theme, .. } = self else { return };
        menu.set_data(d);
        menu.show(at, mon.as_ref(), gpu, text, theme);
    }

    pub(super) fn init_tray(&mut self) {
        let p = self.proxy.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click { button, button_state: tray_icon::MouseButtonState::Up, position, .. } = e {
                let _ = p.send_event(match button {
                    tray_icon::MouseButton::Left => UserEvent::TrayClick,
                    tray_icon::MouseButton::Right => UserEvent::TrayMenu(position),
                    _ => return,
                });
            }
        }));
        match TrayIconBuilder::new().with_tooltip("Wayfinder").with_icon(tray_icon_image()).build() {
            Ok(t) => self.tray = Some(t),
            Err(e) => self.log(format!("tray icon failed: {e}")),
        }
    }

    pub(super) fn init_hotkey(&mut self) {
        match GlobalHotKeyManager::new() {
            Ok(m) => {
                // settings::EDIT_KEYS names it; not Ctrl+Alt, which AltGr sends too, so it would eat "ę"
                let hk = HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyE);
                if let Err(e) = m.register(hk) {
                    self.log(format!("hotkey {} unavailable ({e}); use the tray menu for Edit Mode", settings::EDIT_KEYS.join("+")));
                }
                let p = self.proxy.clone();
                GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
                    if e.state == HotKeyState::Pressed {
                        let _ = p.send_event(UserEvent::Hotkey);
                    }
                }));
                self._hotkeys = Some(m);
            }
            Err(e) => self.log(format!("global hotkeys unavailable: {e}")),
        }
    }
}
