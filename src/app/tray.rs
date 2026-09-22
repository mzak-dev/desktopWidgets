//! How Wayfinder hooks into Windows outside its widgets: the tray icon and
//! menu, the Edit Mode hotkey, and starting with Windows.

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

/// Start with Windows (HKCU Run key).
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
    pub(super) fn init_tray(&mut self) {
        let menu = Menu::new();
        let items = [
            MenuItem::with_id("edit", "Edit layout   Ctrl+Alt+E", true, None),
            MenuItem::with_id("settings", "Settings...", true, None),
            MenuItem::with_id("reload", "Reload widgets and themes", true, None),
            MenuItem::with_id("folder", "Open widgets folder", true, None),
        ];
        for it in &items {
            let _ = menu.append(it);
        }
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&MenuItem::with_id("quit", "Quit Wayfinder", true, None));
        let p = self.proxy.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let _ = p.send_event(UserEvent::Menu(e.id.0.clone()));
        }));
        let p = self.proxy.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click { button: tray_icon::MouseButton::Left, button_state: tray_icon::MouseButtonState::Up, .. } = e {
                let _ = p.send_event(UserEvent::TrayClick);
            }
        }));
        match TrayIconBuilder::new().with_menu(Box::new(menu)).with_menu_on_left_click(false).with_tooltip("Wayfinder").with_icon(tray_icon_image()).build() {
            Ok(t) => self.tray = Some(t),
            Err(e) => self.log(format!("tray icon failed: {e}")),
        }
    }

    pub(super) fn init_hotkey(&mut self) {
        match GlobalHotKeyManager::new() {
            Ok(m) => {
                let hk = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyE);
                if let Err(e) = m.register(hk) {
                    self.log(format!("hotkey Ctrl+Alt+E unavailable ({e}); use the tray menu for Edit Mode"));
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
