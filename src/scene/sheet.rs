//! The contact sheet: many scene renders on one PNG an agent reads in one go. The sheet is a
//! Node tree (a title and a badge over each picture, the pictures as Image nodes) laid out and
//! drawn by the engine's own pipeline on the software adapter, so its labels use the same text
//! engine as everything else.
//!
//! One long edge is at most 1600 px. Tiles are shrunk by one common factor, printed in the
//! header (`x0.62`), and never below 0.4: past that the sheet paginates (`sheet-2.png`).
//! Planning (`plan`) is pure; only `compose` needs a device.

use std::ops::Range;
use std::time::{Duration, Instant};

use image::RgbaImage;

use super::pixel::flatten;
use crate::anim::Anim;
use crate::color::Color;
use crate::gfx::Gpu;
use crate::images::{Decoded, ImageOp};
use crate::text::TextEngine;
use crate::ui::{Env, ImageSpec, Kind, Node, layout};

/// The longest edge of a sheet, px.
pub const LONG_EDGE: u32 = 1600;
/// A sheet is never made smaller than this factor: it paginates instead.
pub const MIN_FACTOR: f32 = 0.4;
const PAD: f32 = 12.0;
const HEAD: f32 = 22.0;
/// A title and a badge, 14 px each, and the gaps around them.
const LABEL: f32 = 32.0;
/// A tile is at least this wide, so its label can be read.
const MIN_TILE: f32 = 150.0;

/// How a badge is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Ok,
    /// Worth a look: NEW, DUMP.
    Note,
    /// A finding: PIXELS, FLAGS, ERROR.
    Bad,
}

/// One picture on a sheet.
pub struct Tile {
    pub title: String,
    pub badge: String,
    pub tone: Tone,
    pub image: RgbaImage,
}

/// One page of a sheet.
pub struct Page {
    pub image: RgbaImage,
    pub factor: f32,
    /// The tiles on it.
    pub tiles: Range<usize>,
}

/// Which tiles share a page, in how many columns, and how much they are shrunk.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub tiles: Range<usize>,
    pub cols: usize,
    pub factor: f32,
    pub size: (u32, u32),
}

fn tile_w(w: u32, f: f32) -> f32 {
    (w as f32 * f).round().max(MIN_TILE)
}

/// The size of the sheet laying `sizes` in `cols` columns at factor `f`.
fn extent(sizes: &[(u32, u32)], cols: usize, f: f32) -> (f32, f32) {
    let (mut w, mut h) = (0.0f32, 2.0 * PAD + HEAD);
    for row in sizes.chunks(cols) {
        w = w.max(row.iter().map(|s| tile_w(s.0, f)).sum::<f32>() + PAD * (row.len() - 1) as f32);
        h += LABEL + row.iter().map(|s| (s.1 as f32 * f).round()).fold(0.0, f32::max) + PAD;
    }
    (w + 2.0 * PAD, h)
}

/// The largest factor up to 1 at which the layout fits the long edge; 0 when none does.
fn best_factor(sizes: &[(u32, u32)], cols: usize) -> f32 {
    let fits = |f: f32| {
        let (w, h) = extent(sizes, cols, f);
        w <= LONG_EDGE as f32 && h <= LONG_EDGE as f32
    };
    if fits(1.0) {
        return 1.0;
    }
    if !fits(0.01) {
        return 0.0;
    }
    let (mut lo, mut hi) = (0.01f32, 1.0f32);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if fits(mid) { lo = mid } else { hi = mid }
    }
    // a factor that prints the same everywhere: two decimals, rounded down
    (lo * 100.0).floor() / 100.0
}

/// Lays `sizes` out: the number of columns (the given one, or the one that shrinks the least
/// and then makes the squarest sheet) and the common factor, paginating (halving the tiles)
/// while the factor would fall below `min_factor` and more than one tile is left.
pub fn plan(sizes: &[(u32, u32)], cols: Option<usize>, min_factor: f32) -> Vec<Plan> {
    plan_range(sizes, 0, cols, min_factor)
}

