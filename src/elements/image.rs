use super::ElementKind;
use crate::format::Attrs;
use crate::ui::{ImageSpec, Kind};

pub const KIND: ElementKind = ElementKind { name: "image", own_attrs: &["src", "tint"], fills_parent_when_unsized: false, build };

fn build(a: &mut Attrs) -> Result<Kind, String> {
    let id = a.text("src")?.unwrap_or_default();
    let (w, h) = a.request_image(&id);
    Ok(Kind::Image(ImageSpec { id, w, h, tint: a.color("tint")? }))
}
