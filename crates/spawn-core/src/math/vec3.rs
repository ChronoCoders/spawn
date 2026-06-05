use core::ops::{
    Add, AddAssign, Div, DivAssign, Index, IndexMut, Mul, MulAssign, Neg, Sub, SubAssign,
};

use crate::math::{Vec2, Vec4};

/// A 3-component vector of `f32`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    /// The x component.
    pub x: f32,
    /// The y component.
    pub y: f32,
    /// The z component.
    pub z: f32,
}

const _: () = assert!(std::mem::size_of::<Vec3>() == 12);
const _: () = assert!(std::mem::align_of::<Vec3>() == 4);

impl Vec3 {
    /// The zero vector.
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);
    /// The vector with all components set to one.
    pub const ONE: Self = Self::new(1.0, 1.0, 1.0);
    /// The unit vector along the x axis.
    pub const X: Self = Self::new(1.0, 0.0, 0.0);
    /// The unit vector along the y axis.
    pub const Y: Self = Self::new(0.0, 1.0, 0.0);
    /// The unit vector along the z axis.
    pub const Z: Self = Self::new(0.0, 0.0, 1.0);
    /// The negative unit vector along the x axis.
    pub const NEG_X: Self = Self::new(-1.0, 0.0, 0.0);
    /// The negative unit vector along the y axis.
    pub const NEG_Y: Self = Self::new(0.0, -1.0, 0.0);
    /// The negative unit vector along the z axis.
    pub const NEG_Z: Self = Self::new(0.0, 0.0, -1.0);

    /// Creates a new vector from its components.
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    /// Creates a vector with all components set to `v`.
    pub fn splat(v: f32) -> Self {
        Self::new(v, v, v)
    }

    /// Returns the dot product of `self` and `rhs`.
    pub fn dot(self, rhs: Self) -> f32 {
        self.x * rhs.x + self.y * rhs.y + self.z * rhs.z
    }

    /// Returns the cross product of `self` and `rhs`.
    pub fn cross(self, rhs: Self) -> Self {
        Self::new(
            self.y * rhs.z - self.z * rhs.y,
            self.z * rhs.x - self.x * rhs.z,
            self.x * rhs.y - self.y * rhs.x,
        )
    }

    /// Returns the Euclidean length of the vector.
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    /// Returns the squared length of the vector.
    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }

    /// Returns the Euclidean distance between `self` and `rhs`.
    pub fn distance(self, rhs: Self) -> f32 {
        (self - rhs).length()
    }

    /// Returns the squared distance between `self` and `rhs`.
    pub fn distance_squared(self, rhs: Self) -> f32 {
        (self - rhs).length_squared()
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
        Self::new(self.x.min(rhs.x), self.y.min(rhs.y), self.z.min(rhs.z))
    }

    /// Returns the componentwise maximum of `self` and `rhs`.
    pub fn max(self, rhs: Self) -> Self {
        Self::new(self.x.max(rhs.x), self.y.max(rhs.y), self.z.max(rhs.z))
    }

    /// Returns `self` clamped componentwise between `min` and `max`.
    pub fn clamp(self, min: Self, max: Self) -> Self {
        self.max(min).min(max)
    }

    /// Returns the componentwise absolute value.
    pub fn abs(self) -> Self {
        Self::new(self.x.abs(), self.y.abs(), self.z.abs())
    }

    /// Extends `self` to a [`Vec4`] with the given `w` component.
    pub fn extend(self, w: f32) -> Vec4 {
        Vec4::new(self.x, self.y, self.z, w)
    }

    /// Truncates `self` to a [`Vec2`], dropping the `z` component.
    pub fn truncate(self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }

    /// Returns the projection of `self` onto `rhs`, or `None` if `rhs` is near zero.
    pub fn project_onto(self, rhs: Self) -> Option<Self> {
        let len_sq = rhs.length_squared();
        if len_sq < 1e-12 {
            None
        } else {
            Some(rhs * (self.dot(rhs) / len_sq))
        }
    }

    /// Reflects `self` across the plane defined by `normal`.
    ///
    /// `normal` is assumed to be unit length.
    pub fn reflect(self, normal: Self) -> Self {
        self - normal * (2.0 * self.dot(normal))
    }

    /// Returns the components as an array.
    pub fn as_array(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }

    /// Returns `true` if all components are finite.
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

impl Add for Vec3 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z)
    }
}

