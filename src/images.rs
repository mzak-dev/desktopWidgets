//! Image files for widgets: PNG, JPEG, WebP, GIF and BMP. An animated GIF, WebP or APNG is
//! packed into one texture, a grid of its frames, so an animation is one file and one
//! upload; the renderer picks the frame to show by time.

use std::io::Cursor;
use std::path::Path;

use image::{AnimationDecoder, ImageFormat, RgbaImage};

/// Largest side of a packed animation; bigger ones are scaled down to fit.
pub const MAX_ATLAS: u32 = 4096;
pub const MAX_FRAMES: usize = 300;
/// A frame delay below this is played at 100 ms, as browsers do (GIFs often say 0 or 10).
const MIN_DELAY_MS: u32 = 20;

/// How an animation's frames sit in its texture: `count` cells, row by row.
#[derive(Clone, Debug, PartialEq)]
pub struct Frames {
    pub cols: u32,
    pub rows: u32,
    pub count: u32,
    /// One frame's size before any scaling, for layout.
    pub frame_w: u32,
    pub frame_h: u32,
    pub delays_ms: Vec<u32>,
    pub total_ms: u64,
}

/// Decoded pixels, straight-alpha RGBA8. For an animation `px` is the packed grid.
pub struct Decoded {
    pub px: Vec<u8>,
    pub w: u32,
    pub h: u32,
    pub frames: Option<Frames>,
}

impl Decoded {
    /// The size to lay the image out at: one frame's.
    pub fn size(&self) -> (u32, u32) {
        self.frames.as_ref().map_or((self.w, self.h), |f| (f.frame_w, f.frame_h))
    }
}

pub fn decode_file(p: &Path) -> Option<Decoded> {
    decode_bytes(&std::fs::read(p).ok()?)
}

fn still(bytes: &[u8]) -> Option<Decoded> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Some(Decoded { px: img.into_raw(), w, h, frames: None })
}

fn collect<'a>(d: impl AnimationDecoder<'a>) -> Option<Vec<(RgbaImage, u32)>> {
    let frames: Vec<image::Frame> = d.into_frames().take(MAX_FRAMES).collect::<Result<_, _>>().ok()?;
    Some(frames.into_iter().map(|f| {
        let (n, d) = f.delay().numer_denom_ms();
        let ms = if d == 0 { 0 } else { n / d };
        (f.into_buffer(), if ms < MIN_DELAY_MS { 100 } else { ms })
    }).collect())
}

/// Any supported file; animations keep their frames.
pub fn decode_bytes(bytes: &[u8]) -> Option<Decoded> {
    let frames = match image::guess_format(bytes).ok()? {
        ImageFormat::Gif => collect(image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).ok()?),
        ImageFormat::WebP => {
            let d = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes)).ok()?;
            if d.has_animation() { collect(d) } else { None }
        }
        ImageFormat::Png => {
            let d = image::codecs::png::PngDecoder::new(Cursor::new(bytes)).ok()?;
            if d.is_apng().unwrap_or(false) { collect(d.apng().ok()?) } else { None }
        }
        _ => None,
    };
    match frames {
        Some(f) if f.len() > 1 => Some(pack(f)),
        Some(mut f) if f.len() == 1 => {
            let (img, _) = f.remove(0);
            let (w, h) = img.dimensions();
            Some(Decoded { px: img.into_raw(), w, h, frames: None })
        }
        _ => still(bytes),
    }
}

/// Lays frames out in a near-square grid, scaled down if the grid would pass `MAX_ATLAS`.
pub fn pack(frames: Vec<(RgbaImage, u32)>) -> Decoded {
    let n = frames.len() as u32;
    let (fw, fh) = frames[0].0.dimensions();
    let cols = (n as f64).sqrt().ceil() as u32;
    let rows = n.div_ceil(cols);
    let k = (MAX_ATLAS as f64 / (cols * fw) as f64).min(MAX_ATLAS as f64 / (rows * fh) as f64).min(1.0);
    let (cw, ch) = (((fw as f64 * k) as u32).max(1), ((fh as f64 * k) as u32).max(1));
    let mut atlas = RgbaImage::new(cols * cw, rows * ch);
    let mut delays = Vec::with_capacity(frames.len());
    for (i, (img, delay)) in frames.into_iter().enumerate() {
        let cell = if (cw, ch) == img.dimensions() { img } else { image::imageops::resize(&img, cw, ch, image::imageops::FilterType::Triangle) };
        let (x, y) = ((i as u32 % cols) * cw, (i as u32 / cols) * ch);
        image::imageops::replace(&mut atlas, &cell, x as i64, y as i64);
        delays.push(delay);
    }
    let total_ms = delays.iter().map(|d| *d as u64).sum();
    let (w, h) = atlas.dimensions();
    Decoded { px: atlas.into_raw(), w, h, frames: Some(Frames { cols, rows, count: n, frame_w: fw, frame_h: fh, delays_ms: delays, total_ms }) }
}

