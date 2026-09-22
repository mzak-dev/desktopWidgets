use super::ElementKind;
use crate::format::Attrs;
use crate::text::{TextAlign, TextSpec};
use crate::ui::Kind;
use crate::value::Value;

pub const KIND: ElementKind = ElementKind { name: "text", own_attrs: &["text", "size", "color", "font", "weight", "text_align", "text_wrap", "line_height"], fills_parent_when_unsized: false, build };

fn weight(v: &Value) -> u16 {
    match v {
        Value::Num(n) => *n as u16,
        Value::Str(s) => match s.as_str() {
            "thin" => 100,
            "light" => 300,
            "medium" => 500,
            "semibold" => 600,
            "bold" => 700,
            "black" => 900,
            _ => 400,
        },
        _ => 400,
    }
}

fn build(a: &mut Attrs) -> Result<Kind, String> {
    let theme = a.theme();
    let mut spec = TextSpec { size: theme.num("font-size-md"), color: theme.color("text"), family: theme.str("font-body"), ..Default::default() };
    spec.text = a.text("text")?.unwrap_or_default();
    if let Some(s) = a.num("size")? {
        spec.size = s.max(1.0);
    }
    if let Some(c) = a.color("color")? {
        spec.color = c;
    }
    if let Some(f) = a.text("font")? {
        spec.family = f;
    }
    if let Some(w) = a.value("weight")? {
        spec.weight = weight(&w);
    }
    if let Some(al) = a.text("text_align")? {
        spec.align = TextAlign::parse(&al);
    }
    spec.wrap = a.flag("text_wrap")?.unwrap_or(false);
    if let Some(l) = a.num("line_height")? {
        spec.line_height = l;
    }
    Ok(Kind::Text(spec))
}
