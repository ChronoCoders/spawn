use core::ops::{
    Add, AddAssign, Div, DivAssign, Index, IndexMut, Mul, MulAssign, Neg, Sub, SubAssign,
};

use crate::math::Vec3;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

const _: () = assert!(std::mem::size_of::<Vec2>() == 8);
const _: () = assert!(std::mem::align_of::<Vec2>() == 4);

impl Vec2 {
    pub const ZERO: Self = Self::new(0.0, 0.0);
    pub const ONE: Self = Self::new(1.0, 1.0);
    pub const X: Self = Self::new(1.0, 0.0);
    pub const Y: Self = Self::new(0.0, 1.0);
    pub const NEG_X: Self = Self::new(-1.0, 0.0);
    pub const NEG_Y: Self = Self::new(0.0, -1.0);

    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn splat(v: f32) -> Self {
        Self::new(v, v)
    }

    pub fn dot(self, rhs: Self) -> f32 {
        self.x * rhs.x + self.y * rhs.y
    }

    /// 90° CCW rotation: `(-y, x)`.
    pub fn perp(self) -> Self {
        Self::new(-self.y, self.x)
    }

    /// 2D cross product (z of the 3D cross product).
    pub fn perp_dot(self, rhs: Self) -> f32 {
        self.x * rhs.y - self.y * rhs.x
    }

    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }

    pub fn distance(self, rhs: Self) -> f32 {
        (self - rhs).length()
    }

    pub fn distance_squared(self, rhs: Self) -> f32 {
        (self - rhs).length_squared()
    }

    /// `None` if length is below `1e-12`.
    pub fn normalize(self) -> Option<Self> {
        let len = self.length();
        if len < 1e-12 {
            None
        } else {
            Some(self / len)
        }
    }

    pub fn normalize_or_zero(self) -> Self {
        self.normalize().unwrap_or(Self::ZERO)
    }

    /// Unclamped.
    pub fn lerp(self, rhs: Self, t: f32) -> Self {
        self + (rhs - self) * t
    }

    pub fn min(self, rhs: Self) -> Self {
        Self::new(self.x.min(rhs.x), self.y.min(rhs.y))
    }

    pub fn max(self, rhs: Self) -> Self {
        Self::new(self.x.max(rhs.x), self.y.max(rhs.y))
    }

    pub fn clamp(self, min: Self, max: Self) -> Self {
        self.max(min).min(max)
    }

    pub fn abs(self) -> Self {
        Self::new(self.x.abs(), self.y.abs())
    }

    pub fn extend(self, z: f32) -> Vec3 {
        Vec3::new(self.x, self.y, z)
    }

    pub fn as_array(self) -> [f32; 2] {
        [self.x, self.y]
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

impl Add for Vec2 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for Vec2 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Neg for Vec2 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y)
    }
}

impl Mul<f32> for Vec2 {
    type Output = Self;
    fn mul(self, rhs: f32) -> Self {
        Self::new(self.x * rhs, self.y * rhs)
    }
}

impl Mul<Vec2> for f32 {
    type Output = Vec2;
    fn mul(self, rhs: Vec2) -> Vec2 {
        rhs * self
    }
}

impl Div<f32> for Vec2 {
    type Output = Self;
    fn div(self, rhs: f32) -> Self {
        Self::new(self.x / rhs, self.y / rhs)
    }
}

