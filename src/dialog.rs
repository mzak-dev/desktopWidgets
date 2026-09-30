//! The Win32 bodies of the `Native` and `Prompt` seams: file/folder pickers, clipboard
//! text, message boxes (blocking, on the event-loop thread: they are user-initiated,
//! modal actions). `WinNative` is the adapter; the traits live in `native`.

use std::path::PathBuf;

use crate::native::{Native, Pick, Prompt, one_line};

use windows::Win32::Foundation::{HGLOBAL, HWND};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH};
use windows::core::{HSTRING, PCWSTR};

/// The real dialogs, parented to `hwnd` when there is a window.
#[derive(Clone, Copy, Debug, Default)]
pub struct WinNative {
    pub hwnd: Option<HWND>,
}

impl WinNative {
    pub fn new(hwnd: Option<HWND>) -> Self {
        Self { hwnd }
    }
}

impl Native for WinNative {
    fn pick(&mut self, what: Pick) -> Option<PathBuf> {
        match what {
            Pick::File => pick(self.hwnd, false, &[]),
            Pick::Folder => pick(self.hwnd, true, &[]),
            Pick::Filtered(types) => pick(self.hwnd, false, types),
        }
    }

    fn clipboard(&mut self) -> Option<String> {
        clipboard_text()
    }

    fn set_clipboard(&mut self, text: &str) {
        set_clipboard_text(text);
    }
}

/// Message boxes have no parent: they are asked before a window exists or from the tray.
impl Prompt for WinNative {
    fn confirm(&mut self, title: &str, text: &str) -> bool {
        confirm(title, text)
    }

    fn tell(&mut self, title: &str, text: &str, error: bool) {
        tell(title, text, error);
    }
}

fn pick(hwnd: Option<HWND>, folders: bool, types: &[(&str, &str)]) -> Option<PathBuf> {
    unsafe {
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        if folders {
            let opts = dlg.GetOptions().ok()?;
            dlg.SetOptions(opts | FOS_PICKFOLDERS).ok()?;
        }
        // the strings must outlive SetFileTypes
        let wide: Vec<(HSTRING, HSTRING)> = types.iter().map(|(name, spec)| (HSTRING::from(*name), HSTRING::from(*spec))).collect();
        let specs: Vec<COMDLG_FILTERSPEC> = wide.iter().map(|(n, s)| COMDLG_FILTERSPEC { pszName: PCWSTR(n.as_ptr()), pszSpec: PCWSTR(s.as_ptr()) }).collect();
        if !specs.is_empty() {
            dlg.SetFileTypes(&specs).ok()?;
        }
        dlg.Show(hwnd).ok()?; // Err on cancel
        let item = dlg.GetResult().ok()?;
        let pw = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = pw.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(pw.as_ptr() as *const _));
        path
    }
}

const CF_UNICODETEXT: u32 = 13;

fn clipboard_text() -> Option<String> {
    unsafe {
        OpenClipboard(None).ok()?;
        let text = GetClipboardData(CF_UNICODETEXT).ok().and_then(|handle| {
            let hglobal = HGLOBAL(handle.0);
            let ptr = GlobalLock(hglobal) as *const u16;
            if ptr.is_null() {
                return None;
            }
            let mut len = 0;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            let _ = GlobalUnlock(hglobal);
            Some(s)
        });
        let _ = CloseClipboard();
        text.and_then(|s| one_line(&s))
    }
}

fn set_clipboard_text(text: &str) {
    unsafe {
        if OpenClipboard(None).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
        if let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) {
            let p = GlobalLock(h) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
                let _ = GlobalUnlock(h);
                let _ = SetClipboardData(CF_UNICODETEXT, Some(windows::Win32::Foundation::HANDLE(h.0)));
            }
        }
        let _ = CloseClipboard();
        let _ = PCWSTR::null();
    }
}

fn confirm(title: &str, text: &str) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{IDOK, MB_ICONQUESTION, MB_OKCANCEL, MessageBoxW};
    unsafe { MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(title), MB_OKCANCEL | MB_ICONQUESTION) == IDOK }
}

fn tell(title: &str, text: &str, error: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW};
    unsafe {
        MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(title), MB_OK | if error { MB_ICONERROR } else { MB_ICONINFORMATION });
    }
}
