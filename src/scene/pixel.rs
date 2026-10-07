//! The pixel tier of `scene diff`: two PNGs of the same scene compared the way dify and
//! egui_kittest compare them (YIQ colour distance per pixel, a threshold, a budget of failing
//! pixels), then described so a reader can act on it: how many pixels, where (regions that
//! name the dump nodes they cover) and a picture of it (`diff_image`).
//!
//! Both images are composited over the fixed backdrop first (a transparent render has no
//! colour of its own), so only what the engine drew is compared.
//!
//! The numbers are placeholders until `scene selfcheck` has measured them: a threshold of 0.6
//! on the YIQ distance (`yiq_delta`: the weighted squared difference of two colours in 0..255
//! units, so 0.6 is about one grey level), no failing pixel when the environment fingerprint
//! matches the baseline's and 0.02 % of the pixels otherwise. `scenes/wayfinder-render.toml`
//! says them, and a scene may only tighten them. Fonts are the machine's, so a baseline is only
//! ever compared on the machine and font set that made it (the fonts hash is part of the
//! fingerprint, and the baseline folder is named for it); what can still differ is the adapter,
//! the Windows build (WARP ships with it) and the engine's crates.

use std::collections::BTreeMap;
use std::path::Path;

use image::{Rgba, RgbaImage};
use serde::Deserialize;
use serde_json::Value as Json;

use super::dump::{self, Kind, Node};
use crate::render::backdrop_at;

/// The thresholds a failing-pixel count is swept over, for the report: how many pixels would
/// still fail at each.
pub const SWEEP: [f32; 5] = [0.0, 0.5, 1.0, 2.0, 5.0];
/// Failing pixels this close are one region.
const REGION_PAD: u32 = 4;
/// Regions listed in full; the rest are counted.
const REGIONS_SHOWN: usize = 8;
/// Dump nodes named under a region.
const COVERS: usize = 3;
/// The file the tolerances are in, in the scene root.
pub const FILE: &str = "wayfinder-render.toml";

/// How strict a comparison is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    /// A pixel fails when its YIQ distance to the baseline's is above this.
    pub threshold: f32,
    /// Failing pixels allowed when the fingerprint matches the baseline's.
    pub failed_pixels: u64,
    /// Percent of all pixels allowed to fail when it does not.
    pub failed_percent: f32,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { threshold: 0.6, failed_pixels: 0, failed_percent: 0.02 }
    }
}

