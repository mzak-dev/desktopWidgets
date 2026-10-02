//! Text, drawn by Skia from what cosmic-text already shaped. `text.rs` still owns
//! shaping, measuring and fonts; this only turns a laid-out buffer into glyphs on a
//! canvas, at the positions glyphon would put them.

use std::collections::HashMap;

use glyphon::cosmic_text::fontdb;
use skia_safe::font::Edging;
use skia_safe::{Canvas, Color, Data, Font, FontHinting, FontMgr, GlyphId, Paint, Point, Typeface};

use crate::draw::TextItem;
use crate::text::TextEngine;

pub struct Glyphs {
    mgr: FontMgr,
    /// One typeface per cosmic-text face, made from the same bytes it loaded.
    faces: HashMap<fontdb::ID, Option<Typeface>>,
}

impl Default for Glyphs {
    fn default() -> Self {
        Self::new()
    }
}

/// A run of consecutive glyphs from one face at one size.
struct Group {
    id: fontdb::ID,
    weight: fontdb::Weight,
    size: f32,
    glyphs: Vec<GlyphId>,
    at: Vec<Point>,
}

impl Glyphs {
    pub fn new() -> Self {
        Self { mgr: FontMgr::new(), faces: HashMap::new() }
    }

    fn typeface(&mut self, fs: &mut glyphon::FontSystem, id: fontdb::ID, weight: fontdb::Weight) -> Option<Typeface> {
        if let Some(t) = self.faces.get(&id) {
            return t.clone();
        }
        let index = fs.db().face(id).map_or(0, |f| f.index);
        let made = fs.get_font(id, weight).and_then(|f| self.mgr.new_from_data(Data::new_copy(f.data()), index as u32));
        if made.is_none() {
            eprintln!("wayfinder: skia: no typeface for font {id:?}");
        }
        self.faces.insert(id, made.clone());
        made
    }

    pub fn draw(&mut self, canvas: &Canvas, it: &TextItem, text: &mut TextEngine) {
        let TextEngine { fs, slots, .. } = text;
        let Some(slot) = slots.get(&it.key) else { return };
        let c = it.color;
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_color(Color::from_argb(c[3], c[0], c[1], c[2]));

        let mut groups: Vec<Group> = Vec::new();
        for run in slot.buf.layout_runs() {
            for g in run.glyphs {
                let size = g.font_size * it.scale;
                // what `LayoutGlyph::physical` plus glyphon's line offset make of it
                let x = it.x + (g.x + g.font_size * g.x_offset) * it.scale;
                let y = (it.y + (g.y - g.font_size * g.y_offset) * it.scale).trunc() + (run.line_y * it.scale).round();
                match groups.last_mut() {
                    Some(last) if last.id == g.font_id && last.weight == g.font_weight && last.size == size => {
                        last.glyphs.push(g.glyph_id);
                        last.at.push(Point::new(x, y));
                    }
                    _ => groups.push(Group { id: g.font_id, weight: g.font_weight, size, glyphs: vec![g.glyph_id], at: vec![Point::new(x, y)] }),
                }
            }
        }
        for g in groups {
            let Some(face) = self.typeface(fs, g.id, g.weight) else { continue };
            let mut font = Font::from_typeface(face, g.size);
            // grayscale coverage on a transparent surface, positioned in fractions of a pixel
            font.set_edging(Edging::AntiAlias).set_subpixel(true).set_hinting(FontHinting::None);
            canvas.draw_glyphs_at(&g.glyphs, g.at.as_slice(), Point::new(0.0, 0.0), &font, &paint);
        }
    }
}
