//! Linear-space RGBA color primitive.

/// Linear-space RGBA color. Components are nominally in `[0, 1]` but are not
/// clamped (HDR values are permitted).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const WHITE: Self = Self::new(1.0, 1.0, 1.0, 1.0);
    pub const BLACK: Self = Self::new(0.0, 0.0, 0.0, 1.0);
    pub const TRANSPARENT: Self = Self::new(0.0, 0.0, 0.0, 0.0);
    pub const RED: Self = Self::new(1.0, 0.0, 0.0, 1.0);
    pub const GREEN: Self = Self::new(0.0, 1.0, 0.0, 1.0);
    pub const BLUE: Self = Self::new(0.0, 0.0, 1.0, 1.0);

    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Alpha = 1.
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// Decodes RGB through the sRGB EOTF into linear space. Alpha is a linear passthrough.
    pub fn from_srgb8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self {
            r: srgb_to_linear(r as f32 / 255.0),
            g: srgb_to_linear(g as f32 / 255.0),
            b: srgb_to_linear(b as f32 / 255.0),
            a: a as f32 / 255.0,
        }
    }

    /// Applies the inverse sRGB EOTF to RGB. Components are clamped to `[0, 1]`
    /// before rounding. Alpha is a linear passthrough.
    pub fn to_srgb8(self) -> [u8; 4] {
        [
            encode_channel(linear_to_srgb(self.r)),
            encode_channel(linear_to_srgb(self.g)),
            encode_channel(linear_to_srgb(self.b)),
            encode_channel(self.a),
        ]
    }

    pub fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    /// Unclamped.
    pub fn lerp(self, rhs: Self, t: f32) -> Self {
        Self {
            r: self.r + (rhs.r - self.r) * t,
            g: self.g + (rhs.g - self.g) * t,
            b: self.b + (rhs.b - self.b) * t,
            a: self.a + (rhs.a - self.a) * t,
        }
    }

    pub fn as_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    pub fn is_finite(self) -> bool {
        self.r.is_finite() && self.g.is_finite() && self.b.is_finite() && self.a.is_finite()
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::WHITE
    }
}

impl core::ops::Mul<f32> for Color {
    type Output = Self;

    fn mul(self, rhs: f32) -> Self {
        Self {
            r: self.r * rhs,
            g: self.g * rhs,
            b: self.b * rhs,
            a: self.a * rhs,
        }
    }
}

impl core::ops::Mul<Color> for Color {
    type Output = Self;

    fn mul(self, rhs: Color) -> Self {
        Self {
            r: self.r * rhs.r,
            g: self.g * rhs.g,
            b: self.b * rhs.b,
            a: self.a * rhs.a,
        }
    }
}

impl core::ops::Add for Color {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self {
            r: self.r + rhs.r,
            g: self.g + rhs.g,
            b: self.b + rhs.b,
            a: self.a + rhs.a,
        }
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(l: f32) -> f32 {
    if l <= 0.0031308 {
        12.92 * l
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    }
}

fn encode_channel(x: f32) -> u8 {
    let x = x.clamp(0.0, 1.0);
    (x * 255.0 + 0.5) as u8
}

const _: () = assert!(core::mem::size_of::<Color>() == 16);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::ApproxEq;

    #[test]
    fn new_and_rgb() {
        let c = Color::new(0.1, 0.2, 0.3, 0.4);
        assert_eq!(c.r, 0.1);
        assert_eq!(c.g, 0.2);
        assert_eq!(c.b, 0.3);
        assert_eq!(c.a, 0.4);
        assert_eq!(Color::rgb(0.1, 0.2, 0.3).a, 1.0);
    }

    #[test]
    fn default_is_white() {
        assert_eq!(Color::default(), Color::WHITE);
    }

    #[test]
    fn constants() {
        assert_eq!(Color::WHITE, Color::new(1.0, 1.0, 1.0, 1.0));
        assert_eq!(Color::BLACK, Color::new(0.0, 0.0, 0.0, 1.0));
        assert_eq!(Color::TRANSPARENT, Color::new(0.0, 0.0, 0.0, 0.0));
        assert_eq!(Color::RED, Color::new(1.0, 0.0, 0.0, 1.0));
        assert_eq!(Color::GREEN, Color::new(0.0, 1.0, 0.0, 1.0));
        assert_eq!(Color::BLUE, Color::new(0.0, 0.0, 1.0, 1.0));
    }

    #[test]
    fn srgb_roundtrip_all_values_per_channel() {
        for v in 0u16..=255 {
            let v = v as u8;
            let c = Color::from_srgb8(v, v, v, v);
            let out = c.to_srgb8();
            assert_eq!(out[0], v);
            assert_eq!(out[1], v);
            assert_eq!(out[2], v);
            assert_eq!(out[3], v);
        }
    }

    #[test]
    fn srgb_known_values() {
        let black = Color::from_srgb8(0, 0, 0, 255);
        assert!(black.r.approx_eq_default(0.0));
        let white = Color::from_srgb8(255, 255, 255, 255);
        assert!(white.r.approx_eq_default(1.0));
        // Mid-gray sRGB 188 decodes to roughly linear 0.5.
        let gray = Color::from_srgb8(188, 188, 188, 255);
        assert!((gray.r - 0.5).abs() < 0.01);
    }

    #[test]
    fn to_srgb8_clamps() {
        let c = Color::new(-1.0, 2.0, 0.0, 5.0);
        let out = c.to_srgb8();
        assert_eq!(out[0], 0);
        assert_eq!(out[1], 255);
        assert_eq!(out[2], 0);
        assert_eq!(out[3], 255);
    }

    #[test]
    fn with_alpha() {
        let c = Color::new(0.1, 0.2, 0.3, 0.4).with_alpha(0.9);
        assert_eq!(c.a, 0.9);
        assert_eq!(c.r, 0.1);
    }

    #[test]
    fn lerp() {
        let a = Color::new(0.0, 0.0, 0.0, 0.0);
        let b = Color::new(1.0, 1.0, 1.0, 1.0);
        let m = a.lerp(b, 0.5);
        assert!(m.approx_eq_default(Color::new(0.5, 0.5, 0.5, 0.5)));
    }

    #[test]
    fn as_array() {
        assert_eq!(
            Color::new(0.1, 0.2, 0.3, 0.4).as_array(),
            [0.1, 0.2, 0.3, 0.4]
        );
    }

    #[test]
    fn is_finite() {
        assert!(Color::WHITE.is_finite());
        assert!(!Color::new(f32::NAN, 0.0, 0.0, 0.0).is_finite());
        assert!(!Color::new(0.0, f32::INFINITY, 0.0, 0.0).is_finite());
    }

    #[test]
    fn ops() {
        let c = Color::new(0.2, 0.4, 0.6, 0.8) * 0.5;
        assert!(c.approx_eq_default(Color::new(0.1, 0.2, 0.3, 0.4)));
        let m = Color::new(0.5, 0.5, 0.5, 1.0) * Color::new(0.2, 0.4, 0.6, 0.5);
        assert!(m.approx_eq_default(Color::new(0.1, 0.2, 0.3, 0.5)));
        let s = Color::new(0.1, 0.2, 0.3, 0.4) + Color::new(0.1, 0.1, 0.1, 0.1);
        assert!(s.approx_eq_default(Color::new(0.2, 0.3, 0.4, 0.5)));
    }
}
