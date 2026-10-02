//! The Skia renderer (ADR-0013): Skia draws a `DrawList` on the CPU into a pixel
//! buffer, and `present` shows it in the window through DirectComposition. A window's
//! swapchain is sized in buckets (`sizing`, ADR-0006) and only the visible part of
//! the buffer is drawn and uploaded.

mod glyphs;
mod paint;
mod present;
mod sizing;

use std::cell::Cell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use skia_safe::{AlphaType, Color, ColorType, ImageInfo, surfaces};
use winit::window::Window;

use super::{Power, RenderError};
use crate::draw::DrawList;
use crate::platform::win32;
use crate::text::TextEngine;
use paint::{Images, Painter, SkImg};
use sizing::{MAX_SIDE, bucketed, plan};

pub struct Gpu {
    /// `adapter / Skia / device type / alpha … / present …`, as the wgpu renderer words it.
    pub info: String,
    /// How many times a swapchain was resized.
    pub reconfigure_count: Cell<u32>,
    /// What presents windows; none when rendering offscreen.
    device: Option<present::Device>,
    images: Images,
    painter: Painter,
    /// Animations play against this clock.
    epoch: Instant,
}

pub struct Target {
    swap: Option<present::Swap>,
    window: Option<Arc<Window>>,
    /// The window's size when last fitted.
    view: (u32, u32),
    /// The swapchain (and `buf`) is at least `view`.
    size: (u32, u32),
    /// Premultiplied BGRA8, `size.0 * 4` bytes a row.
    buf: Vec<u8>,
    failures: u32,
    last_configure: Instant,
}

impl Target {
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn view(&self) -> (u32, u32) {
        self.view
    }
}

/// Puts the swapchain right after a failure by making a fresh one. The old swapchain
/// and its visual go first, as a window takes one DirectComposition target.
fn rebuild(device: &present::Device, t: &mut Target) -> Result<(), String> {
    let hwnd = t.window.as_deref().and_then(win32::hwnd_of).ok_or("no window to rebuild for")?;
    t.swap = None;
    t.swap = Some(device.swap(hwnd, t.size.0, t.size.1)?);
    Ok(())
}

impl Gpu {
    fn build(device: Option<present::Device>, info: String) -> Gpu {
        Gpu { info, reconfigure_count: Cell::new(0), device, images: Images::new(), painter: Painter::new(), epoch: Instant::now() }
    }

    /// `window` is where the first target goes. `Power` picks the adapter that presents;
    /// the drawing itself is on the CPU either way.
    pub fn new(window: &Arc<Window>, power: Power) -> Result<(Gpu, Target), String> {
        let device = present::Device::new(power)?;
        // "Cpu" is what Settings warns about: nothing but WARP is presenting
        let kind = if device.software { "Cpu" } else { "CPU raster" };
        let info = format!("{} / Skia / {kind} / alpha PreMultiplied / present Flip", device.adapter);
        let mut gpu = Gpu::build(Some(device), info);
        let target = gpu.target_for(window)?;
        Ok((gpu, target))
    }

    /// For offscreen renders: nothing is presented, so there is no adapter to pick.
    pub fn new_headless(_power: Power) -> Result<Gpu, String> {
        Ok(Gpu::build(None, "Offscreen / Skia / CPU raster".into()))
    }

    pub fn is_lost(&self) -> bool {
        self.device.as_ref().is_some_and(|d| d.removed())
    }

    pub fn target_for(&mut self, window: &Arc<Window>) -> Result<Target, String> {
        let device = self.device.as_ref().ok_or("an offscreen renderer has no windows")?;
        let hwnd = win32::hwnd_of(window).ok_or("the window has no handle")?;
        let s = window.inner_size();
        let view = (s.width.max(1), s.height.max(1));
        let size = (bucketed(view.0), bucketed(view.1));
        let swap = device.swap(hwnd, size.0, size.1)?;
        Ok(Target { swap: Some(swap), window: Some(window.clone()), view, size, buf: Vec::new(), failures: 0, last_configure: Instant::now() })
    }

    /// Reconfigures only when the window outgrows the swapchain, or has been far smaller for a while.
    pub fn fit(&self, t: &mut Target, w: u32, h: u32) {
        t.view = (w.max(1), h.max(1));
        let Some(size) = plan(t.size, t.view, t.last_configure.elapsed()) else { return };
        let Some(device) = &self.device else { return };
        let size = (size.0.clamp(1, MAX_SIDE), size.1.clamp(1, MAX_SIDE));
        t.last_configure = Instant::now();
        self.reconfigure_count.set(self.reconfigure_count.get() + 1);
        let resized = match &t.swap {
            Some(swap) => device.resize(swap, size.0, size.1),
            None => Ok(()),
        };
        match resized {
            Ok(()) => t.size = size,
            Err(e) => {
                eprintln!("wayfinder: {e}");
                t.failures += 1;
                t.size = size;
                if rebuild(device, t).is_ok() {
                    t.failures = 0;
                }
            }
        }
    }