impl Limits {
    /// How many pixels of `total` may fail.
    pub fn budget(&self, same_fingerprint: bool, total: u64) -> u64 {
        if same_fingerprint { self.failed_pixels } else { (total as f64 * f64::from(self.failed_percent) / 100.0 + 1e-6).floor() as u64 }
    }
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RawLimits {
    threshold: Option<f32>,
    failed_pixels: Option<u64>,
    failed_percent: Option<f32>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RawTolerances {
    format: Option<u32>,
    pixels: RawLimits,
    scene: BTreeMap<String, RawLimits>,
}

/// The tolerances of a scene root: `scenes/wayfinder-render.toml`, or the defaults.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tolerances {
    global: Limits,
    /// Scene ids or patterns (as `scene diff` takes them) and what each tightens.
    scenes: Vec<(String, RawTight)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct RawTight {
    threshold: Option<f32>,
    failed_pixels: Option<u64>,
    failed_percent: Option<f32>,
}

impl Tolerances {
    pub fn parse(text: &str, file: &Path) -> Result<Tolerances, String> {
        let raw: RawTolerances = toml::from_str(text).map_err(|e| format!("{}: {}", file.display(), e.to_string().trim_end()))?;
        match raw.format {
            Some(1) => {}
            Some(n) => return Err(format!("{}: this build reads format = 1, not {n}", file.display())),
            None => return Err(format!("{}: add `format = 1` at the top", file.display())),
        }
        let d = Limits::default();
        let global = Limits { threshold: raw.pixels.threshold.unwrap_or(d.threshold), failed_pixels: raw.pixels.failed_pixels.unwrap_or(d.failed_pixels), failed_percent: raw.pixels.failed_percent.unwrap_or(d.failed_percent) };
        let mut scenes = Vec::new();
        for (id, l) in raw.scene {
            let loose = |what: &str, mine: String, global: String| format!("{}: [scene.\"{id}\"] {what} = {mine} is looser than the global {global}: a scene may only tighten the tolerances", file.display());
            if let Some(t) = l.threshold.filter(|t| *t > global.threshold) {
                return Err(loose("threshold", t.to_string(), global.threshold.to_string()));
            }
            if let Some(n) = l.failed_pixels.filter(|n| *n > global.failed_pixels) {
                return Err(loose("failed_pixels", n.to_string(), global.failed_pixels.to_string()));
            }
            if let Some(p) = l.failed_percent.filter(|p| *p > global.failed_percent) {
                return Err(loose("failed_percent", p.to_string(), global.failed_percent.to_string()));
            }
            scenes.push((id, RawTight { threshold: l.threshold, failed_pixels: l.failed_pixels, failed_percent: l.failed_percent }));
        }
        Ok(Tolerances { global, scenes })
    }

    /// `<root>/wayfinder-render.toml`; the defaults when there is none.
    pub fn load(root: &Path) -> Result<Tolerances, String> {
        let file = root.join(FILE);
        match std::fs::read_to_string(&file) {
            Ok(t) => Tolerances::parse(&t, &file),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Tolerances::default()),
            Err(e) => Err(format!("{}: {e}", file.display())),
        }
    }

    /// The limits for scene `id`: the global ones, tightened by every entry that selects it.
    pub fn for_scene(&self, id: &str) -> Limits {
        let mut l = self.global;
        for (pat, t) in &self.scenes {
            if super::matches(pat, id) {
                l.threshold = t.threshold.map_or(l.threshold, |v| v.min(l.threshold));
                l.failed_pixels = t.failed_pixels.map_or(l.failed_pixels, |v| v.min(l.failed_pixels));
                l.failed_percent = t.failed_percent.map_or(l.failed_percent, |v| v.min(l.failed_percent));
            }
        }
        l
    }
}

/// What a render was made in, as far as it can change pixels. Two renders with the same
/// fingerprint are expected to be byte-identical; with another, a small budget applies.
#[derive(Clone, Debug, PartialEq)]
pub struct Fingerprint {
    pub fonts: String,
    pub adapter: String,
    pub windows: String,
    pub engine: String,
}

impl Fingerprint {
    /// Read from a render's record (`<id>.env.json`).
    pub fn of(sidecar: &Json) -> Option<Fingerprint> {
        let s = |v: &Json| v.as_str().unwrap_or("?").to_string();
        let a = sidecar.get("adapter").filter(|a| a.is_object())?;
        let crates = sidecar["engine"]["crates"].as_object().map(|c| c.iter().filter(|(k, _)| ["wgpu", "cosmic-text", "swash"].contains(&k.as_str())).map(|(k, v)| format!("{k} {}", v.as_str().unwrap_or("?"))).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        Some(Fingerprint { fonts: s(&sidecar["fonts"]["hash"]), adapter: format!("{} / driver {} / {}", s(&a["name"]), s(&a["driver"]), s(&a["backend"])), windows: s(&sidecar["windows"]), engine: crates })
    }

    /// The parts that differ from `other`.
    pub fn differs(&self, other: &Fingerprint) -> Vec<&'static str> {
        let mut out = Vec::new();
        for (name, a, b) in [("fonts", &self.fonts, &other.fonts), ("adapter", &self.adapter, &other.adapter), ("windows", &self.windows, &other.windows), ("engine", &self.engine, &other.engine)] {
            if a != b {
                out.push(name);
            }
        }
        out
    }

