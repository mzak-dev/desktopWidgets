//! The renderer seam: a `DrawList` goes in, pixels come out. Which backend draws
//! them is decided here and nowhere above (ADR-0013). `Gpu` and `Target` are
//! enums over the backends; a `Target` only works with the `Gpu` that made it.

mod wgpu;

use std::sync::Arc;
use std::time::Duration;

use winit::window::Window;

use crate::draw::DrawList;
use crate::text::TextEngine;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power {
    High,
    Low,
    /// Never touches the vendor driver, so automated tests use it.
    Software,
}

impl Power {
    /// Low unless asked: High can pin a core (ADR-005).
    pub fn parse(s: &str) -> Power {
        match s.to_ascii_lowercase().as_str() {
            "high" => Power::High,
            "software" | "warp" | "cpu" => Power::Software,
            _ => Power::Low,
        }
    }
}

/// Which renderer draws the windows. Saved as `renderer` in workspace.json.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Backend {
    #[default]
    Wgpu,
}

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Backend::Wgpu => "wgpu",
        }
    }
}

#[derive(Debug)]
pub enum RenderError {
    /// Nothing is wrong with the device.
    Skip(String),
    Lost(String),
}

pub enum Gpu {
    Wgpu(wgpu::Gpu),
}

pub enum Target {
    Wgpu(wgpu::Target),
}

/// Runs `$e` on whichever backend `$self` holds.
macro_rules! on_gpu {
    ($self:expr, $g:ident => $e:expr) => {
        match $self {
            Gpu::Wgpu($g) => $e,
        }
    };
}

impl Gpu {
    /// `window`'s surface picks a compatible adapter.
    pub fn new(window: &Arc<Window>, power: Power, backend: Backend) -> Result<(Gpu, Target), String> {
        match backend {
            Backend::Wgpu => wgpu::Gpu::new(window, power).map(|(g, t)| (Gpu::Wgpu(g), Target::Wgpu(t))),
        }
    }

    pub fn new_headless(power: Power, backend: Backend) -> Result<Gpu, String> {
        match backend {
            Backend::Wgpu => wgpu::Gpu::new_headless(power).map(Gpu::Wgpu),
        }
    }

    pub fn backend(&self) -> Backend {
        match self {
            Gpu::Wgpu(_) => Backend::Wgpu,
        }
    }

    /// What was picked, for the log and Settings: `name / backend / device type / ...`.
    pub fn info(&self) -> &str {
        on_gpu!(self, g => &g.info)
    }

    pub fn is_lost(&self) -> bool {
        on_gpu!(self, g => g.is_lost())
    }

    /// How many times a window's swapchain was reconfigured (the selftest bounds it).
    pub fn reconfigure_count(&self) -> u32 {
        on_gpu!(self, g => g.reconfigure_count.get())
    }

    pub fn target_for(&mut self, window: &Arc<Window>) -> Result<Target, String> {
        match self {
            Gpu::Wgpu(g) => g.target_for(window).map(Target::Wgpu),
        }
    }

    /// Reconfigures only when the window outgrows its swapchain, or has been far smaller for a while.
    pub fn fit(&self, t: &mut Target, w: u32, h: u32) {
        match (self, t) {
            (Gpu::Wgpu(g), Target::Wgpu(t)) => g.fit(t, w, h),
        }
    }

    pub fn render(&mut self, t: &mut Target, list: &DrawList, text: &mut TextEngine) -> Result<(), RenderError> {
        match (self, t) {
            (Gpu::Wgpu(g), Target::Wgpu(t)) => g.render(t, list, text),
        }
    }

    /// Premultiplied RGBA8, `w * h * 4` bytes.
    pub fn render_offscreen(&mut self, w: u32, h: u32, list: &DrawList, text: &mut TextEngine) -> Result<Vec<u8>, String> {
        on_gpu!(self, g => g.render_offscreen(w, h, list, text))
    }

    pub fn has_image(&self, id: &str) -> bool {
        on_gpu!(self, g => g.has_image(id))
    }

    pub fn image_size(&self, id: &str) -> Option<(u32, u32)> {
        on_gpu!(self, g => g.image_size(id))
    }

    pub fn image_bytes(&self, id: &str) -> Option<u64> {
        on_gpu!(self, g => g.image_bytes(id))
    }

    /// Upload straight-alpha RGBA8.
    pub fn upload_image(&mut self, id: &str, rgba: &[u8], w: u32, h: u32) {
        on_gpu!(self, g => g.upload_image(id, rgba, w, h))
    }

    /// A decoded file: a still, or an animation's packed frames.
    pub fn upload_decoded(&mut self, id: &str, d: &crate::images::Decoded) {
        on_gpu!(self, g => g.upload_decoded(id, d))
    }

    pub fn drop_image(&mut self, id: &str) {
        on_gpu!(self, g => g.drop_image(id))
    }

    /// How soon a playing animation in `list` shows its next frame.
    pub fn animation_delay(&self, list: &DrawList) -> Option<Duration> {
        on_gpu!(self, g => g.animation_delay(list))
    }
}

impl Target {
    /// The swapchain's size, which may exceed the window's (ADR-0006).
    pub fn size(&self) -> (u32, u32) {
        match self {
            Target::Wgpu(t) => t.size(),
        }
    }

    /// The window's size the target was last fitted to.
    pub fn view(&self) -> (u32, u32) {
        match self {
            Target::Wgpu(t) => t.view(),
        }
    }
}
