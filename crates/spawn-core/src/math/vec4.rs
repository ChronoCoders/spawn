use core::ops::{
    Add, AddAssign, Div, DivAssign, Index, IndexMut, Mul, MulAssign, Neg, Sub, SubAssign,
};

use crate::math::Vec3;

/// A 4-component vector of `f32`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec4 {
    /// The x component.
    pub x: f32,
    /// The y component.
    pub y: f32,
    /// The z component.
    pub z: f32,
    /// The w component.
    pub w: f32,
}

const _: () = assert!(std::mem::size_of::<Vec4>() == 16);
const _: () = assert!(std::mem::align_of::<Vec4>() == 4);

impl Vec4 {
    /// The zero vector.
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0, 0.0);
    /// The vector with all components set to one.
    pub const ONE: Self = Self::new(1.0, 1.0, 1.0, 1.0);
    /// The unit vector along the x axis.
    pub const X: Self = Self::new(1.0, 0.0, 0.0, 0.0);
    /// The unit vector along the y axis.
    pub const Y: Self = Self::new(0.0, 1.0, 0.0, 0.0);
    /// The unit vector along the z axis.
    pub const Z: Self = Self::new(0.0, 0.0, 1.0, 0.0);
    /// The unit vector along the w axis.
    pub const W: Self = Self::new(0.0, 0.0, 0.0, 1.0);
    /// The negative unit vector along the x axis.
    pub const NEG_X: Self = Self::new(-1.0, 0.0, 0.0, 0.0);
    /// The negative unit vector along the y axis.
    pub const NEG_Y: Self = Self::new(0.0, -1.0, 0.0, 0.0);
    /// The negative unit vector along the z axis.
    pub const NEG_Z: Self = Self::new(0.0, 0.0, -1.0, 0.0);
    /// The negative unit vector along the w axis.
    pub const NEG_W: Self = Self::new(0.0, 0.0, 0.0, -1.0);

    /// Creates a new vector from its components.
    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    /// Creates a vector with all components set to `v`.
    pub fn splat(v: f32) -> Self {
        Self::new(v, v, v, v)
    }

    /// Returns the dot product of `self` and `rhs`.
    pub fn dot(self, rhs: Self) -> f32 {
        self.x * rhs.x + self.y * rhs.y + self.z * rhs.z + self.w * rhs.w
    }

    /// Returns the Euclidean length of the vector.
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    /// Returns the squared length of the vector.
    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }

    /// Returns the normalized vector, or `None` if its length is below `1e-12`.
    pub fn normalize(self) -> Option<Self> {
        let len = self.length();
        if len < 1e-12 {
            None
        } else {
            Some(self / len)
        }
    }

    /// Returns the normalized vector, or the zero vector if it cannot be normalized.
    pub fn normalize_or_zero(self) -> Self {
        self.normalize().unwrap_or(Self::ZERO)
    }

    /// Returns the unclamped linear interpolation between `self` and `rhs` by `t`.
    pub fn lerp(self, rhs: Self, t: f32) -> Self {
        self + (rhs - self) * t
    }

    /// Returns the componentwise minimum of `self` and `rhs`.
    pub fn min(self, rhs: Self) -> Self {
        Self::new(
            self.x.min(rhs.x),
            self.y.min(rhs.y),
            self.z.min(rhs.z),
            self.w.min(rhs.w),
        )
    }

    /// Returns the componentwise maximum of `self` and `rhs`.
    pub fn max(self, rhs: Self) -> Self {
        Self::new(
            self.x.max(rhs.x),
            self.y.max(rhs.y),
            self.z.max(rhs.z),
            self.w.max(rhs.w),
        )
    }

    /// Returns `self` clamped componentwise between `min` and `max`.
    pub fn clamp(self, min: Self, max: Self) -> Self {
        self.max(min).min(max)
    }

    /// Returns the componentwise absolute value.
    pub fn abs(self) -> Self {
        Self::new(self.x.abs(), self.y.abs(), self.z.abs(), self.w.abs())
    }

    /// Truncates `self` to a [`Vec3`], dropping the `w` component.
    pub fn truncate(self) -> Vec3 {
        Vec3::new(self.x, self.y, self.z)
    }

    /// Returns the components as an array.
    pub fn as_array(self) -> [f32; 4] {
        [self.x, self.y, self.z, self.w]
    }

    /// Returns `true` if all components are finite.
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite() && self.w.is_finite()
    }
}

impl Add for Vec4 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self::new(
            self.x + rhs.x,
            self.y + rhs.y,
            self.z + rhs.z,
            self.w + rhs.w,
        )
    }
}

impl Sub for Vec4 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self::new(
            self.x - rhs.x,
            self.y - rhs.y,
            self.z - rhs.z,
            self.w - rhs.w,
        )
    }
}

impl Neg for Vec4 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z, -self.w)
    }
}

