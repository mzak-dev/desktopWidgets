//! Win32 glue winit does not expose (winit#2059): extended styles, desktop
//! z-order, and a z-order report used by the Phase 0 spike.

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, FindWindowW, GWL_EXSTYLE, GetClassNameW, GetWindowLongPtrW,
    HWND_BOTTOM, HWND_NOTOPMOST, HWND_TOPMOST, IsWindowVisible, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SetWindowLongPtrW, SetWindowPos, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
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

const FLAGS: windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS =
    windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS(
        SWP_NOMOVE.0 | SWP_NOSIZE.0 | SWP_NOACTIVATE.0,
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

/// The window that hosts the desktop icons (`SHELLDLL_DefView`'s parent):
/// Progman on Windows 11 24H2+, a WorkerW before that. Never hardcode either.
pub fn desktop_icon_host() -> Option<HWND> {
    unsafe {
        let progman = FindWindowW(w!("Progman"), None).ok()?;
        if FindWindowExW(Some(progman), None, w!("SHELLDLL_DefView"), None).is_ok() {
            return Some(progman);
        }
        z_order()
            .into_iter()
            .filter(|e| e.class == "WorkerW")
            .map(|e| HWND(e.hwnd as *mut _))
            .find(|&h| FindWindowExW(Some(h), None, w!("SHELLDLL_DefView"), None).is_ok())
    }
}

pub struct ZEntry {
    pub hwnd: isize,
    pub class: String,
    pub visible: bool,
}

/// Every top-level window, topmost first.
pub fn z_order() -> Vec<ZEntry> {
    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let out = unsafe { &mut *(lparam.0 as *mut Vec<ZEntry>) };
        out.push(ZEntry {
            hwnd: hwnd.0 as isize,
            class: class_of(hwnd),
            visible: unsafe { IsWindowVisible(hwnd) }.as_bool(),
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

// ---- Show Desktop ---------------------------------------------------------------

const SHELL_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "ThumbnailDeviceHelperWnd",
    "DummyDWMListenerWindow",
    "Winit Thread Event Target",
    "NotifyIconOverflowWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "XamlExplorerHostIslandWindow",
    "Windows.UI.Core.CoreWindow",
    "ForegroundStaging",
    "MSCTFIME UI",
    "IME",
];

/// True when no ordinary application window is showing, i.e. the desktop is
/// what you are looking at (Win+D, or every window minimised). Widgets in
/// Desktop mode must then float above the desktop layer or they vanish.
pub fn desktop_shown() -> bool {
    use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, GetWindowThreadProcessId, IsIconic};
    let me = unsafe { GetCurrentProcessId() };
    for e in z_order() {
        if !e.visible || SHELL_CLASSES.contains(&e.class.as_str()) {
            continue;
        }
        let hwnd = HWND(e.hwnd as *mut _);
        unsafe {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == me || IsIconic(hwnd).as_bool() {
                continue;
            }
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            if ex & (WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) != 0 && ex & 0x0004_0000 == 0 {
                continue; // tool windows and non-activating overlays are not "apps" (0x40000 = WS_EX_APPWINDOW)
            }
            let mut cloaked = 0u32;
            let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut _, 4);
            if cloaked != 0 {
                continue;
            }
            let mut r = windows::Win32::Foundation::RECT::default();
            let _ = GetWindowRect(hwnd, &mut r);
            if r.right - r.left > 8 && r.bottom - r.top > 8 {
                return false;
            }
        }
    }
    true
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

/// Call `on_change` whenever the foreground window changes or a window is
/// minimised/restored: the events that flip Show Desktop. Event driven, so an
/// idle desktop costs nothing. Hooks are out-of-context and need this thread's
/// message loop, which winit already runs.
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
