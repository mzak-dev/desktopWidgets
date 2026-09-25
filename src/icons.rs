//! Icon sourcing (decision 22): explicit path -> Icon Pack by app name -> the
//! target's own icon -> a generic one. Uploaded once per image id.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use windows::Win32::Graphics::Gdi::{BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, ReleaseDC};
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT};
use windows::Win32::UI::Shell::{SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_SYSICONINDEX, SHGetFileInfoW, SHGetImageList, SHIL_EXTRALARGE};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};
use windows::core::HSTRING;

use crate::data::ID_SEP;
use crate::gfx::Gpu;

pub const GENERIC: &str = "icon:generic";

pub struct Rgba {
    pub px: Vec<u8>,
    pub w: u32,
    pub h: u32,
}

fn load_image(p: &Path) -> Option<Rgba> {
    let img = image::open(p).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Some(Rgba { px: img.into_raw(), w, h })
}

/// A neutral rounded tile for anything with no icon at all.
pub fn generic() -> Rgba {
    let n = 48u32;
    let mut px = vec![0u8; (n * n * 4) as usize];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 + 0.5 - 24.0, y as f32 + 0.5 - 24.0);
            let q = (fx.abs() - 19.0 + 8.0, fy.abs() - 19.0 + 8.0);
            let d = (q.0.max(0.0).powi(2) + q.1.max(0.0).powi(2)).sqrt() + q.0.max(q.1).min(0.0) - 8.0;
            let cover = (0.5 - d).clamp(0.0, 1.0);
            let ring = (1.0 - (d + 1.5).abs()).clamp(0.0, 1.0);
            let dot = (0.5 - ((fx * fx + fy * fy).sqrt() - 6.0)).clamp(0.0, 1.0);
            let a = (cover * 0.22 + ring * 0.45 + dot * 0.55).min(1.0);
            let i = ((y * n + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&[235, 240, 255, (a * 255.0) as u8]);
        }
    }
    Rgba { px, w: n, h: n }
}

fn resolve_path(target: &str) -> Option<PathBuf> {
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
fn shell_icon(path: &Path) -> Option<Rgba> {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk")) {
        if let Some((target, icon)) = link_target(path) {
            if let Some(i) = icon.as_deref().and_then(load_image) {
                return Some(i);
            }
            if target.exists() {
                if let Some(i) = shell_icon(&target) {
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

unsafe fn hicon_to_rgba(hicon: HICON) -> Option<Rgba> {
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
                out = Some(Rgba { px, w, h });
            }
        }
        ReleaseDC(None, hdc);
        let _ = DeleteObject(info.hbmColor.into());
        let _ = DeleteObject(info.hbmMask.into());
        out
    }
}

/// Icon Pack lookup: `<pack>/<name>.png` where name is the target's file
/// stem, lowercased (`Chrome.lnk` -> `chrome.png`), or its full file name.
fn from_pack(pack_dir: &Path, target: &str) -> Option<Rgba> {
    let stem = crate::data::file_stem(target).to_lowercase();
    let file = Path::new(target).file_name()?.to_string_lossy().to_lowercase();
    [format!("{stem}.png"), format!("{file}.png")].iter().find_map(|n| load_image(&pack_dir.join(n)))
}

pub fn resolve(target: &str, explicit: &str, pack_dir: Option<&Path>) -> Rgba {
    if !explicit.is_empty() {
        if let Some(i) = load_image(Path::new(explicit)) {
            return i;
        }
    }
    if let Some(i) = pack_dir.and_then(|d| from_pack(d, target)) {
        return i;
    }
    resolve_path(target).and_then(|p| shell_icon(&p)).unwrap_or_else(generic)
}

/// A `file:` image, or an app icon that may come from an Icon Pack.
fn from_files(id: &str) -> bool {
    id.starts_with("file:") || id.starts_with(crate::thumbs::PREFIX) || id.strip_prefix("icon:").and_then(|rest| rest.split_once(ID_SEP)).is_some_and(|(pack, _)| !matches!(pack, "Default" | ""))
}

/// Images no window draws that stay on the GPU, so a gallery flipping between a few
/// pictures doesn't decode them again. Past this, the longest unused go first.
pub const IDLE_BUDGET: u64 = 64 << 20;

/// Which uploaded images no window draws any more, oldest first.
#[derive(Default)]
pub struct Residency {
    idle: Vec<(String, u64)>,
}

impl Residency {
    /// `loaded` is every image on the GPU with its size, `drawn` what some window draws now.
    /// Returns the images to drop.
    pub fn settle<'a>(&mut self, loaded: impl IntoIterator<Item = (&'a str, u64)>, drawn: &HashSet<&str>, budget: u64) -> Vec<String> {
        let loaded: BTreeMap<&str, u64> = loaded.into_iter().collect();
        self.idle.retain(|(id, _)| loaded.contains_key(id.as_str()) && !drawn.contains(id.as_str()));
        for (id, bytes) in &loaded {
            if !drawn.contains(id) && !self.idle.iter().any(|(i, _)| i == id) {
                self.idle.push((id.to_string(), *bytes));
            }
        }
        let mut total: u64 = self.idle.iter().map(|(_, b)| b).sum();
        let mut out = vec![];
        while total > budget && !self.idle.is_empty() {
            let (id, bytes) = self.idle.remove(0);
            total -= bytes;
            out.push(id);
        }
        out
    }

    fn clear(&mut self) {
        self.idle.clear();
    }
}

