use std::sync::Arc;

use super::Shape;
use crate::color::Color;
use crate::textspec::TextSpec;

#[derive(Clone, Debug)]
pub enum Kind {
    Box,
    Text(TextSpec),
    Image(ImageSpec),
    Shape(Arc<dyn Shape>),
}

impl Kind {
    pub fn shape(s: impl Shape + 'static) -> Kind {
        Kind::Shape(Arc::new(s))
    }
}

#[derive(Clone, Debug, Default)]
pub struct ImageSpec {
    pub id: String,
    /// Intrinsic size, used when the style sets none.
    pub w: f32,
    pub h: f32,
    pub tint: Option<Color>,
    /// An animation plays (`anim = false` freezes it on its first frame).
    pub play: bool,
    /// Shows this frame of an animation instead of playing it.
    pub frame: Option<u32>,
    pub fit: Fit,
    /// Logical px over which the edge fades to transparent, inside the rounded shape.
    pub feather: f32,
    /// Ms over which a new `id` fades in over the old one; 0 swaps at once.
    pub fade: u32,
    /// The picture has loaded; until then a fade keeps showing the old one.
    pub ready: bool,
}

/// How an image fills a box of another shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fit {
    /// All of it shows, letter-boxed.
    #[default]
    Contain,
    /// It fills the box, cropped around its centre.
    Cover,
}

impl Fit {
    pub fn parse(s: &str) -> Option<Fit> {
        match s {
            "contain" => Some(Fit::Contain),
            "cover" => Some(Fit::Cover),
            _ => None,
        }
    }

    /// The drawn size in the box `w` x `h`, and the part of the image shown, `[u0, v0, u1, v1]`.
    pub fn place(self, img: (f32, f32), w: f32, h: f32) -> ((f32, f32), [f32; 4]) {
        let (iw, ih) = (img.0.max(1.0), img.1.max(1.0));
        match self {
            Fit::Contain => {
                let k = (w / iw).min(h / ih);
                ((iw * k, ih * k), [0.0, 0.0, 1.0, 1.0])
            }
            Fit::Cover => {
                let k = (w / iw).max(h / ih);
                let (fx, fy) = ((w / (iw * k)).min(1.0), (h / (ih * k)).min(1.0));
                ((w, h), [0.5 - fx / 2.0, 0.5 - fy / 2.0, 0.5 + fx / 2.0, 0.5 + fy / 2.0])
            }
        }
    }
}

