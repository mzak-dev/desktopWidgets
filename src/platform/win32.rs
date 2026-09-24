//! Win32 glue winit does not expose (winit#2059).

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, GWL_EXSTYLE, GetClassNameW, GetWindowLongPtrW,
    HWND_BOTTOM, HWND_NOTOPMOST, HWND_TOPMOST, IsWindowVisible, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOOWNERZORDER, SWP_NOSENDCHANGING, SWP_NOSIZE, SetWindowLongPtrW, SetWindowPos, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, w};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

pub fn hwnd_of(window: &Window) -> Option<HWND> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return None;
    };
    Some(HWND(h.hwnd.get() as *mut _))
}

/// winit's `with_skip_taskbar` doesn't hide from Alt+Tab; click-through is
/// `Window::set_cursor_hittest(false)`, not a style.
pub fn apply_widget_styles(window: &Window) {
    let Some(hwnd) = hwnd_of(window) else { return };
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let add = (WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) as isize;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | add);
    }
}

/// ADR-002.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZMode {
    Desktop,
    Bottom,
    Normal,
    Topmost,
}

impl ZMode {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "desktop" => Self::Desktop,
            "bottom" => Self::Bottom,
            "normal" => Self::Normal,
            "topmost" => Self::Topmost,
            _ => return None,
        })
    }
}

/// Rainmeter's `ZPOS_FLAGS`. `SWP_NOSENDCHANGING` lets our calls pass the
/// `set_z_guard` veto on foreign z-order changes.
const OUR_ZPOS_FLAGS: windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS =
    windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS(
        SWP_NOMOVE.0 | SWP_NOSIZE.0 | SWP_NOOWNERZORDER.0 | SWP_NOACTIVATE.0 | SWP_NOSENDCHANGING.0,
    );

/// Idempotent: callers re-assert it whenever the shell reshuffles.
pub fn set_zmode(hwnd: HWND, mode: ZMode) {
    unsafe {
        let _ = match mode {
            ZMode::Topmost => SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, OUR_ZPOS_FLAGS),
            ZMode::Normal => SetWindowPos(hwnd, Some(HWND_NOTOPMOST), 0, 0, 0, 0, OUR_ZPOS_FLAGS),
            ZMode::Bottom | ZMode::Desktop => {
                SetWindowPos(hwnd, Some(HWND_BOTTOM), 0, 0, 0, 0, OUR_ZPOS_FLAGS)
            }
        };
    }
}

/// Reorders a shell-owned window: kept for the Phase 0 spike's comparison.
pub fn place_above(hwnd: HWND, host: HWND) {
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_BOTTOM), 0, 0, 0, 0, OUR_ZPOS_FLAGS);
        let _ = SetWindowPos(host, Some(hwnd), 0, 0, 0, 0, OUR_ZPOS_FLAGS);
    }
}

fn class_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// `SHELLDLL_DefView`'s parent, found from the shell window like Rainmeter: Progman
/// on Windows 11 24H2+, a visible WorkerW before that. Never hardcode either.
pub fn desktop_icon_host() -> Option<HWND> {
    use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};
    unsafe {
        let shell = GetShellWindow();
        if shell.0.is_null() || class_of(shell) != "Progman" {
            return None; // Explorer is not the shell (or not running)
        }
        if FindWindowExW(Some(shell), None, w!("SHELLDLL_DefView"), None).is_ok() {
            return Some(shell);
        }
        let mut shell_pid = 0u32;
        GetWindowThreadProcessId(shell, Some(&mut shell_pid));
        z_order()
            .into_iter()
            .filter(|e| e.class == "WorkerW" && e.visible)
            .map(|e| HWND(e.hwnd as *mut _))
            .find(|&h| {
                let mut pid = 0u32;
                GetWindowThreadProcessId(h, Some(&mut pid));
                pid == shell_pid && FindWindowExW(Some(h), None, w!("SHELLDLL_DefView"), None).is_ok()
            })
    }
}

pub struct ZEntry {
    pub hwnd: isize,
    pub class: String,
    pub visible: bool,
    pub topmost: bool,
    pub pid: u32,
}