impl Mul<f32> for Vec4 {
    type Output = Self;
    fn mul(self, rhs: f32) -> Self {
        Self::new(self.x * rhs, self.y * rhs, self.z * rhs, self.w * rhs)
    }
}

impl Mul<Vec4> for f32 {
    type Output = Vec4;
    fn mul(self, rhs: Vec4) -> Vec4 {
        rhs * self
    }
}

impl Div<f32> for Vec4 {
    type Output = Self;
    fn div(self, rhs: f32) -> Self {
        Self::new(self.x / rhs, self.y / rhs, self.z / rhs, self.w / rhs)
    }
}

impl AddAssign for Vec4 {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl SubAssign for Vec4 {
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

impl MulAssign<f32> for Vec4 {
    fn mul_assign(&mut self, rhs: f32) {
        *self = *self * rhs;
    }
}

impl DivAssign<f32> for Vec4 {
    fn div_assign(&mut self, rhs: f32) {
        *self = *self / rhs;
    }
}

impl Index<usize> for Vec4 {
    type Output = f32;
    fn index(&self, index: usize) -> &f32 {
        match index {
            0 => &self.x,
            1 => &self.y,
            2 => &self.z,
            3 => &self.w,
            _ => panic!("index out of bounds: the len is 4 but the index is {index}"),
        }
    }
}

impl IndexMut<usize> for Vec4 {
    fn index_mut(&mut self, index: usize) -> &mut f32 {
        match index {
            0 => &mut self.x,
            1 => &mut self.y,
            2 => &mut self.z,
            3 => &mut self.w,
            _ => panic!("index out of bounds: the len is 4 but the index is {index}"),
        }
    }
}

impl From<[f32; 4]> for Vec4 {
    fn from(v: [f32; 4]) -> Self {
        Self::new(v[0], v[1], v[2], v[3])
    }
}

impl From<(f32, f32, f32, f32)> for Vec4 {
    fn from(v: (f32, f32, f32, f32)) -> Self {
        Self::new(v.0, v.1, v.2, v.3)
    }
}

impl From<Vec4> for [f32; 4] {
    fn from(v: Vec4) -> Self {
        [v.x, v.y, v.z, v.w]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Vec3;
    use crate::traits::ApproxEq;

    #[test]
    fn new_and_splat() {
        assert_eq!(
            Vec4::new(1.0, 2.0, 3.0, 4.0),
            Vec4 {
                x: 1.0,
                y: 2.0,
                z: 3.0,
                w: 4.0
            }
        );
        assert_eq!(Vec4::splat(2.0), Vec4::new(2.0, 2.0, 2.0, 2.0));
    }

    #[test]
    fn constants() {
        assert_eq!(Vec4::ZERO, Vec4::new(0.0, 0.0, 0.0, 0.0));
        assert_eq!(Vec4::ONE, Vec4::new(1.0, 1.0, 1.0, 1.0));
        assert_eq!(Vec4::X, Vec4::new(1.0, 0.0, 0.0, 0.0));
        assert_eq!(Vec4::Y, Vec4::new(0.0, 1.0, 0.0, 0.0));
        assert_eq!(Vec4::Z, Vec4::new(0.0, 0.0, 1.0, 0.0));
        assert_eq!(Vec4::W, Vec4::new(0.0, 0.0, 0.0, 1.0));
        assert_eq!(Vec4::NEG_X, Vec4::new(-1.0, 0.0, 0.0, 0.0));
        assert_eq!(Vec4::NEG_Y, Vec4::new(0.0, -1.0, 0.0, 0.0));
        assert_eq!(Vec4::NEG_Z, Vec4::new(0.0, 0.0, -1.0, 0.0));
        assert_eq!(Vec4::NEG_W, Vec4::new(0.0, 0.0, 0.0, -1.0));
    }

    #[test]
    fn dot() {
        assert!(Vec4::new(1.0, 2.0, 3.0, 4.0)
            .dot(Vec4::new(5.0, 6.0, 7.0, 8.0))
            .approx_eq_default(70.0));
    }

    #[test]
    fn length() {
        assert!(Vec4::new(1.0, 2.0, 2.0, 4.0)
            .length()
            .approx_eq_default(5.0));
        assert!(Vec4::new(1.0, 2.0, 2.0, 4.0)
            .length_squared()
            .approx_eq_default(25.0));
    }

    #[test]
    fn normalize() {
        let n = Vec4::new(0.0, 0.0, 3.0, 4.0).normalize().unwrap();
        assert!(n.approx_eq_default(Vec4::new(0.0, 0.0, 0.6, 0.8)));
        assert!(Vec4::ZERO.normalize().is_none());
        assert!(Vec4::ZERO.normalize_or_zero().approx_eq_default(Vec4::ZERO));
    }

    #[test]
    fn lerp_min_max_clamp_abs() {
        assert!(Vec4::ZERO
            .lerp(Vec4::new(2.0, 4.0, 6.0, 8.0), 0.5)
            .approx_eq_default(Vec4::new(1.0, 2.0, 3.0, 4.0)));
        assert_eq!(
            Vec4::new(1.0, 4.0, 2.0, 5.0).min(Vec4::new(3.0, 2.0, 5.0, 1.0)),
            Vec4::new(1.0, 2.0, 2.0, 1.0)
        );
        assert_eq!(
            Vec4::new(1.0, 4.0, 2.0, 5.0).max(Vec4::new(3.0, 2.0, 5.0, 1.0)),
            Vec4::new(3.0, 4.0, 5.0, 5.0)
        );
        assert_eq!(
            Vec4::new(-1.0, 5.0, 1.0, 3.0).clamp(Vec4::ZERO, Vec4::new(2.0, 2.0, 2.0, 2.0)),
            Vec4::new(0.0, 2.0, 1.0, 2.0)
        );
        assert_eq!(
            Vec4::new(-1.0, -2.0, 3.0, -4.0).abs(),
            Vec4::new(1.0, 2.0, 3.0, 4.0)
        );
    }

    #[test]
    fn truncate_as_array_is_finite() {
        assert_eq!(
            Vec4::new(1.0, 2.0, 3.0, 4.0).truncate(),
            Vec3::new(1.0, 2.0, 3.0)
        );
        assert_eq!(
            Vec4::new(1.0, 2.0, 3.0, 4.0).as_array(),
            [1.0, 2.0, 3.0, 4.0]
        );
        assert!(Vec4::new(1.0, 2.0, 3.0, 4.0).is_finite());
        assert!(!Vec4::new(1.0, f32::NAN, 3.0, 4.0).is_finite());
    }

    #[test]
    fn operators() {
        assert_eq!(
            Vec4::new(1.0, 2.0, 3.0, 4.0) + Vec4::new(5.0, 6.0, 7.0, 8.0),
            Vec4::new(6.0, 8.0, 10.0, 12.0)
        );
        assert_eq!(
            Vec4::new(5.0, 6.0, 7.0, 8.0) - Vec4::new(1.0, 2.0, 3.0, 4.0),
            Vec4::new(4.0, 4.0, 4.0, 4.0)
        );
        assert_eq!(
            -Vec4::new(1.0, 2.0, 3.0, 4.0),
            Vec4::new(-1.0, -2.0, -3.0, -4.0)
        );
        assert_eq!(
            Vec4::new(1.0, 2.0, 3.0, 4.0) * 2.0,
            Vec4::new(2.0, 4.0, 6.0, 8.0)
        );
        assert_eq!(
            2.0 * Vec4::new(1.0, 2.0, 3.0, 4.0),
            Vec4::new(2.0, 4.0, 6.0, 8.0)
        );
        assert_eq!(
            Vec4::new(2.0, 4.0, 6.0, 8.0) / 2.0,
            Vec4::new(1.0, 2.0, 3.0, 4.0)
        );
    }

    #[test]
    fn assign_operators() {
        let mut v = Vec4::new(1.0, 2.0, 3.0, 4.0);
        v += Vec4::ONE;
        assert_eq!(v, Vec4::new(2.0, 3.0, 4.0, 5.0));
        v -= Vec4::ONE;
        assert_eq!(v, Vec4::new(1.0, 2.0, 3.0, 4.0));
        v *= 2.0;
        assert_eq!(v, Vec4::new(2.0, 4.0, 6.0, 8.0));
        v /= 2.0;
        assert_eq!(v, Vec4::new(1.0, 2.0, 3.0, 4.0));
    }

    #[test]
    fn indexing() {
        let mut v = Vec4::new(1.0, 2.0, 3.0, 4.0);
        assert_eq!(v[0], 1.0);
        assert_eq!(v[1], 2.0);
        assert_eq!(v[2], 3.0);
        assert_eq!(v[3], 4.0);
        v[3] = 9.0;
        assert_eq!(v[3], 9.0);
    }

    #[test]
    #[should_panic(expected = "index out of bounds")]
    fn index_out_of_bounds() {
        let _ = Vec4::ZERO[4];
    }

    #[test]
    fn conversions() {
        assert_eq!(
            Vec4::from([1.0, 2.0, 3.0, 4.0]),
            Vec4::new(1.0, 2.0, 3.0, 4.0)
        );
        assert_eq!(
            Vec4::from((1.0, 2.0, 3.0, 4.0)),
            Vec4::new(1.0, 2.0, 3.0, 4.0)
        );
        let a: [f32; 4] = Vec4::new(1.0, 2.0, 3.0, 4.0).into();
        assert_eq!(a, [1.0, 2.0, 3.0, 4.0]);
    }
}