/// The frame showing `elapsed_ms` into a looping animation.
pub fn frame_at(f: &Frames, elapsed_ms: u64) -> u32 {
    let mut t = elapsed_ms % f.total_ms.max(1);
    for (i, d) in f.delays_ms.iter().enumerate() {
        if t < *d as u64 {
            return i as u32;
        }
        t -= *d as u64;
    }
    0
}

/// Frame `i`'s rectangle in texture coordinates: `[u0, v0, u1, v1]`.
pub fn cell_uv(f: &Frames, i: u32) -> [f32; 4] {
    let (c, r) = ((i % f.count) % f.cols, (i % f.count) / f.cols);
    let (cw, ch) = (1.0 / f.cols as f32, 1.0 / f.rows as f32);
    [c as f32 * cw, r as f32 * ch, (c + 1) as f32 * cw, (r + 1) as f32 * ch]
}

/// `inner` (a rectangle within 0..1) placed inside `outer`.
pub fn compose(outer: [f32; 4], inner: [f32; 4]) -> [f32; 4] {
    let (w, h) = (outer[2] - outer[0], outer[3] - outer[1]);
    [outer[0] + inner[0] * w, outer[1] + inner[1] * h, outer[0] + inner[2] * w, outer[1] + inner[3] * h]
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Delay, Frame, Rgba};

    fn solid(w: u32, h: u32, c: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba(c))
    }

    fn gif(frames: &[([u8; 4], u32)]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = image::codecs::gif::GifEncoder::new(&mut out);
            enc.set_repeat(image::codecs::gif::Repeat::Infinite).unwrap();
            for (c, ms) in frames {
                enc.encode_frame(Frame::from_parts(solid(8, 6, *c), 0, 0, Delay::from_numer_denom_ms(*ms, 1))).unwrap();
            }
        }
        out
    }

    fn pixel(d: &Decoded, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * d.w + x) * 4) as usize;
        d.px[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn an_animated_gif_is_one_grid_of_its_frames() {
        let d = decode_bytes(&gif(&[([255, 0, 0, 255], 100), ([0, 255, 0, 255], 200), ([0, 0, 255, 255], 0)])).unwrap();
        let f = d.frames.clone().expect("animated");
        assert_eq!((f.count, f.cols, f.rows, d.size()), (3, 2, 2, (8, 6)));
        assert_eq!(f.delays_ms, [100, 200, 100], "a 0 ms delay plays at 100 ms");
        assert_eq!((d.w, d.h), (16, 12));
        assert_eq!(pixel(&d, 3, 3)[..3], [255, 0, 0]);
        assert_eq!(pixel(&d, 11, 3)[..3], [0, 255, 0]);
        assert_eq!(pixel(&d, 3, 9)[..3], [0, 0, 255]);
    }

    #[test]
    fn frames_follow_their_delays_and_loop() {
        let f = Frames { cols: 2, rows: 2, count: 3, frame_w: 8, frame_h: 6, delays_ms: vec![100, 200, 100], total_ms: 400 };
        let at = |t| frame_at(&f, t);
        assert_eq!((at(0), at(99), at(100), at(299), at(300), at(400), at(450)), (0, 0, 1, 1, 2, 0, 0));
        assert_eq!(cell_uv(&f, 0), [0.0, 0.0, 0.5, 0.5]);
        assert_eq!(cell_uv(&f, 2), [0.0, 0.5, 0.5, 1.0]);
        assert_eq!(compose([0.5, 0.5, 1.0, 1.0], [0.0, 0.25, 1.0, 0.75]), [0.5, 0.625, 1.0, 0.875]);
    }

    #[test]
    fn a_huge_animation_is_scaled_to_fit_one_texture() {
        // 2 x 2 frames of 2100 px would need 4200 px across
        let frames: Vec<(RgbaImage, u32)> = (0..4).map(|_| (solid(2100, 40, [9, 9, 9, 255]), 50)).collect();
        let d = pack(frames);
        assert!(d.w <= MAX_ATLAS && d.h <= MAX_ATLAS, "{}x{}", d.w, d.h);
        assert_eq!(d.size(), (2100, 40), "laid out at its own size");
    }

    #[test]
    fn stills_of_every_format_decode() {
        let img = image::DynamicImage::ImageRgba8(solid(5, 4, [10, 20, 30, 255]));
        for fmt in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::Bmp, ImageFormat::WebP, ImageFormat::Gif] {
            let mut bytes = Cursor::new(Vec::new());
            let img = if fmt == ImageFormat::Jpeg { image::DynamicImage::ImageRgb8(img.to_rgb8()) } else { img.clone() };
            img.write_to(&mut bytes, fmt).unwrap();
            let d = decode_bytes(bytes.get_ref()).unwrap_or_else(|| panic!("{fmt:?}"));
            assert_eq!((d.w, d.h, d.frames.is_none()), (5, 4, true), "{fmt:?}");
        }
        assert!(decode_bytes(b"not an image").is_none());
    }
}