pub fn z_order() -> Vec<ZEntry> {
    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let out = unsafe { &mut *(lparam.0 as *mut Vec<ZEntry>) };
        let mut pid = 0u32;
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        out.push(ZEntry {
            hwnd: hwnd.0 as isize,
            class: class_of(hwnd),
            visible: unsafe { IsWindowVisible(hwnd) }.as_bool(),
            topmost: unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32 & windows::Win32::UI::WindowsAndMessaging::WS_EX_TOPMOST.0 != 0,
            pid,
        });
        BOOL(1)
    }
    let mut out = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut out as *mut _ as isize));
    }
    out
}


/// `ShellExecute open`: files, folders, URLs, shortcuts.
pub fn open(target: &str) -> bool {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{HSTRING, PCWSTR};
    let r = unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(target), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
    r.0 as isize > 32
}

/// (corner preference, accent state). `corners` is the card's DWMWCP value
/// (`card::blur_corners`). Rounding is only for the blur: left on, DWM keeps drawing
/// its border at the window edge, outside the card once the shadow gutter is back.
pub fn blur_window_attributes(on: bool, corners: i32) -> (i32, u32) {
    if on { (corners, 3) } else { (0, 0) } // ACCENT_ENABLE_BLURBEHIND, or DWMWCP_DEFAULT + disabled
}

/// Accent-policy blur (Rainmeter's): unlike DwmEnableBlurBehindWindow it also
/// blurs wallpaper and icons, but ignores SetWindowRgn, so the window must be
/// exactly the card.
pub fn set_blur(hwnd: HWND, on: bool, corners: i32) {
    use windows::Win32::Graphics::Dwm::DwmSetWindowAttribute;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    #[repr(C)]
    struct AccentPolicy {
        state: u32,
        flags: u32,
        gradient: u32,
        animation: u32,
    }
    #[repr(C)]
    struct CompositionAttrib {
        attrib: u32,
        data: *mut AccentPolicy,
        size: usize,
    }
    type SetWca = unsafe extern "system" fn(HWND, *mut CompositionAttrib) -> BOOL;
    let (corners, accent) = blur_window_attributes(on, corners);
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(33), &corners as *const _ as *const _, size_of::<i32>() as u32);
        let Ok(user32) = LoadLibraryW(w!("user32.dll")) else { return };
        let Some(f) = GetProcAddress(user32, windows::core::s!("SetWindowCompositionAttribute")) else { return };
        let f: SetWca = std::mem::transmute(f);
        // WCA_ACCENT_POLICY = 19
        let mut policy = AccentPolicy { state: accent, flags: 0, gradient: 0, animation: 0 };
        let mut data = CompositionAttrib { attrib: 19, data: &mut policy, size: size_of::<AccentPolicy>() };
        let _ = f(hwnd, &mut data);
    }
}

/// A `.lnk` target is copied as is; a name clash gets " (2)", " (3)"... so nothing is overwritten.
pub fn create_shortcut(dir: &std::path::Path, target: &std::path::Path) -> Option<std::path::PathBuf> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{HSTRING, Interface};
    std::fs::create_dir_all(dir).ok()?;
    let stem = target.file_stem()?.to_string_lossy().into_owned();
    let mut out = dir.join(format!("{stem}.lnk"));
    for n in 2.. {
        if !out.exists() {
            break;
        }
        out = dir.join(format!("{stem} ({n}).lnk"));
    }
    if target.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk")) {
        return std::fs::copy(target, &out).ok().map(|_| out);
    }
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        link.SetPath(&HSTRING::from(target.as_os_str())).ok()?;
        if let Some(dir) = target.parent() {
            let _ = link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()));
        }
        link.cast::<IPersistFile>().ok()?.Save(&HSTRING::from(out.as_os_str()), true).ok()?;
    }
    Some(out)
}

/// Physical px.
pub fn monitors(el: &winit::event_loop::ActiveEventLoop) -> Vec<crate::workspace::MonitorInfo> {
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITORINFO, MONITORINFOEXW};
    use winit::platform::windows::MonitorHandleExtWindows;
    el.available_monitors()
        .map(|m| {
            let (p, s) = (m.position(), m.size());
            let mut info = MONITORINFOEXW::default();
            info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
            let ok = unsafe { GetMonitorInfoW(HMONITOR(m.hmonitor() as *mut _), &mut info as *mut _ as *mut MONITORINFO) }.as_bool();
            let work = if ok {
                let r = info.monitorInfo.rcWork;
                (r.left, r.top, (r.right - r.left).max(1) as u32, (r.bottom - r.top).max(1) as u32)
            } else {
                (p.x, p.y, s.width, s.height)
            };
            let name = m.name().unwrap_or_default();
            crate::workspace::MonitorInfo { name, x: p.x, y: p.y, w: s.width, h: s.height, scale: m.scale_factor(), work }
        })
        .collect()
}

