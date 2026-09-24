use super::ElementKind;
use crate::format::Attrs;
use crate::ui::{ImageSpec, Kind};

pub const KIND: ElementKind = ElementKind { name: "image", own_attrs: &["src", "tint", "anim", "frame"], fills_parent_when_unsized: false, build };

fn build(a: &mut Attrs) -> Result<Kind, String> {
    let src = a.text("src")?.unwrap_or_default();
    let id = a.image_id(&src);
    let (w, h) = a.request_image(&id);
    let play = a.flag("anim")?.unwrap_or(true);
    let frame = a.num("frame")?.map(|f| f.max(0.0) as u32);
    Ok(Kind::Image(ImageSpec { id, w, h, tint: a.color("tint")?, play, frame }))
}
