//! Text over glyphon (decision 25): one buffer per node key, re-shaped only when
//! its text, style or width changes.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use glyphon::cosmic_text::fontdb::{ID, Source};
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
    /// Font files registered by `sync_fonts`, with the size and time they were read at.
    files: HashMap<PathBuf, (FileStamp, Vec<ID>)>,
}

type FileStamp = (u64, Option<SystemTime>);

fn stamp(p: &Path) -> std::io::Result<FileStamp> {
    let m = std::fs::metadata(p)?;
    Ok((m.len(), m.modified().ok()))
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
        Self { fs: FontSystem::new(), swash: SwashCache::new(), slots: HashMap::new(), frame: 0, files: HashMap::new() }
    }

    /// Makes the registered font files exactly `files`: new and changed ones are read in,
    /// gone ones removed. Files are read into memory, never mapped, so Windows can still
    /// delete them (a removed Plugin). Returns the files that could not be used.
    ///
    /// ponytail: cosmic-text keeps a removed face's data in its own font cache until restart.
    pub fn sync_fonts(&mut self, files: &[PathBuf]) -> Vec<String> {
        let wanted: HashSet<&PathBuf> = files.iter().collect();
        let gone: Vec<PathBuf> = self.files.keys().filter(|p| !wanted.contains(p)).cloned().collect();
        let mut changed = !gone.is_empty();
        for p in gone {
            self.unload(&p);
        }
        let mut problems = Vec::new();
        for p in files {
            let st = match stamp(p) {
                Ok(st) => st,
                Err(e) => {
                    changed |= self.unload(p);
                    problems.push(format!("{}: {e}", p.display()));
                    continue;
                }
            };
            if self.files.get(p).is_some_and(|(old, _)| *old == st) {
                continue;
            }
            changed |= self.unload(p);
            match std::fs::read(p) {
                Ok(bytes) => {
                    let faces = self.fs.db_mut().load_font_source(Source::Binary(Arc::new(bytes)));
                    if faces.is_empty() {
                        problems.push(format!("{}: not a font file", p.display()));
                    }
                    self.files.insert(p.clone(), (st, faces.to_vec()));
                    changed = true;
                }
                Err(e) => problems.push(format!("{}: {e}", p.display())),
            }
        }
        if changed {
            self.slots.clear(); // a family may now resolve to a different face
        }
        problems
    }

    fn unload(&mut self, p: &Path) -> bool {
        let Some((_, faces)) = self.files.remove(p) else { return false };
        for id in faces {
            self.fs.db_mut().remove_face(id);
        }
        true
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A copy of some installed .ttf, so the test owns (and may delete) the file.
    fn font_copy(t: &TextEngine, name: &str) -> PathBuf {
        let src = t
            .fs
            .db()
            .faces()
            .find_map(|f| match &f.source {
                Source::File(p) | Source::SharedFile(p, _) if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ttf")) => Some(p.clone()),
                _ => None,
            })
            .expect("some installed .ttf font");
        let dir = std::env::temp_dir().join(format!("wf-fonts-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("Mine.ttf");
        std::fs::copy(src, &p).unwrap();
        p
    }

    #[test]
    fn sync_fonts_adds_once_and_removes_gone_files() {
        let mut t = TextEngine::new();
        let base = t.fs.db().len();
        let p = font_copy(&t, "sync");
        assert!(t.sync_fonts(std::slice::from_ref(&p)).is_empty());
        let loaded = t.fs.db().len();
        assert!(loaded > base);
        assert!(t.sync_fonts(std::slice::from_ref(&p)).is_empty());
        assert_eq!(t.fs.db().len(), loaded, "the same file is never loaded twice");
        t.sync_fonts(&[]);
        assert_eq!(t.fs.db().len(), base, "a file no longer wanted is removed");
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[test]
    fn a_synced_font_file_can_be_deleted_after_use() {
        let mut t = TextEngine::new();
        let p = font_copy(&t, "delete");
        t.sync_fonts(std::slice::from_ref(&p));
        let id = t.files[&p].1[0];
        assert!(t.fs.get_font(id, Weight(400)).is_some(), "the face is usable");
        std::fs::remove_file(&p).expect("read into memory, so nothing holds the file");
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[test]
    fn unusable_font_files_are_reported_not_fatal() {
        let mut t = TextEngine::new();
        let dir = std::env::temp_dir().join(format!("wf-fonts-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("junk.ttf"), "not a font").unwrap();
        let problems = t.sync_fonts(&[dir.join("junk.ttf"), dir.join("missing.ttf")]);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].contains("not a font"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
