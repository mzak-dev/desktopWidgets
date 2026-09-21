//! Text shaping and measurement over glyphon/cosmic-text (decision 25). One
//! shaped buffer per node key; re-shaped only when its text, style or width
//! changes, so a live resize only re-wraps.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use glyphon::cosmic_text::{Align, Wrap};
use glyphon::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache, Weight};

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

pub(crate) struct Slot {
    pub(crate) buf: Buffer,
    sig: u64,
    /// Width the buffer is currently shaped for.
    shaped_w: Option<u32>,
    memo: HashMap<Option<u32>, (f32, f32)>,
    seen: u64,
}

pub struct TextEngine {
    pub fs: FontSystem,
    pub swash: SwashCache,
    pub(crate) slots: HashMap<String, Slot>,
    frame: u64,
}

fn sig(s: &TextSpec) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (&s.text, s.size.to_bits(), &s.family, s.weight, s.align, s.wrap, s.line_height.to_bits()).hash(&mut h);
    h.finish()
}

fn family(name: &str) -> Family<'_> {
    match name {
        "" | "sans-serif" | "sans" => Family::SansSerif,
        "serif" => Family::Serif,
        "monospace" | "mono" => Family::Monospace,
        n => Family::Name(n),
    }
}


fn slot<'a>(
    slots: &'a mut HashMap<String, Slot>,
    fs: &mut FontSystem,
    frame: u64,
    key: &str,
    spec: &TextSpec,
) -> &'a mut Slot {
    let s = sig(spec);
    let size = spec.size.max(1.0);
    let slot = slots.entry(key.to_string()).or_insert_with(|| Slot {
        buf: Buffer::new(fs, Metrics::new(size, size * spec.line_height)),
        sig: 0,
        shaped_w: None,
        memo: HashMap::new(),
        seen: frame,
    });
    slot.seen = frame;
    if slot.sig != s {
        slot.buf.set_metrics(Metrics::new(size, size * spec.line_height));
        slot.buf.set_wrap(if spec.wrap { Wrap::WordOrGlyph } else { Wrap::None });
        let attrs = Attrs::new().family(family(&spec.family)).weight(Weight(spec.weight));
        let align = match spec.align {
            TextAlign::Left => None,
            TextAlign::Center => Some(Align::Center),
            TextAlign::Right => Some(Align::Right),
        };
        slot.buf.set_text(&spec.text, &attrs, Shaping::Advanced, align);
        slot.sig = s;
        slot.shaped_w = Some(u32::MAX); // force a reshape
        slot.memo.clear();
    }
    slot
}

impl TextEngine {
    pub fn new() -> Self {
        Self { fs: FontSystem::new(), swash: SwashCache::new(), slots: HashMap::new(), frame: 0 }
    }

    /// Register a user font file (the "custom fonts" feature).
    pub fn load_font_file(&mut self, path: &std::path::Path) -> bool {
        let before = self.fs.db().len();
        self.fs.db_mut().load_font_file(path).is_ok() && self.fs.db().len() > before
    }

    pub fn load_font_dir(&mut self, dir: &std::path::Path) -> usize {
        let before = self.fs.db().len();
        self.fs.db_mut().load_fonts_dir(dir);
        self.fs.db().len() - before
    }

    pub fn family_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.fs.db().faces().filter_map(|f| f.families.first().map(|(n, _)| n.clone())).collect();
        v.sort();
        v.dedup();
        v
    }

    pub fn begin_frame(&mut self) {
        self.frame += 1;
    }

    pub fn end_frame(&mut self) {
        let f = self.frame;
        self.slots.retain(|_, s| s.seen + 12 >= f);
    }

    /// Content size at an optional width limit (logical px).
    pub fn measure(&mut self, key: &str, spec: &TextSpec, max_w: Option<f32>) -> (f32, f32) {
        let wkey = max_w.filter(|_| spec.wrap).map(|w| w.max(1.0).round() as u32);
        let slot = slot(&mut self.slots, &mut self.fs, self.frame, key, spec);
        if let Some(&m) = slot.memo.get(&wkey) {
            return m;
        }
        slot.buf.set_size(wkey.map(|w| w as f32), None);
        slot.buf.shape_until_scroll(&mut self.fs, false);
        slot.shaped_w = Some(wkey.unwrap_or(u32::MAX - 1));
        let (mut w, mut h) = (0.0f32, 0.0f32);
        for run in slot.buf.layout_runs() {
            w = w.max(run.line_w);
            h = h.max(run.line_top + run.line_height);
        }
        let m = (w.ceil(), h.ceil());
        slot.memo.insert(wkey, m);
        m
    }

    /// Shape for the final laid-out width so wrapping and alignment are exact.
    pub fn prepare(&mut self, key: &str, spec: &TextSpec, width: f32) {
        let slot = slot(&mut self.slots, &mut self.fs, self.frame, key, spec);
        let want = Some(width.max(1.0).round() as u32);
        if slot.shaped_w != want {
            slot.buf.set_size(want.map(|w| w as f32), None);
            slot.buf.shape_until_scroll(&mut self.fs, false);
            slot.shaped_w = want;
        }
    }

    pub fn buffer(&self, key: &str) -> Option<&Buffer> {
        self.slots.get(key).map(|s| &s.buf)
    }

    /// x offset of a byte position in a single-line buffer, for carets.
    pub fn caret_x(&self, key: &str, byte: usize) -> f32 {
        let Some(b) = self.buffer(key) else { return 0.0 };
        let mut end = 0.0;
        for run in b.layout_runs() {
            for g in run.glyphs {
                if g.start >= byte {
                    return g.x;
                }
                end = g.x + g.w;
            }
        }
        end
    }

    /// Byte offset nearest an x position, for click-to-place carets.
    pub fn byte_at(&self, key: &str, x: f32) -> usize {
        let Some(b) = self.buffer(key) else { return 0 };
        let mut last = 0;
        for run in b.layout_runs() {
            for g in run.glyphs {
                if x < g.x + g.w * 0.5 {
                    return g.start;
                }
                last = g.end;
            }
        }
        last
    }
}

impl Default for TextEngine {
    fn default() -> Self {
        Self::new()
    }
}
