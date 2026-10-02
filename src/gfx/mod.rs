//! The renderer seam: a `DrawList` goes in, pixels come out. Which backend draws
//! them is decided here and nowhere above (ADR-0013). `Gpu` and `Target` are
//! enums over the backends; a `Target` only works with the `Gpu` that made it.

#[cfg(feature = "skia")]
mod skia;
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
    /// Only in a build with the `skia` feature.
    #[cfg(feature = "skia")]
    Skia,
}

impl Backend {
    /// The renderer a saved or typed name means; `wgpu` for anything this build lacks.
    pub fn parse(s: &str) -> Backend {
        match s.trim().to_ascii_lowercase().as_str() {
            #[cfg(feature = "skia")]
            "skia" => Backend::Skia,
            _ => Backend::Wgpu,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Backend::Wgpu => "wgpu",
            #[cfg(feature = "skia")]
            Backend::Skia => "skia",
        }
    }

    /// The renderers this build has, for Settings to offer.
    pub fn available() -> &'static [Backend] {
        &[
            Backend::Wgpu,
            #[cfg(feature = "skia")]
            Backend::Skia,
        ]
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
    #[cfg(feature = "skia")]
    Skia(skia::Gpu),
}

pub enum Target {
    Wgpu(wgpu::Target),
    #[cfg(feature = "skia")]
    Skia(skia::Target),
}

/// Runs `$e` on whichever backend `$self` holds.
macro_rules! on_gpu {
    ($self:expr, $g:ident => $e:expr) => {
        match $self {
            Gpu::Wgpu($g) => $e,
            #[cfg(feature = "skia")]
            Gpu::Skia($g) => $e,
        }
    };
}

impl Gpu {
    /// `window`'s surface picks a compatible adapter.
    pub fn new(window: &Arc<Window>, power: Power, backend: Backend) -> Result<(Gpu, Target), String> {
        match backend {
            Backend::Wgpu => wgpu::Gpu::new(window, power).map(|(g, t)| (Gpu::Wgpu(g), Target::Wgpu(t))),
            #[cfg(feature = "skia")]
            Backend::Skia => skia::Gpu::new(window, power).map(|(g, t)| (Gpu::Skia(g), Target::Skia(t))),
        }
    }

    pub fn new_headless(power: Power, backend: Backend) -> Result<Gpu, String> {
        match backend {
            Backend::Wgpu => wgpu::Gpu::new_headless(power).map(Gpu::Wgpu),
            #[cfg(feature = "skia")]
            Backend::Skia => skia::Gpu::new_headless(power).map(Gpu::Skia),
        }
    }

    pub fn backend(&self) -> Backend {
        match self {
            Gpu::Wgpu(_) => Backend::Wgpu,
            #[cfg(feature = "skia")]
            Gpu::Skia(_) => Backend::Skia,
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
            #[cfg(feature = "skia")]
            Gpu::Skia(g) => g.target_for(window).map(Target::Skia),
        }
    }

    /// Reconfigures only when the window outgrows its swapchain, or has been far smaller for a while.
    pub fn fit(&self, t: &mut Target, w: u32, h: u32) {
        match (self, t) {
            (Gpu::Wgpu(g), Target::Wgpu(t)) => g.fit(t, w, h),
            #[cfg(feature = "skia")]
            (Gpu::Skia(g), Target::Skia(t)) => g.fit(t, w, h),
            #[cfg(feature = "skia")]
            _ => {}
        }
    }

    pub fn render(&mut self, t: &mut Target, list: &DrawList, text: &mut TextEngine) -> Result<(), RenderError> {
        match (self, t) {
            (Gpu::Wgpu(g), Target::Wgpu(t)) => g.render(t, list, text),
            #[cfg(feature = "skia")]
            (Gpu::Skia(g), Target::Skia(t)) => g.render(t, list, text),
            #[cfg(feature = "skia")]
            _ => Err(RenderError::Skip("the window's target belongs to another renderer".into())),
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
            #[cfg(feature = "skia")]
            Target::Skia(t) => t.size(),
        }
    }

    /// The window's size the target was last fitted to.
    pub fn view(&self) -> (u32, u32) {
        match self {
            Target::Wgpu(t) => t.view(),
            #[cfg(feature = "skia")]
            Target::Skia(t) => t.view(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_renderer_name_means_wgpu_unless_this_build_has_what_it_names() {
        assert_eq!(Backend::default(), Backend::Wgpu);
        for s in ["", "wgpu", "WGPU", " wgpu ", "vulkan", "nonsense"] {
            assert_eq!(Backend::parse(s), Backend::Wgpu, "{s:?}");
        }
        let skia = Backend::parse("Skia");
        assert_eq!(skia == Backend::Wgpu, !cfg!(feature = "skia"), "skia only where it is built");
    }

    #[test]
    fn every_available_renderer_round_trips_through_its_name() {
        assert_eq!(Backend::available()[0], Backend::Wgpu, "wgpu is listed first");
        for b in Backend::available() {
            assert_eq!(Backend::parse(b.name()), *b);
        }
        assert_eq!(Backend::available().len(), if cfg!(feature = "skia") { 2 } else { 1 });
    }

    #[test]
    fn power_words_keep_their_meaning() {
        assert_eq!(Power::parse("high"), Power::High);
        assert_eq!(Power::parse("WARP"), Power::Software);
        assert_eq!(Power::parse("anything else"), Power::Low);
    }
}