    pub fn describe(&self) -> String {
        format!("{}, windows {}, fonts {}, {}", self.adapter, self.windows, self.fonts, self.engine)
    }
}

/// The distance between two colours the way dify and pixelmatch measure it: the squared
/// differences along the Y, I and Q axes of the YIQ colour space, weighted. Grey 1 level apart
/// is 0.5; black and white are 35 215 apart.
pub fn yiq_delta(a: [u8; 3], b: [u8; 3]) -> f32 {
    let d = |k: usize| f32::from(a[k]) - f32::from(b[k]);
    let (r, g, bl) = (d(0), d(1), d(2));
    let y = r * 0.298_895_3 + g * 0.586_622_5 + bl * 0.114_482_23;
    let i = r * 0.595_977_99 - g * 0.274_176_1 - bl * 0.321_801_89;
    let q = r * 0.211_470_17 - g * 0.522_617_11 + bl * 0.311_146_94;
    0.5053 * y * y + 0.299 * i * i + 0.1957 * q * q
}

/// `img` over the fixed backdrop, opaque: what is compared.
pub fn flatten(img: &RgbaImage) -> RgbaImage {
    if img.pixels().all(|p| p.0[3] == 255) {
        return img.clone();
    }
    let (w, h) = img.dimensions();
    let mut out = img.clone();
    for (x, y, p) in out.enumerate_pixels_mut() {
        let a = f32::from(p.0[3]) / 255.0;
        let bg = backdrop_at(crate::ambient::Backdrop::Gradient, x as f32 / w as f32, y as f32 / h as f32);
        for k in 0..3 {
            p.0[k] = (f32::from(p.0[k]) * a + bg[k] * (1.0 - a)).clamp(0.0, 255.0).round() as u8;
        }
        p.0[3] = 255;
    }
    out
}

/// Two images of one size, compared.
#[derive(Clone, Debug, PartialEq)]
pub struct Compared {
    pub w: u32,
    pub h: u32,
    pub total: u64,
    /// Pixels whose distance is above the threshold.
    pub failing: u64,
    pub max_delta: f32,
    /// Failing pixels at each of `SWEEP`'s thresholds.
    pub sweep: Vec<(f32, u64)>,
    /// The distance of every pixel, row by row.
    deltas: Vec<f32>,
    threshold: f32,
}

impl Compared {
    pub fn percent(&self) -> f64 {
        if self.total == 0 { 0.0 } else { self.failing as f64 * 100.0 / self.total as f64 }
    }

    fn fails(&self, i: usize) -> bool {
        self.deltas[i] > self.threshold
    }
}

/// The outcome of comparing a baseline with a new render.
#[derive(Clone, Debug, PartialEq)]
pub enum Pair {
    /// The sizes differ: no pixel compare is meaningful.
    Size { base: (u32, u32), new: (u32, u32) },
    Same(Compared),
}

pub fn compare(base: &RgbaImage, new: &RgbaImage, threshold: f32) -> Pair {
    if base.dimensions() != new.dimensions() {
        return Pair::Size { base: base.dimensions(), new: new.dimensions() };
    }
    let (b, n) = (flatten(base), flatten(new));
    let (w, h) = b.dimensions();
    let deltas: Vec<f32> = b.pixels().zip(n.pixels()).map(|(p, q)| if p == q { 0.0 } else { yiq_delta([p.0[0], p.0[1], p.0[2]], [q.0[0], q.0[1], q.0[2]]) }).collect();
    let failing = deltas.iter().filter(|d| **d > threshold).count() as u64;
    let max_delta = deltas.iter().copied().fold(0.0, f32::max);
    let sweep = SWEEP.iter().map(|t| (*t, deltas.iter().filter(|d| **d > *t).count() as u64)).collect();
    Pair::Same(Compared { w, h, total: u64::from(w) * u64::from(h), failing, max_delta, sweep, deltas, threshold })
}

/// A rectangle of failing pixels (device px), and how many of them it holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    /// `[x, y, w, h]`.
    pub rect: [u32; 4],
    pub failing: u64,
}

fn touches(a: &[u32; 4], b: &[u32; 4]) -> bool {
    a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
}

