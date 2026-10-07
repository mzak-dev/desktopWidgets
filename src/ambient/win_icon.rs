//! The real icon source: the Windows shell. A shortcut shows what it opens, a file its
//! type's icon or its own, from the system image list; a bare app name is looked up on `PATH`.

use std::path::{Path, PathBuf};

use windows::Win32::Graphics::Gdi::{BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, ReleaseDC};
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT};
use windows::Win32::UI::Shell::{SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_SYSICONINDEX, SHGetFileInfoW, SHGetImageList, SHIL_EXTRALARGE};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};
use windows::core::HSTRING;

use super::icon::IconSource;
use crate::images::Decoded;

pub struct ShellIcons;

impl IconSource for ShellIcons {
    fn shell_icon(&self, path: &Path) -> Option<Decoded> {
        shell(path)
    }

    fn resolve_path(&self, target: &str) -> Option<PathBuf> {
        find(target)
    }
}

fn find(target: &str) -> Option<PathBuf> {
    if target.contains("://") {
        return None;
    }
    let p = Path::new(target);
    if p.exists() {
        return Some(p.to_path_buf());
    }
    if p.components().count() == 1 {
        let name = if p.extension().is_some() { target.to_string() } else { format!("{target}.exe") };
        for dir in std::env::split_paths(&std::env::var_os("PATH")?) {
            let c = dir.join(&name);
            if c.exists() {
                return Some(c);
            }
        }
    }
    None
}

/// Where a `.lnk` points, and the file it takes its icon from if it names one.
fn link_target(lnk: &Path) -> Option<(PathBuf, Option<PathBuf>)> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink, SLR_NO_UI};
    use windows::core::Interface;
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        link.cast::<IPersistFile>().ok()?.Load(&HSTRING::from(lnk.as_os_str()), STGM_READ).ok()?;
        let _ = link.Resolve(windows::Win32::Foundation::HWND::default(), SLR_NO_UI.0 as u32);
        let mut buf = [0u16; 520];
        link.GetPath(&mut buf, std::ptr::null_mut(), 0).ok()?;
        let path = String::from_utf16_lossy(&buf[..buf.iter().position(|c| *c == 0).unwrap_or(buf.len())]);
        let mut icon = [0u16; 520];
        let mut idx = 0;
        let _ = link.GetIconLocation(&mut icon, &mut idx);
        let icon = String::from_utf16_lossy(&icon[..icon.iter().position(|c| *c == 0).unwrap_or(icon.len())]);
        // an icon file is used only when it is a plain image; `.exe` and `.dll` icons go through the shell
        let icon = (!icon.is_empty() && idx == 0).then(|| PathBuf::from(icon)).filter(|p| p.exists());
        (!path.is_empty()).then(|| (PathBuf::from(path), icon))
    }
}

/// 48px via the system image list, falling back to the classic 32px icon. A shortcut shows
/// what it opens, so a `.lnk` to a document or folder has that icon, not a blank page.
fn shell(path: &Path) -> Option<Decoded> {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk")) {
        if let Some((target, icon)) = link_target(path) {
            if let Some(i) = icon.as_deref().and_then(crate::images::load_image) {
                return Some(i);
            }
            if target.exists() {
                if let Some(i) = shell(&target) {
                    return Some(i);
                }
            }
        }
    }
    unsafe {
        let wide = HSTRING::from(path.as_os_str());
        let mut sfi = SHFILEINFOW::default();
        let size = size_of::<SHFILEINFOW>() as u32;
        if SHGetFileInfoW(&wide, FILE_FLAGS_AND_ATTRIBUTES(0), Some(&mut sfi), size, SHGFI_SYSICONINDEX) != 0 {
            if let Ok(list) = SHGetImageList::<IImageList>(SHIL_EXTRALARGE as i32) {
                if let Ok(h) = list.GetIcon(sfi.iIcon, ILD_TRANSPARENT.0 as u32) {
                    let out = hicon_to_rgba(h);
                    let _ = DestroyIcon(h);
                    if out.is_some() {
                        return out;
                    }
                }
            }
        }
        let mut sfi = SHFILEINFOW::default();
        if SHGetFileInfoW(&wide, FILE_FLAGS_AND_ATTRIBUTES(0), Some(&mut sfi), size, SHGFI_ICON | SHGFI_LARGEICON) == 0 || sfi.hIcon.is_invalid() {
            return None;
        }
        let out = hicon_to_rgba(sfi.hIcon);
        let _ = DestroyIcon(sfi.hIcon);
        out
    }
}

unsafe fn hicon_to_rgba(hicon: HICON) -> Option<Decoded> {
    unsafe {
        let mut info = ICONINFO::default();
        GetIconInfo(hicon, &mut info).ok()?;
        let hdc = GetDC(None);
        let mut probe = BITMAPINFO::default();
        probe.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        GetDIBits(hdc, info.hbmColor, 0, 0, None, &mut probe, DIB_RGB_COLORS);
        let (w, h) = (probe.bmiHeader.biWidth.unsigned_abs(), probe.bmiHeader.biHeight.unsigned_abs());
        let mut out = None;
        if w > 0 && h > 0 && w <= 512 && h <= 512 {
            let mut bmi = BITMAPINFO::default();
            bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w as i32;
            bmi.bmiHeader.biHeight = -(h as i32); // top-down
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = BI_RGB.0;
            let mut px = vec![0u8; (w * h * 4) as usize];
            if GetDIBits(hdc, info.hbmColor, 0, h, Some(px.as_mut_ptr().cast()), &mut bmi, DIB_RGB_COLORS) != 0 {
                for p in px.chunks_exact_mut(4) {
                    p.swap(0, 2); // BGRA -> RGBA
                }
                if px.chunks_exact(4).all(|p| p[3] == 0) {
                    px.chunks_exact_mut(4).for_each(|p| p[3] = 255); // old icons without alpha
                }
                out = Some(Decoded { px, w, h, frames: None });
            }
        }
        ReleaseDC(None, hdc);
        let _ = DeleteObject(info.hbmColor.into());
        let _ = DeleteObject(info.hbmMask.into());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shell_gives_notepad_an_icon_and_the_path_finds_it() {
        let notepad = ShellIcons.resolve_path(r"C:\Windows\notepad.exe").expect("notepad exists");
        let icon = ShellIcons.shell_icon(&notepad).expect("the shell has an icon for it");
        assert!(icon.w >= 32, "sized by the system, got {}", icon.w);
        assert!(ShellIcons.resolve_path("definitely-not-a-real-app-xyz").is_none());
        assert!(ShellIcons.resolve_path("https://example.com/a.exe").is_none(), "a URL is not a file");
    }
}
