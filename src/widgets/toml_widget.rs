use super::{Built, Inputs, Widget, WidgetMeta};
use crate::format::{self, WidgetDef};
use crate::theme::Theme;

pub struct TomlWidget {
    def: WidgetDef,
}

impl TomlWidget {
    pub fn new(def: WidgetDef) -> Self {
        Self { def }
    }

    pub fn def(&self) -> &WidgetDef {
        &self.def
    }
}

impl Widget for TomlWidget {
    fn meta(&self) -> &WidgetMeta {
        &self.def.meta
    }

    fn build(&self, inp: &Inputs, theme: &Theme, image_size: &dyn Fn(&str) -> Option<(f32, f32)>) -> Result<Built, String> {
        format::build(&self.def, inp, theme, image_size)
    }
}
