//! Element kinds (CONTEXT.md: Element), the building blocks of a widget
//! definition. Each kind owns its attribute names, how it is built from those
//! attributes (`format::Attrs`), and, for vector kinds, how it draws (`Shape`).
//! `format` validates and dispatches through `KINDS`; `ui` draws a `Shape`
//! without knowing which one it is. Box, text and image are core kinds the
//! engine lays out and draws itself; the settings window builds them directly.
//!
//! Adding a kind: a file here with a `KIND` (and a `Shape` impl if it draws),
//! and a line in `KINDS`.

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

/// Where a shape draws: its laid-out rect and the state it inherits.
pub struct ShapeCx {
    /// Centre of the rect, physical px.
    pub center: [f32; 2],
    /// Size of the rect, logical px.
    pub size: (f32, f32),
    /// Physical px per logical px.
    pub scale: f32,
    /// Opacity inherited from ancestors, hover and enter animations.
    pub opacity: f32,
    /// Clip rect, physical px.
    pub clip: [f32; 4],
}

/// A vector element: draws itself as SDF primitives inside its rect.
pub trait Shape: Debug + Send + Sync {
    /// The element type name (`arc`), for tests and diagnostics.
    fn name(&self) -> &'static str;
    fn emit(&self, cx: &ShapeCx, out: &mut Vec<Inst>);
}

/// A colour as shader floats, with an inherited opacity folded into its alpha.
pub fn rgba(c: Color, opacity: f32) -> [f32; 4] {
    let [r, g, b, a] = c.0;
    [r, g, b, a * opacity]
}

/// One element type of the widget format.
pub struct ElementKind {
    /// The `type = "..."` name.
    pub name: &'static str,
    /// Attributes it accepts besides the common layout/look/behaviour ones.
    pub attrs: &'static [&'static str],
    /// Without an explicit width or height it fills its parent (absolutely,
    /// inset 0), the way a clock hand spans the whole face.
    pub fills_parent: bool,
    pub build: fn(&mut Attrs) -> Result<Kind, String>,
}

fn build_box(_: &mut Attrs) -> Result<Kind, String> {
    Ok(Kind::Box)
}

pub const BOX: ElementKind = ElementKind { name: "box", attrs: &[], fills_parent: false, build: build_box };

/// Every element kind, in the order error messages list them.
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
