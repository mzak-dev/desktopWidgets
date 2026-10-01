//! The headless render behind `--render-widget`: a Widget's frame built by the same path as
//! the desktop's (`widgets::prepare`), in a pinned environment.
//!
//! Hermetic by default: a fixed Ambient (research/18, `Pins`), no installed content, an empty
//! temporary data folder, no network, the software adapter. A `real` seam, `--installed` or a
//! hardware adapter opt out of one piece each, and the record beside the PNG
//! (`<png>.env.json`, see `env`) says which. The text is measured with the machine's fonts
//! (there are no bundled ones), so two renders are byte-identical on one machine and font set,
//! not across machines.
//!
//! Time is virtual: the first frame is drawn at T0 and the last at T0 + settle, and every
//! source reads that clock (`SourceCx::now`), so a playing track or a fading animation is
//! where the clock says, not where the machine's speed left it. In between the render blocks,
//! and never draws a half-loaded state: each Code Source the widget reads must have answered,
//! and the Image Store must have nothing pending. A timeout is an error.

pub mod env;
mod world;

use std::path::PathBuf;

use crate::ambient::{Backdrop, Pins, Seam};
use crate::gfx::{Gpu, Power};
use crate::shortcut::Shortcut;

pub use world::{Content, Failure, HOVER_HOLD, Settled, settle};

/// One `--render-widget` run (and, filled in by a scene, one scene's frame).
#[derive(Debug, PartialEq)]
pub struct Request {
    /// A widget id (built-in; with `installed`, a plugin's or the user's too) or a widget file.
    pub widget: String,
    pub png: PathBuf,
    /// The card's size in logical px; else `tier`'s size, else the widget's default.
    pub size: Option<(f32, f32)>,
    /// Render at the size of this Tier.
    pub tier: Option<String>,
    pub params: Vec<(String, serde_json::Value)>,
    pub state: Vec<(String, serde_json::Value)>,
    /// `--time`: the time of day, on the pinned date (or today's with a real clock).
    pub time: Option<(u32, u32)>,
    /// How long to wait for each plugin code source to answer, in seconds.
    pub wait: f32,
    /// `--data`: read only with `installed`; otherwise a render uses an empty temporary folder.
    pub data: Option<PathBuf>,
    /// Look widgets up among the installed plugins and the user's own (not hermetic).
    pub installed: bool,
    /// `--content-root`: a plugin folder whose widgets and code sources load, without
    /// installing it. Its files are hashed into the record, so the run stays hermetic.
    pub plugin: Option<PathBuf>,
    /// Modules to put away at the tier the card size picks.
    pub hide: Vec<String>,
    /// The shortcut list of an icon widget.
    pub items: Option<Vec<Shortcut>>,
    /// A node key held hovered, or a point (logical px) whose node is, on the settled frame.
    pub hover: Option<String>,
    pub hover_at: Option<(f32, f32)>,
    pub gpu: String,
    /// The environment: `--env`, `--palette`, `--scale`, `--transparent`, `--now`, `--real`.
    pub pins: Pins,
}

impl Request {
    pub fn new(widget: String) -> Self {
        Self { widget, png: PathBuf::new(), size: None, tier: None, params: vec![], state: vec![], time: None, wait: 5.0, data: None, installed: false, plugin: None, hide: vec![], items: None, hover: None, hover_at: None, gpu: "software".into(), pins: Pins::default() }
    }
}

/// Renders one widget as the desktop would, in the pinned environment, and writes a PNG and
/// its `.env.json`. Returns whether the widget showed an error.
pub fn render(r: &Request) -> Result<bool, String> {
    let power = Power::parse(&r.gpu).map_err(|e| format!("--gpu: {e}"))?;
    let mut s = settle(r, power, false).map_err(String::from)?;
    let mut gpu = Gpu::new_headless(power)?;
    let adapter = gpu.adapter_report();
    if !adapter.software && !s.real.contains(&Seam::Gpu) {
        return Err(format!("refusing to render on {} ({}): a hermetic render uses the software adapter", adapter.name, adapter.device_type));
    }
    for n in &s.notes {
        println!("{n}");
    }
    let pins = &s.pins;
    let (pw, ph) = ((s.window.0 * pins.scale).round() as u32, (s.window.1 * pins.scale).round() as u32);
    gpu.apply(s.images.drain());
    let mut px = gpu.render_offscreen(pw, ph, &s.prepared.frame.list, &mut s.text, s.at)?;
    // premultiplied: over a backdrop, or back to straight alpha for a transparent PNG
    for (i, c) in px.chunks_exact_mut(4).enumerate() {
        let a = c[3] as f32 / 255.0;
        if pins.transparent {
            if a > 0.0 {
                for k in 0..3 {
                    c[k] = (c[k] as f32 / a).min(255.0) as u8;
                }
            }
        } else {
            let (x, y) = ((i as u32 % pw) as f32 / pw as f32, (i as u32 / pw) as f32 / ph as f32);
            let bg = match pins.backdrop {
                Backdrop::Gradient => [30.0 + 70.0 * x, 60.0 + 50.0 * (1.0 - y), 120.0 + 60.0 * y],
                Backdrop::Solid(rgb) => rgb.map(f32::from),
            };
            for k in 0..3 {
                c[k] = (c[k] as f32 + bg[k] * (1.0 - a)).clamp(0.0, 255.0) as u8;
            }
            c[3] = 255;
        }
    }
    let img = image::RgbaImage::from_raw(pw, ph, px).ok_or("the renderer returned the wrong size")?;
    img.save(&r.png).map_err(|e| format!("{}: {e}", r.png.display()))?;

    let faces = s.text.font_faces();
    let id = &s.content.id;
    let facts = env::Facts { widget: id, size: (pw, ph), pins, real: &s.real, installed: r.installed, roots: &s.content.roots, faces: &faces, adapter: Some(&adapter), deps: &s.prepared.deps, code_sources: &s.code_sources, rounds: s.rounds };
    let because = env::leaks(&facts);
    let side = env::sidecar_path(&r.png);
    let json = serde_json::to_string_pretty(&env::sidecar(&facts)).map_err(|e| e.to_string())?;
    std::fs::write(&side, json + "\n").map_err(|e| format!("{}: {e}", side.display()))?;
    println!("rendered {id} at {:.0}x{:.0} to {}", s.card_size.0, s.card_size.1, r.png.display());
    if because.is_empty() {
        println!("hermetic render; environment recorded in {}", side.display());
    } else {
        println!("not hermetic ({}); environment recorded in {}", because.join("; "), side.display());
    }
    Ok(s.prepared.error.is_some())
}
