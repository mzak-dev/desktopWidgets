//! `image`: an uploaded texture (an app icon, a picture). Laid out and drawn
//! by the engine (`ui`, `gfx`); this module owns its attributes.

use super::ElementKind;
use crate::format::Attrs;
use crate::ui::{ImageSpec, Kind};

pub const KIND: ElementKind = ElementKind { name: "image", attrs: &["src", "tint"], fills_parent: false, build };

fn build(a: &mut Attrs) -> Result<Kind, String> {
    let id = a.text("src")?.unwrap_or_default();
    let (w, h) = a.image(&id);
    Ok(Kind::Image(ImageSpec { id, w, h, tint: a.color("tint")? }))
}
