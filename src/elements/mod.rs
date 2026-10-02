//! One file per element kind: its attributes, its build and, for vector kinds,
//! its `Shape`. Box, text and image are drawn by the engine itself.

mod arc;
mod block;
mod graph;
mod hand;
mod image;
mod kind;
mod text;
mod ticks;

use std::fmt::Debug;

use crate::color::Color;
use crate::draw::Inst;
use crate::theme::Theme;
use crate::value::Value;

pub use self::arc::ArcSpec;
pub use self::block::BlockSpec;
pub use self::graph::GraphSpec;
pub use self::hand::HandSpec;
pub use self::ticks::TicksSpec;

pub use self::kind::{Fit, ImageSpec, Kind};

/// What `Attrs` needs from whoever builds the tree; `format::TreeBuilder` implements it.
pub trait AttrSource<'a> {
    fn theme(&self) -> &'a Theme;
    fn get(&mut self, k: &str) -> Result<Option<Value>, String>;
    fn color(&mut self, k: &str) -> Result<Option<Color>, String>;
    fn image_id(&mut self, src: &str) -> String;
    fn request_image(&mut self, id: &str) -> ((f32, f32), bool);
}

/// Resolves tokens and bindings; bad values become warnings, not errors.
pub struct Attrs<'r, 'a> {
    src: &'r mut dyn AttrSource<'a>,
}

impl<'r, 'a> Attrs<'r, 'a> {
    pub fn new(src: &'r mut dyn AttrSource<'a>) -> Self {
        Self { src }
    }

    pub fn theme(&self) -> &'a Theme {
        self.src.theme()
    }

    pub fn value(&mut self, k: &str) -> Result<Option<Value>, String> {
        self.src.get(k)
    }

    pub fn num(&mut self, k: &str) -> Result<Option<f32>, String> {
        Ok(self.value(k)?.and_then(|v| v.as_f64()).map(|x| x as f32))
    }

    pub fn flag(&mut self, k: &str) -> Result<Option<bool>, String> {
        Ok(self.value(k)?.map(|v| v.truthy()))
    }

    pub fn text(&mut self, k: &str) -> Result<Option<String>, String> {
        Ok(self.value(k)?.map(|v| v.to_string()))
    }

    pub fn color(&mut self, k: &str) -> Result<Option<Color>, String> {
        self.src.color(k)
    }

    /// An image `src`: `./x.png` is a file next to the definition, inside its content root.
    pub fn image_id(&mut self, src: &str) -> String {
        self.src.image_id(src)
    }

    /// Its size, 32x32 until the image is uploaded, and whether it has been.
    pub fn request_image(&mut self, id: &str) -> ((f32, f32), bool) {
        self.src.request_image(id)
    }
}

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
    /// The shape's name and its attributes as text, in a fixed order, for the UI trace. Numbers go
    /// through `num_str` and colours through `Color::to_hex`: `Debug` output is not a stable format.
    fn describe(&self) -> (&'static str, Vec<(&'static str, String)>);
}

/// A number to 0.01, shortest (`12`, `12.5`, `12.25`), never `-0`: the form traces and dumps print.
pub fn num_str(v: f32) -> String {
    let s = format!("{:.2}", if v == 0.0 { 0.0 } else { v });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
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
pub static KINDS: &[ElementKind] = &[BOX, self::text::KIND, self::image::KIND, self::hand::KIND, self::ticks::KIND, self::arc::KIND, self::graph::KIND, self::block::KIND];

pub fn find(name: &str) -> Option<&'static ElementKind> {
    KINDS.iter().find(|k| k.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_print_to_a_hundredth_and_as_short_as_they_can() {
        let got: Vec<String> = [12.0, 12.5, 12.25, 12.254, 0.0, -0.0, -0.001, 1e-4, 63.4, 100.0, -3.5].into_iter().map(num_str).collect();
        assert_eq!(got, ["12", "12.5", "12.25", "12.25", "0", "0", "0", "0", "63.4", "100", "-3.5"]);
    }

    #[test]
    fn kind_names_are_unique() {
        for (i, k) in KINDS.iter().enumerate() {
            assert!(KINDS[i + 1..].iter().all(|o| o.name != k.name), "duplicate kind `{}`", k.name);
            assert!(std::ptr::eq(find(k.name).unwrap(), k));
        }
    }
}
