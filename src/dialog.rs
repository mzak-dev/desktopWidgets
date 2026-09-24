//! Native file/folder pickers and clipboard text (blocking, on the event-loop
//! thread: they are user-initiated, modal actions).

use std::path::PathBuf;

use windows::Win32::Foundation::{HGLOBAL, HWND};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH};
use windows::core::{HSTRING, PCWSTR};

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

pub fn pick_file(hwnd: Option<HWND>) -> Option<PathBuf> {
    pick(hwnd, false, &[])
}

/// Only files matching one of `(name, "*.a;*.b")`.
pub fn pick_file_of(hwnd: Option<HWND>, types: &[(&str, &str)]) -> Option<PathBuf> {
    pick(hwnd, false, types)
}

pub fn pick_folder(hwnd: Option<HWND>) -> Option<PathBuf> {
    pick(hwnd, true, &[])
}

const CF_UNICODETEXT: u32 = 13;

/// Clipboard text with newlines flattened: every field here is one line.
pub fn clipboard_text() -> Option<String> {
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
        text.map(|s| s.replace(['\r', '\n'], " ").trim().to_string()).filter(|s| !s.is_empty())
    }
}

pub fn set_clipboard_text(text: &str) {
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

/// A modal OK/Cancel question; true on OK.
pub fn confirm(title: &str, text: &str) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{IDOK, MB_ICONQUESTION, MB_OKCANCEL, MessageBoxW};
    unsafe { MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(title), MB_OKCANCEL | MB_ICONQUESTION) == IDOK }
}

/// A modal note; `error` shows the error icon.
pub fn tell(title: &str, text: &str, error: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW};
    unsafe {
        MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(title), MB_OK | if error { MB_ICONERROR } else { MB_ICONINFORMATION });
    }
}