    pub fn render(&mut self, t: &mut Target, list: &DrawList, text: &mut TextEngine) -> Result<(), RenderError> {
        let Gpu { device, images, painter, epoch, .. } = self;
        let Some(device) = device else { return Err(RenderError::Skip("an offscreen renderer has no windows".into())) };
        if device.removed() {
            return Err(RenderError::Lost("the presenting device was removed".into()));
        }
        if t.swap.is_none() {
            if let Err(e) = rebuild(device, t) {
                t.failures += 1;
                return Err(if t.failures >= 3 { RenderError::Lost(e) } else { RenderError::Skip(e) });
            }
        }

        let (w, h) = (t.view.0.min(t.size.0), t.view.1.min(t.size.1));
        let stride = t.size.0 as usize * 4;
        let want = stride * t.size.1 as usize;
        if t.buf.len() != want {
            t.buf = vec![0; want];
        }
        let info = ImageInfo::new((w as i32, h as i32), ColorType::BGRA8888, AlphaType::Premul, None);
        {
            let Some(mut surface) = surfaces::wrap_pixels(&info, &mut t.buf[..stride * h as usize], stride, None) else {
                return Err(RenderError::Skip("could not wrap the pixel buffer".into()));
            };
            let canvas = surface.canvas();
            canvas.clear(Color::TRANSPARENT);
            painter.paint(canvas, list, text, images, epoch.elapsed().as_millis() as u64);
        }
        let Some(swap) = &t.swap else { return Err(RenderError::Skip("no swapchain".into())) };
        match device.present(swap, &t.buf, stride as u32, w, h) {
            Ok(()) => {
                t.failures = 0;
                Ok(())
            }
            Err(present::PresentError::Lost(e)) => Err(RenderError::Lost(e)),
            Err(present::PresentError::Failed(e)) => {
                t.failures += 1;
                Err(if t.failures >= 3 { RenderError::Lost(e) } else { RenderError::Skip(e) })
            }
        }
    }

    /// Premultiplied RGBA8, `w * h * 4` bytes.
    pub fn render_offscreen(&mut self, w: u32, h: u32, list: &DrawList, text: &mut TextEngine) -> Result<Vec<u8>, String> {
        let info = ImageInfo::new((w as i32, h as i32), ColorType::RGBA8888, AlphaType::Premul, None);
        let mut surface = surfaces::raster(&info, None, None).ok_or("could not make a raster surface")?;
        surface.canvas().clear(Color::TRANSPARENT);
        self.painter.paint(surface.canvas(), list, text, &self.images, self.epoch.elapsed().as_millis() as u64);
        let mut out = vec![0u8; w as usize * h as usize * 4];
        if !surface.read_pixels(&info, &mut out, w as usize * 4, (0, 0)) {
            return Err("could not read the pixels back".into());
        }
        Ok(out)
    }

    pub fn has_image(&self, id: &str) -> bool {
        self.images.contains_key(id)
    }

    pub fn image_size(&self, id: &str) -> Option<(u32, u32)> {
        self.images.get(id).map(|i| (i.w, i.h))
    }

    pub fn image_bytes(&self, id: &str) -> Option<u64> {
        self.images.get(id).map(|i| i.bytes)
    }

    /// Upload straight-alpha RGBA8.
    pub fn upload_image(&mut self, id: &str, rgba: &[u8], w: u32, h: u32) {
        self.upload(id, rgba, w, h, None);
    }

    /// A decoded file: a still, or an animation's packed frames.
    pub fn upload_decoded(&mut self, id: &str, d: &crate::images::Decoded) {
        self.upload(id, &d.px, d.w, d.h, d.frames.clone());
    }

    fn upload(&mut self, id: &str, rgba: &[u8], w: u32, h: u32, frames: Option<crate::images::Frames>) {
        match SkImg::new(rgba, w, h, frames) {
            Some(img) => {
                self.images.insert(id.to_string(), img);
            }
            None => eprintln!("wayfinder: skia: could not upload image {id} ({w}x{h})"),
        }
    }

    pub fn drop_image(&mut self, id: &str) {
        self.images.remove(id);
    }

    /// How soon a playing animation in `list` shows its next frame.
    pub fn animation_delay(&self, list: &DrawList) -> Option<Duration> {
        let playing = list.layers.iter().flat_map(|l| &l.images).filter(|d| d.play && d.frame.is_none());
        let ms = playing.filter_map(|d| self.images.get(&d.tex)?.frames.as_ref()?.delays_ms.iter().min().copied()).min()?;
        Some(Duration::from_millis(ms.max(16) as u64))
    }
}