impl Sub for Vec3 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
}

impl Neg for Vec3 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

impl Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, rhs: f32) -> Self {
        Self::new(self.x * rhs, self.y * rhs, self.z * rhs)
    }
}

impl Mul<Vec3> for f32 {
    type Output = Vec3;
    fn mul(self, rhs: Vec3) -> Vec3 {
        rhs * self
    }
}

impl Div<f32> for Vec3 {
    type Output = Self;
    fn div(self, rhs: f32) -> Self {
        Self::new(self.x / rhs, self.y / rhs, self.z / rhs)
    }
}

impl AddAssign for Vec3 {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl SubAssign for Vec3 {
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

impl MulAssign<f32> for Vec3 {
    fn mul_assign(&mut self, rhs: f32) {
        *self = *self * rhs;
    }
}

impl DivAssign<f32> for Vec3 {
    fn div_assign(&mut self, rhs: f32) {
        *self = *self / rhs;
    }
}

impl Index<usize> for Vec3 {
    type Output = f32;
    fn index(&self, index: usize) -> &f32 {
        match index {
            0 => &self.x,
            1 => &self.y,
            2 => &self.z,
            _ => panic!("index out of bounds: the len is 3 but the index is {index}"),
        }
    }
}

impl IndexMut<usize> for Vec3 {
    fn index_mut(&mut self, index: usize) -> &mut f32 {
        match index {
            0 => &mut self.x,
            1 => &mut self.y,
            2 => &mut self.z,
            _ => panic!("index out of bounds: the len is 3 but the index is {index}"),
        }
    }
}

impl From<[f32; 3]> for Vec3 {
    fn from(v: [f32; 3]) -> Self {
        Self::new(v[0], v[1], v[2])
    }
}

impl From<(f32, f32, f32)> for Vec3 {
    fn from(v: (f32, f32, f32)) -> Self {
        Self::new(v.0, v.1, v.2)
    }
}

impl From<Vec3> for [f32; 3] {
    fn from(v: Vec3) -> Self {
        [v.x, v.y, v.z]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{Vec2, Vec4};
    use crate::traits::ApproxEq;

    #[test]
    fn new_and_splat() {
        assert_eq!(
            Vec3::new(1.0, 2.0, 3.0),
            Vec3 {
                x: 1.0,
                y: 2.0,
                z: 3.0
            }
        );
        assert_eq!(Vec3::splat(3.0), Vec3::new(3.0, 3.0, 3.0));
    }

    #[test]
    fn constants() {
        assert_eq!(Vec3::ZERO, Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(Vec3::ONE, Vec3::new(1.0, 1.0, 1.0));
        assert_eq!(Vec3::X, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(Vec3::Y, Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(Vec3::Z, Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(Vec3::NEG_X, Vec3::new(-1.0, 0.0, 0.0));
        assert_eq!(Vec3::NEG_Y, Vec3::new(0.0, -1.0, 0.0));
        assert_eq!(Vec3::NEG_Z, Vec3::new(0.0, 0.0, -1.0));
    }

    #[test]
    fn dot_cross() {
        assert!(Vec3::new(1.0, 2.0, 3.0)
            .dot(Vec3::new(4.0, 5.0, 6.0))
            .approx_eq_default(32.0));
        assert!(Vec3::X.cross(Vec3::Y).approx_eq_default(Vec3::Z));
    }

    #[test]
    fn length_distance() {
        assert!(Vec3::new(2.0, 3.0, 6.0).length().approx_eq_default(7.0));
        assert!(Vec3::new(2.0, 3.0, 6.0)
            .length_squared()
            .approx_eq_default(49.0));
        assert!(Vec3::ZERO
            .distance(Vec3::new(2.0, 3.0, 6.0))
            .approx_eq_default(7.0));
        assert!(Vec3::ZERO
            .distance_squared(Vec3::new(2.0, 3.0, 6.0))
            .approx_eq_default(49.0));
    }

    #[test]
    fn normalize() {
        let n = Vec3::new(0.0, 3.0, 4.0).normalize().unwrap();
        assert!(n.approx_eq_default(Vec3::new(0.0, 0.6, 0.8)));
        assert!(Vec3::ZERO.normalize().is_none());
        assert!(Vec3::ZERO.normalize_or_zero().approx_eq_default(Vec3::ZERO));
    }

    #[test]
    fn lerp_min_max_clamp_abs() {
        assert!(Vec3::ZERO
            .lerp(Vec3::new(2.0, 4.0, 6.0), 0.5)
            .approx_eq_default(Vec3::new(1.0, 2.0, 3.0)));
        assert_eq!(
            Vec3::new(1.0, 4.0, 2.0).min(Vec3::new(3.0, 2.0, 5.0)),
            Vec3::new(1.0, 2.0, 2.0)
        );
        assert_eq!(
            Vec3::new(1.0, 4.0, 2.0).max(Vec3::new(3.0, 2.0, 5.0)),
            Vec3::new(3.0, 4.0, 5.0)
        );
        assert_eq!(
            Vec3::new(-1.0, 5.0, 1.0).clamp(Vec3::ZERO, Vec3::new(2.0, 2.0, 2.0)),
            Vec3::new(0.0, 2.0, 1.0)
        );
        assert_eq!(Vec3::new(-1.0, -2.0, 3.0).abs(), Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn extend_truncate() {
        assert_eq!(
            Vec3::new(1.0, 2.0, 3.0).extend(4.0),
            Vec4::new(1.0, 2.0, 3.0, 4.0)
        );
        assert_eq!(Vec3::new(1.0, 2.0, 3.0).truncate(), Vec2::new(1.0, 2.0));
    }

    #[test]
    fn project_onto() {
        let p = Vec3::new(2.0, 2.0, 0.0).project_onto(Vec3::X).unwrap();
        assert!(p.approx_eq_default(Vec3::new(2.0, 0.0, 0.0)));
        assert!(Vec3::ONE.project_onto(Vec3::ZERO).is_none());
    }

    #[test]
    fn reflect() {
        let r = Vec3::new(1.0, -1.0, 0.0).reflect(Vec3::Y);
        assert!(r.approx_eq_default(Vec3::new(1.0, 1.0, 0.0)));
    }

    #[test]
    fn as_array_is_finite() {
        assert_eq!(Vec3::new(1.0, 2.0, 3.0).as_array(), [1.0, 2.0, 3.0]);
        assert!(Vec3::new(1.0, 2.0, 3.0).is_finite());
        assert!(!Vec3::new(f32::INFINITY, 2.0, 3.0).is_finite());
    }

    #[test]
    fn operators() {
        assert_eq!(
            Vec3::new(1.0, 2.0, 3.0) + Vec3::new(4.0, 5.0, 6.0),
            Vec3::new(5.0, 7.0, 9.0)
        );
        assert_eq!(
            Vec3::new(4.0, 5.0, 6.0) - Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(3.0, 3.0, 3.0)
        );
        assert_eq!(-Vec3::new(1.0, 2.0, 3.0), Vec3::new(-1.0, -2.0, -3.0));
        assert_eq!(Vec3::new(1.0, 2.0, 3.0) * 2.0, Vec3::new(2.0, 4.0, 6.0));
        assert_eq!(2.0 * Vec3::new(1.0, 2.0, 3.0), Vec3::new(2.0, 4.0, 6.0));
        assert_eq!(Vec3::new(2.0, 4.0, 6.0) / 2.0, Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn assign_operators() {
        let mut v = Vec3::new(1.0, 2.0, 3.0);
        v += Vec3::ONE;
        assert_eq!(v, Vec3::new(2.0, 3.0, 4.0));
        v -= Vec3::ONE;
        assert_eq!(v, Vec3::new(1.0, 2.0, 3.0));
        v *= 2.0;
        assert_eq!(v, Vec3::new(2.0, 4.0, 6.0));
        v /= 2.0;
        assert_eq!(v, Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn indexing() {
        let mut v = Vec3::new(1.0, 2.0, 3.0);
        assert_eq!(v[0], 1.0);
        assert_eq!(v[1], 2.0);
        assert_eq!(v[2], 3.0);
        v[2] = 5.0;
        assert_eq!(v[2], 5.0);
    }

    #[test]
    #[should_panic(expected = "index out of bounds")]
    fn index_out_of_bounds() {
        let _ = Vec3::ZERO[3];
    }

    #[test]
    fn conversions() {
        assert_eq!(Vec3::from([1.0, 2.0, 3.0]), Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(Vec3::from((1.0, 2.0, 3.0)), Vec3::new(1.0, 2.0, 3.0));
        let a: [f32; 3] = Vec3::new(1.0, 2.0, 3.0).into();
        assert_eq!(a, [1.0, 2.0, 3.0]);
    }
}
