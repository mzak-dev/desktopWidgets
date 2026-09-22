//! One file per element kind: its attributes, its build and, for vector kinds,
//! its `Shape`. Box, text and image are drawn by the engine itself.

mod arc;
mod hand;
mod image;
mod text;
mod ticks;

use std::fmt::Debug;

use crate::color::Color;
use crate::draw::Inst;
use crate::format::Attrs;
use crate::ui::Kind;

pub use self::arc::ArcSpec;
pub use self::hand::HandSpec;
pub use self::ticks::TicksSpec;

pub struct ShapeCx {
    pub center_px: [f32; 2],
    pub logical_size: (f32, f32),
    pub scale: f32,
    pub inherited_opacity: f32,
    pub clip_px: [f32; 4],
}

pub trait Shape: Debug + Send + Sync {
    fn name(&self) -> &'static str;
    fn emit(&self, cx: &ShapeCx, out: &mut Vec<Inst>);
}

pub fn rgba_with_opacity(c: Color, opacity: f32) -> [f32; 4] {
    let [r, g, b, a] = c.0;
    [r, g, b, a * opacity]
}

pub struct ElementKind {
    pub name: &'static str,
    pub own_attrs: &'static [&'static str],
    /// Like a clock hand spanning the face.
    pub fills_parent_when_unsized: bool,
    pub build: fn(&mut Attrs) -> Result<Kind, String>,
}

fn build_box(_: &mut Attrs) -> Result<Kind, String> {
    Ok(Kind::Box)
}

pub const BOX: ElementKind = ElementKind { name: "box", own_attrs: &[], fills_parent_when_unsized: false, build: build_box };

/// In the order error messages list them.
pub const KINDS: &[ElementKind] = &[BOX, self::text::KIND, self::image::KIND, self::hand::KIND, self::ticks::KIND, self::arc::KIND];

pub fn find(name: &str) -> Option<&'static ElementKind> {
    KINDS.iter().find(|k| k.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_names_are_unique() {
        for (i, k) in KINDS.iter().enumerate() {
            assert!(KINDS[i + 1..].iter().all(|o| o.name != k.name), "duplicate kind `{}`", k.name);
            assert!(std::ptr::eq(find(k.name).unwrap(), k));
        }
    }
}