/// The regions of a comparison: failing pixels grown by 4 px and merged where they join, the
/// biggest first (by failing pixels), and how many smaller ones beyond the first 8 there are.
pub fn regions(c: &Compared) -> (Vec<Region>, usize) {
    let (w, h) = (c.w as usize, c.h as usize);
    let pad = REGION_PAD as usize;
    // grow: a pixel is in when a failing one is within `pad` in both axes (two running passes)
    let mut grown = vec![false; w * h];
    let mut cols = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            if c.fails(y * w + x) {
                for xx in x.saturating_sub(pad)..=(x + pad).min(w - 1) {
                    cols[y * w + xx] = true;
                }
            }
        }
    }
    for y in 0..h {
        for x in 0..w {
            if cols[y * w + x] {
                for yy in y.saturating_sub(pad)..=(y + pad).min(h - 1) {
                    grown[yy * w + x] = true;
                }
            }
        }
    }
    // connected components of the grown mask, eight ways
    let mut seen = vec![false; w * h];
    let mut found: Vec<Region> = Vec::new();
    for start in 0..w * h {
        if !grown[start] || seen[start] {
            continue;
        }
        let (mut x0, mut y0, mut x1, mut y1, mut failing) = (usize::MAX, usize::MAX, 0usize, 0usize, 0u64);
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            if c.fails(i) {
                failing += 1;
            }
            for yy in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for xx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    let j = yy * w + xx;
                    if grown[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        found.push(Region { rect: [x0 as u32, y0 as u32, (x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32], failing });
    }
    // boxes that overlap are one region
    'merge: loop {
        for i in 0..found.len() {
            for j in i + 1..found.len() {
                if touches(&found[i].rect, &found[j].rect) {
                    let (a, b) = (found[i].rect, found.remove(j).rect);
                    let (x0, y0) = (a[0].min(b[0]), a[1].min(b[1]));
                    let (x1, y1) = ((a[0] + a[2]).max(b[0] + b[2]), (a[1] + a[3]).max(b[1] + b[3]));
                    found[i].rect = [x0, y0, x1 - x0, y1 - y0];
                    continue 'merge;
                }
            }
        }
        break;
    }
    // a merge dropped the second box's count with it: recount from the mask
    for r in &mut found {
        let [x, y, rw, rh] = r.rect;
        r.failing = (y..y + rh).flat_map(|yy| (x..x + rw).map(move |xx| (xx, yy))).filter(|(xx, yy)| c.fails(*yy as usize * w + *xx as usize)).count() as u64;
    }
    found.sort_by(|a, b| b.failing.cmp(&a.failing).then(a.rect[1].cmp(&b.rect[1])).then(a.rect[0].cmp(&b.rect[0])));
    let more = found.len().saturating_sub(REGIONS_SHOWN);
    found.truncate(REGIONS_SHOWN);
    (found, more)
}

/// The nodes of the dump whose rects meet `region` (logical px), smallest first.
fn covering<'a>(nodes: &'a [Node], region: [f64; 4]) -> Vec<&'a Node> {
    let meets = |r: &[f64; 4]| r[0] < region[0] + region[2] && region[0] < r[0] + r[2] && r[1] < region[1] + region[3] && region[1] < r[1] + r[3] && r[2] > 0.0 && r[3] > 0.0;
    let mut v: Vec<&Node> = nodes.iter().filter(|n| meets(&n.rect)).collect();
    v.sort_by(|a, b| (a.rect[2] * a.rect[3]).total_cmp(&(b.rect[2] * b.rect[3])));
    v.truncate(COVERS);
    v
}

fn named(n: &Node) -> String {
    match &n.kind {
        Kind::Text(t) => {
            let shown: String = t.text.chars().take(32).collect();
            format!("{} (text \"{shown}{}\")", n.key, if t.text.chars().count() > 32 { "..." } else { "" })
        }
        Kind::Image(_) => format!("{} (image)", n.key),
        Kind::Shape { shape, .. } => format!("{} (shape:{shape})", n.key),
        Kind::Box => format!("{} (box)", n.key),
    }
}

