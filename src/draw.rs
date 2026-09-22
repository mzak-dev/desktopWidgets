//! The renderer's only input: a flat, physical-pixel draw list. Nothing above
//! this sees a wgpu type, so swapping the renderer touches only `gfx`.

use bytemuck::{Pod, Zeroable};

pub const NO_CLIP: [f32; 4] = [-1e6, -1e6, 1e6, 1e6];

pub const KIND_RECT: f32 = 0.0;
pub const KIND_CAPSULE: f32 = 1.0;
pub const KIND_SHADOW: f32 = 2.0;
/// `a` centre, `b` = (start, sweep) radians clockwise from 12, `radius` of the
/// centre line, `border` half the stroke width.
pub const KIND_ARC: f32 = 3.0;

/// One SDF primitive. Rect: `a` centre, `b` half extents. Capsule: `a`,`b` are
/// the end points and `radius` is half the stroke width. Shadow: a soft rect.
#[repr(C)]
#[derive(Clone, Copy, Default, Pod, Zeroable)]
pub struct Inst {
    pub a: [f32; 2],
    pub b: [f32; 2],
    pub radius: f32,
    pub border: f32,
    pub kind: f32,
    pub soft: f32,
    pub fill_top: [f32; 4],
    pub fill_bot: [f32; 4],
    pub border_color: [f32; 4],
    pub clip: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default, Pod, Zeroable)]
pub struct ImgInst {
    pub center: [f32; 2],
    pub half: [f32; 2],
    pub radius: f32,
    pub alpha: f32,
    pub _pad: [f32; 2],
    pub tint: [f32; 4],
    pub clip: [f32; 4],
}

pub struct ImgDraw {
    pub tex: String,
    pub inst: ImgInst,
}

pub struct TextItem {
    /// Key into the `TextEngine`'s shaped buffers.
    pub key: String,
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub color: [u8; 4],
    pub clip: [f32; 4],
}

/// Painter's order inside a layer is shapes, then images, then text.
/// Layer 1 is drawn entirely above layer 0 (popups, edit handles).
#[derive(Default)]
pub struct Layer {
    pub shapes: Vec<Inst>,
    pub images: Vec<ImgDraw>,
    pub texts: Vec<TextItem>,
}

#[derive(Default)]
pub struct DrawList {
    pub layers: [Layer; 2],
}

pub fn intersect(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]).max(a[0].max(b[0])), a[3].min(b[3]).max(a[1].max(b[1]))]
}

#[cfg(test)]
mod tests {
    /// The shader only compiles for real at pipeline creation; catch WGSL errors without a GPU.
    #[test]
    fn shader_is_valid_wgsl() {
        let m = naga::front::wgsl::parse_str(include_str!("shader.wgsl")).expect("shader.wgsl parses");
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&m).expect("shader.wgsl validates");
    }
}