fn plan_range(all: &[(u32, u32)], start: usize, cols: Option<usize>, min_factor: f32) -> Vec<Plan> {
    if all.is_empty() {
        return vec![];
    }
    let candidates: Vec<usize> = match cols {
        Some(c) => vec![c.clamp(1, all.len())],
        None => (1..=all.len().min(8)).collect(),
    };
    let best = candidates
        .into_iter()
        .map(|c| (c, best_factor(all, c)))
        .max_by(|a, b| {
            let score = |c: usize, f: f32| {
                let (w, h) = extent(all, c, f.max(0.01));
                (f, std::cmp::Reverse(w.max(h) as u32), std::cmp::Reverse(c))
            };
            let (sa, sb) = (score(a.0, a.1), score(b.0, b.1));
            sa.0.total_cmp(&sb.0).then(sa.1.cmp(&sb.1)).then(sa.2.cmp(&sb.2))
        })
        .expect("at least one candidate");
    if best.1 < min_factor && all.len() > 1 {
        let mid = all.len().div_ceil(2);
        let mut plans = plan_range(&all[..mid], start, cols, min_factor);
        plans.extend(plan_range(&all[mid..], start + mid, cols, min_factor));
        return plans;
    }
    let f = best.1.max(0.01);
    let (w, h) = extent(all, best.0, f);
    vec![Plan { tiles: start..start + all.len(), cols: best.0, factor: f, size: (w.ceil() as u32, h.ceil() as u32) }]
}

fn colour(hex: &str) -> Color {
    Color::parse(hex).expect("a colour literal")
}

/// The tail of `title` that fits `w` px at about 5.8 px a letter, led by an ellipsis.
fn fit_title(title: &str, w: f32) -> String {
    let max = ((w / 5.8) as usize).max(8);
    let n = title.chars().count();
    if n <= max {
        return title.to_string();
    }
    format!("…{}", title.chars().skip(n - (max - 1)).collect::<String>())
}

fn tone_colour(t: Tone) -> Color {
    colour(match t {
        Tone::Ok => "#6fd69a",
        Tone::Note => "#7fb4ff",
        Tone::Bad => "#ff8a7a",
    })
}

fn page_tree(tiles: &[Tile], p: &Plan, header: &str, pages: (usize, usize)) -> Node {
    let f = p.factor;
    let mut root = Node::new("sheet").wh(p.size.0 as f32, p.size.1 as f32).col().gap(PAD).pad(PAD).fill(colour("#12151c"));
    let page = if pages.1 > 1 { format!("   page {}/{}", pages.0, pages.1) } else { String::new() };
    let head = format!("{header}   x{f:.2}{page}");
    root = root.child(Node::text("sheet/head", head, 12.0, colour("#aab4c8")).h(HEAD).no_shrink());
    let sizes: Vec<usize> = p.tiles.clone().collect();
    for (r, row) in sizes.chunks(p.cols).enumerate() {
        let row_h = LABEL + row.iter().map(|i| (tiles[*i].image.height() as f32 * f).round()).fold(0.0, f32::max);
        let mut line = Node::new(format!("sheet/row{r}")).row().gap(PAD).h(row_h).no_shrink();
        for i in row {
            let t = &tiles[*i];
            let (iw, ih) = ((t.image.width() as f32 * f).round(), (t.image.height() as f32 * f).round());
            let w = tile_w(t.image.width(), f);
            let title = Node::text(format!("sheet/{i}/title"), fit_title(&t.title, w), 10.5, colour("#e6ebf5")).wh(w, 14.0).no_shrink();
            let badge = Node::text(format!("sheet/{i}/badge"), t.badge.clone(), 10.5, tone_colour(t.tone)).with_text(|s| s.weight = 700).wh(w, 14.0).no_shrink();
            let pic = Node { kind: Kind::Image(ImageSpec { id: format!("sheet/{i}"), w: iw, h: ih, ready: true, ..Default::default() }), ..Node::new(format!("sheet/{i}/img")).wh(iw, ih).no_shrink() };
            line = line.child(Node::new(format!("sheet/{i}")).col().gap(2.0).w(w).no_shrink().child(title).child(badge).child(pic));
        }
        root = root.child(line);
    }
    root
}