/// The report lines of a pixel comparison, in the shape `scene diff` prints under a scene:
/// the count and the threshold sweep, the fingerprint and the budget, and each region with the
/// dump nodes it covers.
pub fn report(c: &Compared, regions: &[Region], more: usize, nodes: &[Node], scale: f64, fp: &str, budget: u64) -> Vec<String> {
    let sweep: Vec<String> = c.sweep.iter().map(|(t, n)| format!("{t:.1}:{n}")).collect();
    let mut out = vec![
        format!("  pixels: {} failing of {} ({:.2} %), max delta {:.2}   sweep by threshold: {}", c.failing, c.total, c.percent(), c.max_delta, sweep.join(" ")),
        format!("  fingerprint: {fp}   budget {budget}"),
    ];
    for (i, r) in regions.iter().enumerate() {
        let logical = [f64::from(r.rect[0]) / scale, f64::from(r.rect[1]) / scale, f64::from(r.rect[2]) / scale, f64::from(r.rect[3]) / scale];
        let covers: Vec<String> = covering(nodes, logical).into_iter().map(named).collect();
        out.push(format!("  region {}  [{},{} {}x{}]  {} px   covers {}", i + 1, dump::n(logical[0]), dump::n(logical[1]), dump::n(logical[2]), dump::n(logical[3]), r.failing, if covers.is_empty() { "nothing in the dump".to_string() } else { covers.join(", ") }));
    }
    if more > 0 {
        out.push(format!("  +{more} small"));
    }
    out
}

/// The 3x5 digits that number regions on `diff_image`.
const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b001, 0b001, 0b001],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