pub fn cursor_pos() -> (i32, i32) {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

/// One atomic call that leaves z-order and focus alone.
pub fn set_rect(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
    use windows::Win32::UI::WindowsAndMessaging::{SWP_NOZORDER, SET_WINDOW_POS_FLAGS};
    unsafe {
        let _ = SetWindowPos(hwnd, None, x, y, w, h, SET_WINDOW_POS_FLAGS(SWP_NOACTIVATE.0 | SWP_NOZORDER.0));
    }
}

/// Off in Edit Mode so keys (Ctrl+Z, Esc) reach us.
pub fn set_no_activate(window: &Window, on: bool) {
    let Some(hwnd) = hwnd_of(window) else { return };
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let na = WS_EX_NOACTIVATE.0 as isize;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (if on { ex | na } else { ex & !na }) | WS_EX_TOOLWINDOW.0 as isize);
    }
}

/// Sends each sibling just below `hwnd`, so it stays under application windows (decision 23).
pub fn raise_above(hwnd: HWND, others: &[HWND]) {
    for &o in others {
        if o != hwnd {
            unsafe {
                let _ = SetWindowPos(o, Some(hwnd), 0, 0, 0, 0, OUR_ZPOS_FLAGS);
            }
        }
    }
}


/// Rainmeter's hidden "System" window at `HWND_BOTTOM`. The shell keeps it above
/// the icon host, so "the sentinel is below the host" is exactly Show Desktop.
pub struct Sentinel(HWND);

unsafe extern "system" fn sentinel_proc(h: HWND, m: u32, w: windows::Win32::Foundation::WPARAM, l: LPARAM) -> windows::Win32::Foundation::LRESULT {
    unsafe { windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(h, m, w, l) }
}

impl Sentinel {
    pub fn new() -> Option<Sentinel> {
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, RegisterClassExW, WNDCLASSEXW, WS_DISABLED, WS_POPUP};
        unsafe {
            let hinst = GetModuleHandleW(None).ok()?;
            let class = w!("WayfinderSystem");
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(sentinel_proc),
                hInstance: windows::Win32::Foundation::HINSTANCE(hinst.0),
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassExW(&wc); // already registered on a second call: fine
            let h = CreateWindowExW(WS_EX_TOOLWINDOW, class, w!("System"), WS_POPUP | WS_DISABLED, 0, 0, 0, 0, None, None, Some(windows::Win32::Foundation::HINSTANCE(hinst.0)), None).ok()?;
            let _ = SetWindowPos(h, Some(HWND_BOTTOM), 0, 0, 0, 0, OUR_ZPOS_FLAGS);
            Some(Sentinel(h))
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.0
    }

    /// Tests only: a hidden window isn't held to the z-order band rules the real host obeys.
    pub fn show(&self) {
        use windows::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, ShowWindow};
        unsafe {
            let _ = ShowWindow(self.0, SW_SHOWNOACTIVATE);
        }
    }

    pub fn sink(&self) {
        unsafe {
            let _ = SetWindowPos(self.0, Some(HWND_BOTTOM), 0, 0, 0, 0, OUR_ZPOS_FLAGS);
        }
    }

    pub fn raise(&self) {
        unsafe {
            let _ = SetWindowPos(self.0, Some(windows::Win32::UI::WindowsAndMessaging::HWND_TOP), 0, 0, 0, 0, OUR_ZPOS_FLAGS);
        }
    }
}

impl Drop for Sentinel {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.0);
        }
    }
}

fn z_handles() -> Vec<isize> {
    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        unsafe { &mut *(lparam.0 as *mut Vec<isize>) }.push(hwnd.0 as isize);
        BOOL(1)
    }
    let mut out = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

/// Pure, so it is tested on synthetic z-orders.
pub fn is_desktop_shown(order: &[isize], host: isize, sentinel: isize) -> Option<bool> {
    let h = order.iter().position(|&w| w == host)?;
    let s = order.iter().position(|&w| w == sentinel)?;
    Some(s > h)
}

pub fn desktop_state_with(host: HWND, sentinel: &Sentinel) -> Option<bool> {
    is_desktop_shown(&z_handles(), host.0 as isize, sentinel.0.0 as isize)
}