/// Uploads images on demand and remembers which ids the GPU already has.
#[derive(Default)]
pub struct IconService {
    /// Icon Pack name to its folder, from every content root.
    packs: BTreeMap<String, PathBuf>,
    seen: HashSet<String>,
    residency: Residency,
    /// `thumb:` images, made off the UI thread.
    thumbs: crate::thumbs::Thumbs,
}

impl IconService {
    pub fn new(packs: BTreeMap<String, PathBuf>) -> Self {
        Self { packs, ..Default::default() }
    }

    pub fn set_packs(&mut self, packs: BTreeMap<String, PathBuf>) {
        self.packs = packs;
    }

    /// Where small copies of big pictures are kept (`<data>/.cache/thumbs`).
    pub fn set_cache(&mut self, dir: PathBuf) {
        self.thumbs.set_cache(dir);
    }

    /// Called from another thread when a `thumb:` image is ready: then call `take_ready`.
    pub fn set_waker(&self, wake: std::sync::Arc<dyn Fn() + Send + Sync>) {
        self.thumbs.set_waker(wake);
    }

    /// Uploads the `thumb:` images made since last asked and returns their ids.
    pub fn take_ready(&mut self, gpu: &mut Gpu) -> Vec<String> {
        let ready = self.thumbs.take();
        let mut ids = Vec::with_capacity(ready.len());
        for (id, d) in ready {
            match d {
                Some(d) => gpu.upload_decoded(&id, &d),
                None => {
                    let g = generic();
                    gpu.upload_image(&id, &g.px, g.w, g.h);
                }
            }
            self.seen.insert(id.clone());
            ids.push(id);
        }
        ids
    }

    /// Whether `thumb:` images are still being made.
    pub fn pending(&self) -> bool {
        self.thumbs.has_pending()
    }

    /// Make sure `id` is on the GPU. Returns true when something was uploaded. A `thumb:`
    /// image is only queued here; `take_ready` uploads it.
    pub fn ensure(&mut self, gpu: &mut Gpu, id: &str) -> bool {
        if id.is_empty() || gpu.has_image(id) {
            return false;
        }
        if id.starts_with(crate::thumbs::PREFIX) {
            self.thumbs.request(id);
            return false;
        }
        if !self.seen.insert(id.to_string()) && gpu.has_image(GENERIC) {
            return false; // tried already; fall back to generic below
        }
        let img = if id == GENERIC {
            generic()
        } else if let Some(path) = id.strip_prefix("file:") {
            match crate::images::decode_file(Path::new(path)) {
                Some(d) => {
                    gpu.upload_decoded(id, &d);
                    return true;
                }
                None => generic(),
            }
        } else if let Some(rest) = id.strip_prefix("icon:") {
            let mut it = rest.split(ID_SEP);
            let (pack, target, explicit) = (it.next().unwrap_or(""), it.next().unwrap_or(""), it.next().unwrap_or(""));
            resolve(target, explicit, self.packs.get(pack).map(PathBuf::as_path))
        } else {
            generic()
        };
        gpu.upload_image(id, &img.px, img.w, img.h);
        true
    }

    /// Content was reloaded: images read from files (and Icon Packs) may have changed.
    /// The system's own icons are kept; extracting them again is slow.
    pub fn flush_files(&mut self, gpu: &mut Gpu) {
        let stale: Vec<String> = self.seen.iter().filter(|id| from_files(id)).cloned().collect();
        for id in stale {
            gpu.drop_image(&id);
            self.seen.remove(&id);
        }
    }