impl AddAssign for Vec2 {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl SubAssign for Vec2 {
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

impl MulAssign<f32> for Vec2 {
    fn mul_assign(&mut self, rhs: f32) {
        *self = *self * rhs;
    }
}

impl DivAssign<f32> for Vec2 {
    fn div_assign(&mut self, rhs: f32) {
        *self = *self / rhs;
    }
}

impl Index<usize> for Vec2 {
    type Output = f32;
    /// # Panics
    ///
    /// Panics if `index` is out of range (must be `0` or `1`).
    fn index(&self, index: usize) -> &f32 {
        match index {
            0 => &self.x,
            1 => &self.y,
            _ => panic!("index out of bounds: the len is 2 but the index is {index}"),
        }
    }
}

impl IndexMut<usize> for Vec2 {
    /// # Panics
    ///
    /// Panics if `index` is out of range (must be `0` or `1`).
    fn index_mut(&mut self, index: usize) -> &mut f32 {
        match index {
            0 => &mut self.x,
            1 => &mut self.y,
            _ => panic!("index out of bounds: the len is 2 but the index is {index}"),
        }
    }
}

impl From<[f32; 2]> for Vec2 {
    fn from(v: [f32; 2]) -> Self {
        Self::new(v[0], v[1])
    }
}

impl From<(f32, f32)> for Vec2 {
    fn from(v: (f32, f32)) -> Self {
        Self::new(v.0, v.1)
    }
}

impl From<Vec2> for [f32; 2] {
    fn from(v: Vec2) -> Self {
        [v.x, v.y]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Vec3;
    use crate::traits::ApproxEq;

    #[test]
    fn new_and_splat() {
        assert_eq!(Vec2::new(1.0, 2.0), Vec2 { x: 1.0, y: 2.0 });
        assert_eq!(Vec2::splat(3.0), Vec2::new(3.0, 3.0));
    }

    #[test]
    fn constants() {
        assert_eq!(Vec2::ZERO, Vec2::new(0.0, 0.0));
        assert_eq!(Vec2::ONE, Vec2::new(1.0, 1.0));
        assert_eq!(Vec2::X, Vec2::new(1.0, 0.0));
        assert_eq!(Vec2::Y, Vec2::new(0.0, 1.0));
        assert_eq!(Vec2::NEG_X, Vec2::new(-1.0, 0.0));
        assert_eq!(Vec2::NEG_Y, Vec2::new(0.0, -1.0));
    }

    #[test]
    fn dot_perp() {
        assert!(Vec2::new(1.0, 2.0)
            .dot(Vec2::new(3.0, 4.0))
            .approx_eq_default(11.0));
        assert_eq!(Vec2::X.perp(), Vec2::Y);
        assert!(Vec2::new(1.0, 0.0)
            .perp_dot(Vec2::new(0.0, 1.0))
            .approx_eq_default(1.0));
    }

    #[test]
    fn length_distance() {
        assert!(Vec2::new(3.0, 4.0).length().approx_eq_default(5.0));
        assert!(Vec2::new(3.0, 4.0).length_squared().approx_eq_default(25.0));
        assert!(Vec2::new(0.0, 0.0)
            .distance(Vec2::new(3.0, 4.0))
            .approx_eq_default(5.0));
        assert!(Vec2::new(0.0, 0.0)
            .distance_squared(Vec2::new(3.0, 4.0))
            .approx_eq_default(25.0));
    }

    #[test]
    fn normalize() {
        let n = Vec2::new(3.0, 4.0).normalize().unwrap();
        assert!(n.approx_eq_default(Vec2::new(0.6, 0.8)));
        assert!(Vec2::ZERO.normalize().is_none());
        assert!(Vec2::ZERO.normalize_or_zero().approx_eq_default(Vec2::ZERO));
        assert!(Vec2::new(3.0, 4.0)
            .normalize_or_zero()
            .approx_eq_default(Vec2::new(0.6, 0.8)));
    }

    #[test]
    fn lerp_min_max_clamp_abs() {
        assert!(Vec2::ZERO
            .lerp(Vec2::new(2.0, 4.0), 0.5)
            .approx_eq_default(Vec2::new(1.0, 2.0)));
        assert_eq!(
            Vec2::new(1.0, 4.0).min(Vec2::new(3.0, 2.0)),
            Vec2::new(1.0, 2.0)
        );
        assert_eq!(
            Vec2::new(1.0, 4.0).max(Vec2::new(3.0, 2.0)),
            Vec2::new(3.0, 4.0)
        );
        assert_eq!(
            Vec2::new(-1.0, 5.0).clamp(Vec2::ZERO, Vec2::new(2.0, 2.0)),
            Vec2::new(0.0, 2.0)
        );
        assert_eq!(Vec2::new(-1.0, -2.0).abs(), Vec2::new(1.0, 2.0));
    }

    #[test]
    fn extend_as_array_is_finite() {
        assert_eq!(Vec2::new(1.0, 2.0).extend(3.0), Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(Vec2::new(1.0, 2.0).as_array(), [1.0, 2.0]);
        assert!(Vec2::new(1.0, 2.0).is_finite());
        assert!(!Vec2::new(f32::NAN, 2.0).is_finite());
    }

    #[test]
    fn operators() {
        assert_eq!(
            Vec2::new(1.0, 2.0) + Vec2::new(3.0, 4.0),
            Vec2::new(4.0, 6.0)
        );
        assert_eq!(
            Vec2::new(3.0, 4.0) - Vec2::new(1.0, 2.0),
            Vec2::new(2.0, 2.0)
        );
        assert_eq!(-Vec2::new(1.0, 2.0), Vec2::new(-1.0, -2.0));
        assert_eq!(Vec2::new(1.0, 2.0) * 2.0, Vec2::new(2.0, 4.0));
        assert_eq!(2.0 * Vec2::new(1.0, 2.0), Vec2::new(2.0, 4.0));
        assert_eq!(Vec2::new(2.0, 4.0) / 2.0, Vec2::new(1.0, 2.0));
    }

    #[test]
    fn assign_operators() {
        let mut v = Vec2::new(1.0, 2.0);
        v += Vec2::new(1.0, 1.0);
        assert_eq!(v, Vec2::new(2.0, 3.0));
        v -= Vec2::new(1.0, 1.0);
        assert_eq!(v, Vec2::new(1.0, 2.0));
        v *= 2.0;
        assert_eq!(v, Vec2::new(2.0, 4.0));
        v /= 2.0;
        assert_eq!(v, Vec2::new(1.0, 2.0));
    }

    #[test]
    fn indexing() {
        let mut v = Vec2::new(1.0, 2.0);
        assert_eq!(v[0], 1.0);
        assert_eq!(v[1], 2.0);
        v[0] = 5.0;
        assert_eq!(v[0], 5.0);
    }

    #[test]
    #[should_panic(expected = "index out of bounds")]
    fn index_out_of_bounds() {
        let _ = Vec2::ZERO[2];
    }

    #[test]
    fn conversions() {
        assert_eq!(Vec2::from([1.0, 2.0]), Vec2::new(1.0, 2.0));
        assert_eq!(Vec2::from((1.0, 2.0)), Vec2::new(1.0, 2.0));
        let a: [f32; 2] = Vec2::new(1.0, 2.0).into();
        assert_eq!(a, [1.0, 2.0]);
    }
}