/// The picture of a comparison: pixels that did not change dimmed to grey, failing ones
/// magenta, each region boxed in yellow and numbered as in the report.
pub fn diff_image(new: &RgbaImage, c: &Compared, regions: &[Region]) -> RgbaImage {
    let n = flatten(new);
    let mut out = RgbaImage::new(c.w, c.h);
    for (x, y, p) in n.enumerate_pixels() {
        let i = (y * c.w + x) as usize;
        let px = if c.fails(i) {
            Rgba([255, 0, 255, 255])
        } else {
            let lum = 0.299 * f32::from(p.0[0]) + 0.587 * f32::from(p.0[1]) + 0.114 * f32::from(p.0[2]);
            let g = (lum * 0.3 + 90.0).round() as u8;
            Rgba([g, g, g, 255])
        };
        out.put_pixel(x, y, px);
    }
    let yellow = Rgba([255, 221, 0, 255]);
    for (k, r) in regions.iter().enumerate() {
        let [x, y, w, h] = r.rect;
        for dx in 0..w {
            out.put_pixel(x + dx, y, yellow);
            out.put_pixel(x + dx, y + h - 1, yellow);
        }
        for dy in 0..h {
            out.put_pixel(x, y + dy, yellow);
            out.put_pixel(x + w - 1, y + dy, yellow);
        }
        // the number, 2x scale, on black, above the box's top left corner (inside it at the top
        // edge of the image, where it may cover failing pixels)
        let digits: Vec<usize> = (k + 1).to_string().bytes().map(|b| (b - b'0') as usize).collect();
        let (bx, by) = if y >= 12 { (x, y - 12) } else { (x + 1, y + 1) };
        let (bw, bh) = (digits.len() as u32 * 8 + 2, 12u32);
        for dy in 0..bh.min(c.h.saturating_sub(by)) {
            for dx in 0..bw.min(c.w.saturating_sub(bx)) {
                out.put_pixel(bx + dx, by + dy, Rgba([0, 0, 0, 255]));
            }
        }
        for (di, d) in digits.iter().enumerate() {
            for (row, bits) in DIGITS[*d].iter().enumerate() {
                for col in 0..3u32 {
                    if bits >> (2 - col) & 1 == 1 {
                        for (sx, sy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                            let (px, py) = (bx + 1 + di as u32 * 8 + col * 2 + sx, by + 1 + row as u32 * 2 + sy);
                            if px < c.w && py < c.h {
                                out.put_pixel(px, py, yellow);
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 3]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba([c[0], c[1], c[2], 255]))
    }

    #[test]
    fn the_yiq_distance_is_zero_for_equal_colours_and_big_for_black_against_white() {
        assert_eq!(yiq_delta([10, 20, 30], [10, 20, 30]), 0.0);
        let grey = yiq_delta([100, 100, 100], [101, 101, 101]);
        assert!((grey - 0.5053).abs() < 0.01, "one grey level is about 0.5: {grey}");
        let max = yiq_delta([0, 0, 0], [255, 255, 255]);
        assert!((max - 0.5053 * 255.0 * 255.0).abs() < 50.0, "{max}");
        assert_eq!(yiq_delta([1, 2, 3], [9, 8, 7]), yiq_delta([9, 8, 7], [1, 2, 3]));
    }

    #[test]
    fn identical_images_have_no_failing_pixels_and_a_changed_block_is_one_region() {
        let a = solid(40, 30, [20, 40, 60]);
        let Pair::Same(c) = compare(&a, &a.clone(), 0.6) else { panic!("same size") };
        assert_eq!((c.failing, c.max_delta, c.total), (0, 0.0, 1200));
        assert!(regions(&c).0.is_empty());
        let mut b = a.clone();
        for y in 10..14 {
            for x in 12..20 {
                b.put_pixel(x, y, Rgba([250, 250, 250, 255]));
            }
        }
        let Pair::Same(c) = compare(&a, &b, 0.6) else { panic!() };
        assert_eq!(c.failing, 32);
        let (r, more) = regions(&c);
        assert_eq!((r.len(), more, r[0].failing), (1, 0, 32));
        // the block grown by 4 px on every side, inside the image
        assert_eq!(r[0].rect, [8, 6, 16, 12]);
        assert_eq!(c.sweep.first().map(|s| s.1), Some(32), "{:?}", c.sweep);
    }

    #[test]
    fn a_small_difference_fails_at_a_low_threshold_only_and_the_sweep_says_so() {
        let a = solid(10, 10, [100, 100, 100]);
        let b = solid(10, 10, [104, 104, 104]);
        let Pair::Same(c) = compare(&a, &b, 0.6) else { panic!() };
        assert_eq!(c.failing, 100, "4 grey levels is about 8");
        let at = |t: f32| c.sweep.iter().find(|(s, _)| *s == t).map(|s| s.1).unwrap();
        assert_eq!((at(0.0), at(5.0)), (100, 100));
        let Pair::Same(loose) = compare(&a, &b, 10.0) else { panic!() };
        assert_eq!(loose.failing, 0, "{}", loose.max_delta);
    }

    #[test]
    fn two_far_apart_changes_are_two_regions_biggest_first_and_near_ones_merge() {
        let a = solid(120, 40, [0, 0, 0]);
        let mut b = a.clone();
        for (x0, w) in [(5u32, 2u32), (60, 6), (68, 2)] {
            for y in 20..22 {
                for x in x0..x0 + w {
                    b.put_pixel(x, y, Rgba([255, 255, 255, 255]));
                }
            }
        }
        let Pair::Same(c) = compare(&a, &b, 0.6) else { panic!() };
        let (r, more) = regions(&c);
        assert_eq!((r.len(), more), (2, 0), "{r:?}");
        assert_eq!(r[0].failing, 16, "the two blocks 2 px apart join: {r:?}");
        assert_eq!(r[1].failing, 4);
        // more than eight are cut and counted
        let mut many = a.clone();
        for k in 0..11u32 {
            many.put_pixel(5 + k * 10, 20, Rgba([255, 255, 255, 255]));
        }
        let Pair::Same(c) = compare(&a, &many, 0.6) else { panic!() };
        let (r, more) = regions(&c);
        assert_eq!((r.len(), more), (8, 3));
    }

    #[test]
    fn a_size_change_is_its_own_outcome() {
        assert_eq!(compare(&solid(10, 10, [0; 3]), &solid(10, 12, [0; 3]), 0.6), Pair::Size { base: (10, 10), new: (10, 12) });
    }

    #[test]
    fn a_transparent_pixel_is_compared_over_the_fixed_backdrop() {
        let mut a = RgbaImage::new(4, 4);
        a.put_pixel(1, 1, Rgba([255, 0, 0, 0]));
        let f = flatten(&a);
        assert!(f.pixels().all(|p| p.0[3] == 255));
        // a fully transparent pixel is the backdrop, whatever colour it carries
        assert_eq!(f.get_pixel(1, 1), flatten(&RgbaImage::new(4, 4)).get_pixel(1, 1));
    }

    #[test]
    fn tolerances_default_to_the_placeholders_and_a_scene_may_only_tighten() {
        let t = Tolerances::default();
        let l = t.for_scene("golden/x");
        assert_eq!((l.threshold, l.failed_pixels, l.failed_percent), (0.6, 0, 0.02));
        assert_eq!((l.budget(true, 100_000), l.budget(false, 100_000), l.budget(false, 10)), (0, 20, 0));
        let f = Path::new("t.toml");
        let t = Tolerances::parse("format = 1\n[pixels]\nthreshold = 1.0\nfailed_pixels = 4\n[scene.\"golden/clock\"]\nthreshold = 0.3\n[scene.\"golden/icon*\"]\nfailed_pixels = 1\n", f).unwrap();
        assert_eq!((t.for_scene("golden/clock").threshold, t.for_scene("golden/clock").failed_pixels), (0.3, 4));
        assert_eq!((t.for_scene("golden/icon_list").threshold, t.for_scene("golden/icon_list").failed_pixels), (1.0, 1));
        assert_eq!(t.for_scene("golden/other").failed_pixels, 4);
        let e = Tolerances::parse("format = 1\n[pixels]\nthreshold = 0.6\n[scene.\"a\"]\nthreshold = 2.0\n", f).unwrap_err();
        assert!(e.contains("may only tighten") && e.contains("threshold"), "{e}");
        assert!(Tolerances::parse("[pixels]\n", f).unwrap_err().contains("format = 1"));
        assert!(Tolerances::parse("format = 1\n[pixels]\nthreshhold = 1\n", f).unwrap_err().contains("threshhold"));
    }

    #[test]
    fn a_fingerprint_is_read_from_the_record_and_names_what_differs() {
        let rec = |windows: &str, driver: &str| serde_json::json!({ "fonts": { "hash": "abcd" }, "adapter": { "name": "WARP", "driver": driver, "backend": "Dx12" }, "windows": windows, "engine": { "crates": { "wgpu": "30.0.1", "cosmic-text": "0.19.0", "taffy": "0.14.0" } } });
        let a = Fingerprint::of(&rec("10.0.1", "1.0")).unwrap();
        assert_eq!(a.engine, "cosmic-text 0.19.0, wgpu 30.0.1");
        assert_eq!(a.differs(&Fingerprint::of(&rec("10.0.1", "1.0")).unwrap()), Vec::<&str>::new());
        assert_eq!(a.differs(&Fingerprint::of(&rec("10.0.2", "1.1")).unwrap()), ["adapter", "windows"]);
        assert!(Fingerprint::of(&serde_json::json!({ "adapter": null })).is_none(), "a dump's record has no adapter");
    }

    #[test]
    fn the_report_names_the_dump_nodes_a_region_covers_smallest_first() {
        let mut d = crate::scene::dump::fixtures::tiny_dump(&["Thursday 15 Jan"]);
        for n in d.nodes.iter_mut().filter(|n| matches!(n.kind, Kind::Text(_))) {
            n.rect = [40.0, 40.0, 90.0, 18.0];
        }
        let a = solid(300, 300, [0, 0, 0]);
        let mut b = a.clone();
        // a block over the text node of the tiny dump
        let t = d.nodes.iter().find(|n| matches!(n.kind, Kind::Text(_))).unwrap();
        let (x, y) = (t.rect[0] as u32 + 2, t.rect[1] as u32 + 2);
        for dy in 0..3 {
            for dx in 0..3 {
                b.put_pixel(x + dx, y + dy, Rgba([255, 255, 255, 255]));
            }
        }
        let Pair::Same(c) = compare(&a, &b, 0.6) else { panic!() };
        let (r, more) = regions(&c);
        let lines = report(&c, &r, more, &d.nodes, 1.0, "same", 0);
        assert!(lines[0].starts_with("  pixels: 9 failing of 90000 (0.01 %)") && lines[1].contains("budget 0"), "{lines:?}");
        assert!(lines[2].starts_with("  region 1  [") && lines[2].contains("covers") && lines[2].contains("(text \"Thursday 15 Jan\")"), "{lines:?}");
        // a drawing of it: failing pixels magenta, the region boxed
        let img = diff_image(&b, &c, &r);
        assert_eq!(img.get_pixel(x, y).0, [255, 0, 255, 255]);
        assert_eq!(img.get_pixel(r[0].rect[0] + r[0].rect[2] - 1, r[0].rect[1] + r[0].rect[3] - 1).0, [255, 221, 0, 255]);
    }
}
