//! Win32 glue winit does not expose (winit#2059): extended styles, desktop
//! z-order, and a z-order report used by the Phase 0 spike.

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

/// Hide from Alt+Tab (`WS_EX_TOOLWINDOW`; winit's `with_skip_taskbar` only
/// covers the taskbar) and never take focus on click (`WS_EX_NOACTIVATE`).
/// Click-through is *not* here: use `Window::set_cursor_hittest(false)`.
pub fn apply_widget_styles(window: &Window) {
    let Some(hwnd) = hwnd_of(window) else { return };
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let add = (WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) as isize;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | add);
    }
}

/// Where an Instance sits relative to everything else (ADR-002).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZMode {
    /// Just above the desktop, below every application window.
    Desktop,
    /// Bottom of the normal z-band.
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

/// Rainmeter's `ZPOS_FLAGS`. `SWP_NOSENDCHANGING` matters: our own z-order calls
/// skip `WM_WINDOWPOSCHANGING`, so the guard that vetoes *foreign* z-order
/// changes (see `set_z_guard`) never blocks us.
const FLAGS: windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS =
    windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS(
        SWP_NOMOVE.0 | SWP_NOSIZE.0 | SWP_NOOWNERZORDER.0 | SWP_NOACTIVATE.0 | SWP_NOSENDCHANGING.0,
    );

/// Re-assert `mode` for `hwnd`. Cheap and idempotent: z-order on Windows is a
/// process, not a state, so callers re-run this when the shell reshuffles.
pub fn set_zmode(hwnd: HWND, mode: ZMode) {
    unsafe {
        let _ = match mode {
            ZMode::Topmost => SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, FLAGS),
            ZMode::Normal => SetWindowPos(hwnd, Some(HWND_NOTOPMOST), 0, 0, 0, 0, FLAGS),
            ZMode::Bottom | ZMode::Desktop => {
                SetWindowPos(hwnd, Some(HWND_BOTTOM), 0, 0, 0, 0, FLAGS)
            }
        };
    }
}

/// Put `hwnd` directly *above* `host` by sending `host` behind it. This
/// reorders a shell-owned window; it is the candidate for `ZMode::Desktop`
/// that the spike evaluates against a plain `HWND_BOTTOM`.
pub fn place_above(hwnd: HWND, host: HWND) {
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_BOTTOM), 0, 0, 0, 0, FLAGS);
        let _ = SetWindowPos(host, Some(hwnd), 0, 0, 0, 0, FLAGS);
    }
}

fn class_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// The window that hosts the desktop icons (`SHELLDLL_DefView`'s parent), found
/// the way Rainmeter does: from the shell window. On Windows 11 24H2+ that is
/// Progman itself; on earlier builds it is a visible WorkerW of the shell's own
/// process. Never hardcode either.
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

/// Every top-level window, topmost first.
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

// ---- launching, monitors, geometry ------------------------------------------------

/// `ShellExecute open`: files, folders, URLs, shortcuts.
pub fn open(target: &str) -> bool {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{HSTRING, PCWSTR};
    let r = unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(target), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
    r.0 as isize > 32
}

/// Monitors with their work areas (taskbar excluded), physical px.
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

/// Move and resize in one atomic call, without touching z-order or focus.
pub fn set_rect(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
    use windows::Win32::UI::WindowsAndMessaging::{SWP_NOZORDER, SET_WINDOW_POS_FLAGS};
    unsafe {
        let _ = SetWindowPos(hwnd, None, x, y, w, h, SET_WINDOW_POS_FLAGS(SWP_NOACTIVATE.0 | SWP_NOZORDER.0));
    }
}

/// Toggle `WS_EX_NOACTIVATE`: off in Edit Mode so keys (Ctrl+Z, Esc) reach us.
pub fn set_no_activate(window: &Window, on: bool) {
    let Some(hwnd) = hwnd_of(window) else { return };
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let na = WS_EX_NOACTIVATE.0 as isize;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (if on { ex | na } else { ex & !na }) | WS_EX_TOOLWINDOW.0 as isize);
    }
}

/// Put `hwnd` above every window in `others` without leaving the widget band:
/// each sibling is sent to just below `hwnd`, so nothing else is reordered
/// (decision 23: above sibling widgets, still below application windows).
pub fn raise_above(hwnd: HWND, others: &[HWND]) {
    for &o in others {
        if o != hwnd {
            unsafe {
                let _ = SetWindowPos(o, Some(hwnd), 0, 0, 0, 0, FLAGS);
            }
        }
    }
}

// ---- Show Desktop (Rainmeter's mechanism, from System.cpp) --------------------------------