pub fn desktop_state(sentinel: &Sentinel) -> Option<bool> {
    let host = desktop_icon_host()?;
    if !unsafe { IsWindowVisible(host) }.as_bool() {
        return None;
    }
    desktop_state_with(host, sentinel)
}

/// Rainmeter's `ChangeZPos`: join the topmost band under the first foreign topmost
/// window above the raised host (the taskbar layer). Candidates can refuse, so keep trying.
pub fn float_over_desktop(hwnd: HWND, host: HWND) {
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{GW_HWNDPREV, GetWindow, GetWindowThreadProcessId, WS_EX_TOPMOST};
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, OUR_ZPOS_FLAGS);
        let me = GetCurrentProcessId();
        let mut w = host;
        while let Ok(prev) = GetWindow(w, GW_HWNDPREV) {
            w = prev;
            let mut pid = 0u32;
            GetWindowThreadProcessId(w, Some(&mut pid));
            if pid != me && (GetWindowLongPtrW(w, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0) != 0 && SetWindowPos(hwnd, Some(w), 0, 0, 0, 0, OUR_ZPOS_FLAGS).is_ok() {
                return;
            }
        }
    }
}

pub fn sink_to_desktop(hwnd: HWND) {
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_BOTTOM), 0, 0, 0, 0, OUR_ZPOS_FLAGS);
    }
}

/// Test hook: a raise the guard can see, unlike our own calls.
pub fn try_raise(hwnd: HWND) {
    use windows::Win32::UI::WindowsAndMessaging::{HWND_TOP, SET_WINDOW_POS_FLAGS};
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, 0, 0, SET_WINDOW_POS_FLAGS(SWP_NOMOVE.0 | SWP_NOSIZE.0 | SWP_NOACTIVATE.0));
    }
}