/// Draws `tiles` on `gpu` (the software adapter) as one page or several.
pub fn compose(gpu: &mut Gpu, tiles: &[Tile], header: &str, cols: Option<usize>, min_factor: f32) -> Result<Vec<Page>, String> {
    let sizes: Vec<(u32, u32)> = tiles.iter().map(|t| t.image.dimensions()).collect();
    let plans = plan(&sizes, cols, min_factor);
    let mut text = TextEngine::new();
    let mut anim = Anim::default();
    let mut out = Vec::new();
    for (n, p) in plans.iter().enumerate() {
        let ops: Vec<ImageOp> = p.tiles.clone().map(|i| {
            let flat = flatten(&tiles[i].image);
            ImageOp::Upload(format!("sheet/{i}"), Decoded { px: flat.into_raw(), w: tiles[i].image.width(), h: tiles[i].image.height(), frames: None })
        }).collect();
        gpu.apply(ops);
        let root = page_tree(tiles, p, header, (n + 1, plans.len()));
        let mut env = Env { text: &mut text, anim: &mut anim, hover: None, now: Instant::now(), scale: 1.0, trace: false };
        let frame = layout(&root, (p.size.0 as f32, p.size.1 as f32), &mut env);
        let px = gpu.render_offscreen(p.size.0, p.size.1, &frame.list, &mut text, Duration::ZERO);
        gpu.apply(p.tiles.clone().map(|i| ImageOp::Drop(format!("sheet/{i}"))).collect());
        let mut px = px?;
        px.chunks_exact_mut(4).for_each(|c| c[3] = 255);
        let image = RgbaImage::from_raw(p.size.0, p.size.1, px).ok_or("the renderer returned the wrong size")?;
        out.push(Page { image, factor: p.factor, tiles: p.tiles.clone() });
    }
    Ok(out)
}

/// `sheet.png`, `sheet-2.png`...: the file name of page `n` (from 1) of a sheet called `base`.
pub fn page_name(base: &std::path::Path, n: usize) -> std::path::PathBuf {
    if n <= 1 {
        return base.to_path_buf();
    }
    let stem = base.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    base.with_file_name(format!("{stem}-{n}.png"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_few_small_tiles_fit_at_full_size_in_the_squarest_columns() {
        let p = plan(&[(200, 200); 4], None, MIN_FACTOR);
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].factor, p[0].tiles.clone()), (1.0, 0..4));
        let (w, h) = p[0].size;
        assert!(w <= LONG_EDGE && h <= LONG_EDGE && p[0].cols == 2, "{p:?}");
        assert_eq!(plan(&[(200, 200); 4], Some(4), MIN_FACTOR)[0].cols, 4);
        assert_eq!(plan(&[(200, 200)], Some(9), MIN_FACTOR)[0].cols, 1, "no more columns than tiles");
        assert!(plan(&[], None, MIN_FACTOR).is_empty());
    }

    #[test]
    fn big_tiles_share_one_common_factor_that_keeps_the_long_edge_within_1600() {
        let sizes = [(950, 700); 3];
        let p = plan(&sizes, Some(3), MIN_FACTOR);
        assert_eq!(p.len(), 1);
        assert!(p[0].factor < 1.0 && p[0].factor >= MIN_FACTOR, "{p:?}");
        assert!(p[0].size.0 <= LONG_EDGE && p[0].size.1 <= LONG_EDGE, "{p:?}");
        let again = plan(&sizes, Some(3), MIN_FACTOR);
        assert_eq!(p, again, "planning is deterministic");
    }

    #[test]
    fn a_sheet_that_would_shrink_below_the_floor_paginates_in_order() {
        let sizes = vec![(1200, 900); 20];
        let p = plan(&sizes, None, MIN_FACTOR);
        assert!(p.len() > 1, "{p:?}");
        assert_eq!(p.first().map(|x| x.tiles.start), Some(0));
        assert_eq!(p.last().map(|x| x.tiles.end), Some(20));
        for w in p.windows(2) {
            assert_eq!(w[0].tiles.end, w[1].tiles.start, "pages follow one another");
        }
        assert!(p.iter().all(|x| x.factor >= MIN_FACTOR && x.size.0 <= LONG_EDGE && x.size.1 <= LONG_EDGE), "{p:?}");
        // with no floor one page is made, however small
        assert_eq!(plan(&sizes, None, 0.0).len(), 1);
    }

    #[test]
    fn page_names_and_long_titles_are_made_to_fit() {
        let base = std::path::Path::new("out/sheet.png");
        assert_eq!((page_name(base, 1), page_name(base, 2)), (base.to_path_buf(), std::path::PathBuf::from("out/sheet-2.png")));
        assert_eq!(fit_title("golden/clock", 200.0), "golden/clock");
        let t = fit_title("golden/system_monitor/large_with_everything_on#all_on", 150.0);
        assert!(t.starts_with('…') && t.ends_with("#all_on") && t.chars().count() <= 26, "{t}");
    }
}