    /// Drops images no window has drawn for a while (see `IDLE_BUDGET`). `drawn` is what
    /// every open window draws now; the generic icon always stays.
    pub fn release_unused<'a>(&mut self, gpu: &mut Gpu, drawn: impl IntoIterator<Item = &'a str>) {
        let drawn: HashSet<&str> = drawn.into_iter().collect();
        let IconService { seen, residency, .. } = self;
        let loaded = seen.iter().filter(|id| id.as_str() != GENERIC).filter_map(|id| Some((id.as_str(), gpu.image_bytes(id)?)));
        for id in residency.settle(loaded, &drawn, IDLE_BUDGET) {
            gpu.drop_image(&id);
            self.seen.remove(&id);
        }
    }

    /// The GPU was rebuilt: nothing is uploaded any more.
    pub fn forget(&mut self) {
        self.seen.clear();
        self.residency.clear();
    }

    /// Forget everything (icon pack changed, so ids are new anyway, but this frees the memory).
    pub fn reset(&mut self, gpu: &mut Gpu, ids: impl IntoIterator<Item = String>) {
        for id in ids {
            gpu.drop_image(&id);
        }
        self.seen.clear();
        self.residency.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_file_and_pack_images_are_flushed_on_reload() {
        let icon = |pack: &str| format!("icon:{pack}{ID_SEP}C:\\app.exe{ID_SEP}");
        assert!(from_files("file:C:\\x.png") && from_files("thumb:256:C:\\x.png"));
        assert!(from_files(&icon("Neon")));
        assert!(!from_files(&icon("Default")), "a shell icon is expensive to extract again");
        assert!(!from_files(GENERIC));
    }

    #[test]
    fn unused_images_stay_within_a_budget_then_go_oldest_first() {
        let mut r = Residency::default();
        let set = |ids: &[&'static str]| ids.iter().copied().collect::<HashSet<&str>>();
        let loaded = |ids: &[&'static str]| ids.iter().map(|id| (*id, 10u64)).collect::<Vec<_>>();
        assert!(r.settle(loaded(&["a", "b", "c"]), &set(&["a", "b", "c"]), 25).is_empty(), "all drawn");
        assert!(r.settle(loaded(&["a", "b", "c"]), &set(&["c"]), 25).is_empty(), "a and b idle, 20 bytes fit");
        assert!(r.settle(loaded(&["a", "b", "c", "d"]), &set(&["b", "d"]), 25).is_empty(), "b drawn again, a and c idle");
        assert_eq!(r.settle(loaded(&["a", "b", "c", "d"]), &set(&[]), 30), ["a"], "a idled first, so it goes first");
        assert_eq!(r.settle(loaded(&["b", "c", "d"]), &set(&[]), 0), ["c", "b", "d"], "then in the order they idled");
        assert!(r.settle(loaded(&["e"]), &set(&[]), 25).is_empty() && r.idle.len() == 1, "images no longer loaded are forgotten");
    }

    #[test]
    fn generic_icon_is_a_centred_shape_not_a_blank_square() {
        let g = generic();
        assert_eq!((g.w, g.h, g.px.len()), (48, 48, 48 * 48 * 4));
        let alpha = |x: usize, y: usize| g.px[(y * 48 + x) * 4 + 3];
        assert_eq!(alpha(0, 0), 0, "corner is transparent");
        assert!(alpha(24, 24) > 100, "centre dot is visible");
    }

    #[test]
    fn fallback_chain_reaches_each_step_in_order() {
        let dir = std::env::temp_dir().join(format!("wf-icons-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // step 2: an Icon Pack entry beats the target's own icon
        let mut px = image::RgbaImage::new(8, 8);
        px.put_pixel(0, 0, image::Rgba([1, 2, 3, 255]));
        px.save(dir.join("notepad.png")).unwrap();
        let hit = resolve("C:\\Windows\\notepad.exe", "", Some(&dir));
        assert_eq!((hit.w, hit.px[0]), (8, 1), "pack icon used");
        // step 1: an explicit path beats the pack
        let explicit = dir.join("mine.png");
        image::RgbaImage::new(4, 4).save(&explicit).unwrap();
        assert_eq!(resolve("notepad", explicit.to_str().unwrap(), Some(&dir)).w, 4);
        // step 3: no pack entry -> the shell icon (sized by the system, not 8/4)
        let shell = resolve("C:\\Windows\\notepad.exe", "", None);
        assert!(shell.w >= 32, "shell icon, got {}", shell.w);
        // step 4: nothing resolvable -> generic
        assert_eq!(resolve("definitely-not-a-real-app-xyz", "", None).w, 48);
        std::fs::remove_dir_all(&dir).ok();
    }
}