unsafe extern "system" fn zguard_subclass(
    hwnd: HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: LPARAM,
    _id: usize,
    guard: usize,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::Shell::DefSubclassProc;
    use windows::Win32::UI::WindowsAndMessaging::{SWP_NOZORDER, WINDOWPOS};
    const WM_WINDOWPOSCHANGING: u32 = 0x0046;
    if msg == WM_WINDOWPOSCHANGING && guard != 0 && lparam.0 != 0 {
        // Desktop and Bottom widgets stay where we put them: nobody else may re-order them.
        let wp = unsafe { &mut *(lparam.0 as *mut WINDOWPOS) };
        wp.flags |= SWP_NOZORDER;
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// Rainmeter's `OnWindowPosChanging` veto for On Desktop / Bottom skins.
pub fn set_z_guard(window: &Window, on: bool) {
    use windows::Win32::UI::Shell::SetWindowSubclass;
    if let Some(h) = hwnd_of(window) {
        unsafe {
            let _ = SetWindowSubclass(h, Some(zguard_subclass), 2, on as usize);
        }
    }
}

static SHELL_EVENT: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>> = std::sync::OnceLock::new();

unsafe extern "system" fn shell_event_proc(
    _: windows::Win32::UI::Accessibility::HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if let Some(f) = SHELL_EVENT.get() {
        f();
    }
}

/// Foreground and minimise events only, no polling, so an idle desktop costs nothing;
/// Explorer reorders a few ms after them, so the caller re-checks on a short ladder.
pub fn watch_shell_events(on_change: impl Fn() + Send + Sync + 'static) {
    use windows::Win32::UI::Accessibility::SetWinEventHook;
    use windows::Win32::UI::WindowsAndMessaging::{EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZESTART, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS};
    if SHELL_EVENT.set(Box::new(on_change)).is_err() {
        return;
    }
    unsafe {
        let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
        let _ = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(shell_event_proc), 0, 0, flags);
        let _ = SetWinEventHook(EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND, None, Some(shell_event_proc), 0, 0, flags);
    }
}


static DISPLAY_EVENT: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>> = std::sync::OnceLock::new();

unsafe extern "system" fn display_subclass(
    hwnd: HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::Shell::DefSubclassProc;
    const WM_DISPLAYCHANGE: u32 = 0x007E;
    const WM_SETTINGCHANGE: u32 = 0x001A; // includes work-area (taskbar) changes
    if (msg == WM_DISPLAYCHANGE || msg == WM_SETTINGCHANGE) && let Some(f) = DISPLAY_EVENT.get() {
        f();
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// Subclasses one window: every top-level window gets the broadcast.
pub fn watch_display_changes(window: &Window, on_change: impl Fn() + Send + Sync + 'static) {
    use windows::Win32::UI::Shell::SetWindowSubclass;
    let _ = DISPLAY_EVENT.set(Box::new(on_change));
    if let Some(h) = hwnd_of(window) {
        unsafe {
            let _ = SetWindowSubclass(h, Some(display_subclass), 1, 0);
        }
    }
}

pub fn ex_style(hwnd: HWND) -> u32 {
    unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 }
}

/// 0 = topmost.
pub fn z_index(hwnd: HWND) -> Option<usize> {
    z_order().iter().position(|e| e.hwnd == hwnd.0 as isize)
}

#[cfg(test)]
mod tests {
    use super::{blur_window_attributes, is_desktop_shown};

    #[test]
    fn blur_off_returns_to_the_never_blurred_corners() {
        assert_eq!(blur_window_attributes(true, 3), (3, 3)); // the card's corners, ACCENT_ENABLE_BLURBEHIND
        assert_eq!(blur_window_attributes(false, 3), (0, 0)); // DWMWCP_DEFAULT, ACCENT_DISABLED
    }

    #[test]
    fn shown_exactly_when_the_host_is_above_the_sentinel() {
        // z-order is topmost first. Normal: the sentinel sits above the icon host.
        assert_eq!(is_desktop_shown(&[10, 20, 30, 99 /*sentinel*/, 50 /*host*/], 50, 99), Some(false));
        // Show Desktop: Explorer has raised the host above the sentinel.
        assert_eq!(is_desktop_shown(&[10, 50 /*host*/, 20, 99 /*sentinel*/], 50, 99), Some(true));
        // ...even when nothing else is open at all (the old heuristic got this wrong)
        assert_eq!(is_desktop_shown(&[99, 50], 50, 99), Some(false));
        assert_eq!(is_desktop_shown(&[50, 99], 50, 99), Some(true));
    }

    #[test]
    fn missing_windows_mean_unknown_not_shown() {
        assert_eq!(is_desktop_shown(&[1, 2, 3], 50, 99), None, "no host: the shell is not running");
        assert_eq!(is_desktop_shown(&[1, 50, 3], 50, 99), None, "no sentinel yet");
    }
}

pub fn is_visible(hwnd: HWND) -> bool {
    unsafe { IsWindowVisible(hwnd) }.as_bool()
}

/// The Explorer "open" command for `.wfplugin` files.
pub fn association_command(exe: &std::path::Path) -> String {
    format!("\"{}\" --install \"%1\"", exe.display())
}

const PLUGIN_CLASS: &str = "Software\\Classes\\Wayfinder.Plugin";

fn reg_default(subkey: &str) -> Option<String> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::{HSTRING, PCWSTR};
    let mut buf = [0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, &HSTRING::from(subkey), PCWSTR::null(), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut len)) };
    r.is_ok().then(|| String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]))
}

fn reg_set_default(subkey: &str, value: &str) -> Result<(), String> {
    use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegSetValueExW};
    use windows::core::{HSTRING, PCWSTR};
    let mut key = HKEY::default();
    unsafe {
        RegCreateKeyExW(HKEY_CURRENT_USER, &HSTRING::from(subkey), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, None, &mut key, None).ok().map_err(|e| format!("{subkey}: {e}"))?;
        let v: Vec<u16> = value.encode_utf16().chain([0]).collect();
        let r = RegSetValueExW(key, PCWSTR::null(), None, REG_SZ, Some(std::slice::from_raw_parts(v.as_ptr().cast(), v.len() * 2)));
        let _ = RegCloseKey(key);
        r.ok().map_err(|e| format!("{subkey}: {e}"))
    }
}

/// Which exe opens `.wfplugin` files, seen from this one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileOwner {
    Me,
    Nobody,
    Other(std::path::PathBuf),
}

/// The exe an association command starts: its first quoted path, or its first word.
pub fn association_exe(command: &str) -> Option<std::path::PathBuf> {
    let c = command.trim();
    let exe = match c.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?,
        None => c.split_whitespace().next()?,
    };
    (!exe.is_empty()).then(|| exe.into())
}

fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    let norm = |p: &std::path::Path| p.to_string_lossy().replace('/', "\\").to_lowercase();
    norm(a) == norm(b)
}

