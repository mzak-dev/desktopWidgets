//! Straight-alpha sRGB colour. Shaders premultiply (ADR-001).

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Color(pub [f32; 4]);

/// The loud "you referenced something undefined" colour (decision 14).
pub const MAGENTA: Color = Color([1.0, 0.0, 1.0, 1.0]);
pub const TRANSPARENT: Color = Color([0.0; 4]);

impl Color {
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self([r, g, b, a])
    }

    /// `#rgb`, `#rrggbb`, `#rrggbbaa`, or a few names.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        match s {
            "transparent" => return Some(TRANSPARENT),
            "white" => return Some(Self([1.0; 4])),
            "black" => return Some(Self([0.0, 0.0, 0.0, 1.0])),
            _ => {}
        }
        let h = s.strip_prefix('#')?;
        if !h.is_ascii() {
            return None;
        }
        let byte = |i: usize, n: usize| u8::from_str_radix(&h[i..i + n], 16).ok();
        let (r, g, b, a) = match h.len() {
            3 => (byte(0, 1)? * 17, byte(1, 1)? * 17, byte(2, 1)? * 17, 255),
            6 => (byte(0, 2)?, byte(2, 2)?, byte(4, 2)?, 255),
            8 => (byte(0, 2)?, byte(2, 2)?, byte(4, 2)?, byte(6, 2)?),
            _ => return None,
        };
        let f = |v: u8| v as f32 / 255.0;
        Some(Self([f(r), f(g), f(b), f(a)]))
    }

    pub fn to_hex(self) -> String {
        let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let [r, g, bl, a] = self.0;
        if a >= 0.999 {
            format!("#{:02x}{:02x}{:02x}", b(r), b(g), b(bl))
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", b(r), b(g), b(bl), b(a))
        }
    }

    pub fn mul_alpha(self, k: f32) -> Self {
        let [r, g, b, a] = self.0;
        Self([r, g, b, a * k])
    }

    pub fn with_alpha(self, a: f32) -> Self {
        let [r, g, b, _] = self.0;
        Self([r, g, b, a])
    }

    pub fn lerp(self, o: Self, t: f32) -> Self {
        let mut c = [0.0; 4];
        for (i, v) in c.iter_mut().enumerate() {
            *v = self.0[i] + (o.0[i] - self.0[i]) * t;
        }
        Self(c)
    }

    /// Text colour for glyphon.
    pub fn to_u8(self) -> [u8; 4] {
        self.0.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
    }

    /// HSV in 0..1 (h,s,v), alpha untouched.
    pub fn to_hsv(self) -> [f32; 3] {
        let [r, g, b, _] = self.0;
        let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
        let d = mx - mn;
        let h = if d == 0.0 {
            0.0
        } else if mx == r {
            ((g - b) / d).rem_euclid(6.0)
        } else if mx == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        } / 6.0;
        [h, if mx == 0.0 { 0.0 } else { d / mx }, mx]
    }

    pub fn from_hsv(h: f32, s: f32, v: f32) -> Self {
        let f = |n: f32| {
            let k = (n + h * 6.0).rem_euclid(6.0);
            v - v * s * k.min(4.0 - k).clamp(0.0, 1.0)
        };
        Self([f(5.0), f(3.0), f(1.0), 1.0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        for s in ["#ff8800", "#00000080", "#123456"] {
            assert_eq!(Color::parse(s).unwrap().to_hex(), s);
        }
        assert_eq!(Color::parse("#fff"), Some(Color([1.0; 4])));
        assert_eq!(Color::parse("#12"), None);
        assert_eq!(Color::parse("nope"), None);
        assert_eq!(Color::parse("#ééé"), None); // non-ASCII must not panic on slicing
    }

    #[test]
    fn hsv_round_trip() {
        let c = Color::parse("#3b82f6").unwrap();
        let [h, s, v] = c.to_hsv();
        assert_eq!(Color::from_hsv(h, s, v).to_hex(), "#3b82f6");
    }
}
