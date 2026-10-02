//! What a text node asks for: pure data, no shaping. `text::TextEngine` turns it into glyphs.

#[derive(Clone, Debug, PartialEq)]
pub struct TextSpec {
    pub text: String,
    pub size: f32,
    pub family: String,
    pub weight: u16,
    pub align: TextAlign,
    pub wrap: bool,
    pub line_height: f32,
    pub color: crate::color::Color,
    /// Caret byte offset, drawn by the UI layer for focused inputs.
    pub caret: Option<usize>,
}

impl Default for TextSpec {
    fn default() -> Self {
        Self {
            text: String::new(),
            size: 14.0,
            family: String::new(),
            weight: 400,
            align: TextAlign::Left,
            wrap: false,
            line_height: 1.25,
            color: crate::color::Color([1.0; 4]),
            caret: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

impl TextAlign {
    pub fn parse(s: &str) -> Self {
        match s {
            "center" => Self::Center,
            "right" | "end" => Self::Right,
            _ => Self::Left,
        }
    }
}