/// Who opens `.wfplugin` files, from the extension's class and its open command. A command
/// naming an exe that is gone (a moved or deleted build) counts as nobody's.
pub fn file_owner_of(class: Option<&str>, command: Option<&str>, me: &std::path::Path, exists: impl Fn(&std::path::Path) -> bool) -> FileOwner {
    let Some(exe) = command.filter(|_| class == Some("Wayfinder.Plugin")).and_then(association_exe) else { return FileOwner::Nobody };
    if same_file(&exe, me) && command == Some(association_command(me).as_str()) {
        FileOwner::Me
    } else if exists(&exe) && !same_file(&exe, me) {
        FileOwner::Other(exe)
    } else {
        FileOwner::Nobody
    }
}

/// Who opens `.wfplugin` files now, without changing anything.
pub fn file_type_owner(me: &std::path::Path) -> FileOwner {
    let open = format!("{PLUGIN_CLASS}\\shell\\open\\command");
    file_owner_of(reg_default("Software\\Classes\\.wfplugin").as_deref(), reg_default(&open).as_deref(), me, |p| p.is_file())
}

/// Makes double-clicking a `.wfplugin` file install it with this exe, for the current user
/// only, unless another build that still exists already does: two exes started in turn
/// (`wayfinder.exe`, `wayfinder-extra.exe`) would otherwise take it from each other at every
/// start. `take` claims it anyway, when the user asks in Settings. Returns who opens the
/// files afterwards and whether anything changed.
pub fn register_file_type(exe: &std::path::Path, take: bool) -> Result<(FileOwner, bool), String> {
    let command = association_command(exe);
    let open = format!("{PLUGIN_CLASS}\\shell\\open\\command");
    match file_type_owner(exe) {
        FileOwner::Me => return Ok((FileOwner::Me, false)),
        FileOwner::Other(p) if !take => return Ok((FileOwner::Other(p), false)),
        _ => {}
    }
    reg_set_default("Software\\Classes\\.wfplugin", "Wayfinder.Plugin")?;
    reg_set_default(PLUGIN_CLASS, "Wayfinder plugin")?;
    reg_set_default(&format!("{PLUGIN_CLASS}\\DefaultIcon"), &format!("\"{}\",0", exe.display()))?;
    reg_set_default(&open, &command)?;
    unsafe {
        use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
    Ok((FileOwner::Me, true))
}

#[cfg(test)]
mod association_tests {
    use super::*;

    #[test]
    fn association_command_quotes_exe_and_file() {
        let exe = std::path::Path::new("C:\\Program Files\\Wayfinder\\wayfinder.exe");
        assert_eq!(association_command(exe), "\"C:\\Program Files\\Wayfinder\\wayfinder.exe\" --install \"%1\"");
        assert_eq!(association_exe(&association_command(exe)).as_deref(), Some(exe));
        assert_eq!(association_exe("C:\\x.exe --install %1").as_deref(), Some(std::path::Path::new("C:\\x.exe")));
    }

    #[test]
    fn two_builds_never_take_plugin_files_from_each_other() {
        let me = std::path::Path::new("C:\\Apps\\wayfinder-extra.exe");
        let other = std::path::Path::new("C:\\Apps\\wayfinder.exe");
        let class = Some("Wayfinder.Plugin");
        let cmd = |p: &std::path::Path| Some(association_command(p));
        let both_exist = |_: &std::path::Path| true;
        assert_eq!(file_owner_of(class, cmd(me).as_deref(), me, both_exist), FileOwner::Me);
        assert_eq!(file_owner_of(class, cmd(other).as_deref(), me, both_exist), FileOwner::Other(other.into()), "the other build keeps them");
        assert_eq!(file_owner_of(class, cmd(other).as_deref(), me, |p: &std::path::Path| p != other), FileOwner::Nobody, "a build that is gone holds nothing");
        assert_eq!(file_owner_of(None, None, me, both_exist), FileOwner::Nobody);
        assert_eq!(file_owner_of(Some("SomeZipTool"), cmd(other).as_deref(), me, both_exist), FileOwner::Nobody, "the extension points elsewhere");
        let upper = std::path::Path::new("C:\\APPS\\WAYFINDER-EXTRA.EXE");
        assert_eq!(file_owner_of(class, cmd(upper).as_deref(), me, both_exist), FileOwner::Nobody, "ours with an outdated command: rewrite it, never call it someone else's");
    }
}