/// A hidden window pinned at `HWND_BOTTOM`: Rainmeter's "System" window.
///
/// In the normal state it sits *above* the desktop-icon host (the shell never
/// lets a window go below its own). Show Desktop works by Explorer raising the
/// host above everything, so "the sentinel is now below the host" is exactly,
/// and only, Show Desktop. No guessing from which application windows are open.
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
            let _ = SetWindowPos(h, Some(HWND_BOTTOM), 0, 0, 0, 0, FLAGS);
            Some(Sentinel(h))
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.0
    }

    /// Make it visible (zero size, no focus). Only tests use this: a hidden window is
    /// not held to the z-order band rules that the real, visible desktop host obeys.
    pub fn show(&self) {
        use windows::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, ShowWindow};
        unsafe {
            let _ = ShowWindow(self.0, SW_SHOWNOACTIVATE);
        }
    }

    /// Put it back at the bottom (a test hook and a cheap re-assert).
    pub fn sink(&self) {
        unsafe {
            let _ = SetWindowPos(self.0, Some(HWND_BOTTOM), 0, 0, 0, 0, FLAGS);
        }
    }

    pub fn raise(&self) {
        unsafe {
            let _ = SetWindowPos(self.0, Some(windows::Win32::UI::WindowsAndMessaging::HWND_TOP), 0, 0, 0, 0, FLAGS);
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

/// Handles of every top-level window, topmost first (cheaper than `z_order`).
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

/// The whole detection: the host has been raised above the sentinel. Pure, so
/// it is testable on synthetic z-orders. `None` when either window is missing.
pub fn is_desktop_shown(order: &[isize], host: isize, sentinel: isize) -> Option<bool> {
    let h = order.iter().position(|&w| w == host)?;
    let s = order.iter().position(|&w| w == sentinel)?;
    Some(s > h)
}

/// Show Desktop state against an explicit host window (tests use a fake one).
pub fn desktop_state_with(host: HWND, sentinel: &Sentinel) -> Option<bool> {
    is_desktop_shown(&z_handles(), host.0 as isize, sentinel.0.0 as isize)
}

/// Is the desktop being shown right now? `None` when the shell is not there.
pub fn desktop_state(sentinel: &Sentinel) -> Option<bool> {
    let host = desktop_icon_host()?;
    if !unsafe { IsWindowVisible(host) }.as_bool() {
        return None;
    }
    desktop_state_with(host, sentinel)
}

/// Show Desktop response for a Desktop-mode widget (Rainmeter's `ChangeZPos`):
/// join the topmost band, then walk *up* from the host and drop in directly
/// under the first foreign topmost window that accepts us. With the host raised
/// to the top of the normal band, as Explorer does, that is the taskbar layer,
/// so the widget floats just above the desktop and the taskbar still draws
/// over it. Candidates can refuse (other integrity level, a dying window), so,
/// like Rainmeter, keep trying successive ones.
pub fn float_over_desktop(hwnd: HWND, host: HWND) {
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{GW_HWNDPREV, GetWindow, GetWindowThreadProcessId, WS_EX_TOPMOST};
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, FLAGS);
        let me = GetCurrentProcessId();
        let mut w = host;
        while let Ok(prev) = GetWindow(w, GW_HWNDPREV) {
            w = prev;
            let mut pid = 0u32;
            GetWindowThreadProcessId(w, Some(&mut pid));
            if pid != me && (GetWindowLongPtrW(w, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0) != 0 && SetWindowPos(hwnd, Some(w), 0, 0, 0, 0, FLAGS).is_ok() {
                return;
            }
        }
    }
}

/// Back to the desktop layer (leaves the topmost band, drops under app windows).
pub fn sink_to_desktop(hwnd: HWND) {
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_BOTTOM), 0, 0, 0, 0, FLAGS);
    }
}

/// A foreign attempt to raise a window, sent the normal way (so a guard can see
/// it). Test hook: our own calls never do this.
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

/// Veto foreign z-order changes for this window (Rainmeter's `OnWindowPosChanging`
/// for On Desktop / Bottom skins). Our own calls use `SWP_NOSENDCHANGING`.
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

/// Call `on_change` when the foreground window changes or a window is minimised
/// or restored: the events around Show Desktop. Rainmeter hooks the foreground
/// event and also polls every 250 ms; here it is events only (the caller runs a
/// short retry ladder, since Explorer reorders a few ms *after* the event), so an
/// idle desktop costs nothing.
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

// ---- display changes --------------------------------------------------------------

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

/// Call `on_change` when monitors or the work area change (hot-plug, resolution,
/// taskbar moved). Subclasses one window; topmost-level windows all receive the
/// broadcast, so one is enough while it lives.
pub fn watch_display_changes(window: &Window, on_change: impl Fn() + Send + Sync + 'static) {
    use windows::Win32::UI::Shell::SetWindowSubclass;
    let _ = DISPLAY_EVENT.set(Box::new(on_change));
    if let Some(h) = hwnd_of(window) {
        unsafe {
            let _ = SetWindowSubclass(h, Some(display_subclass), 1, 0);
        }
    }
}

/// Extended window style bits (for tests and diagnostics).
pub fn ex_style(hwnd: HWND) -> u32 {
    unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 }
}

/// Index of `hwnd` in the top-level z-order (0 = topmost); larger = further back.
pub fn z_index(hwnd: HWND) -> Option<usize> {
    z_order().iter().position(|e| e.hwnd == hwnd.0 as isize)
}

#[cfg(test)]
mod tests {
    use super::is_desktop_shown;

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
