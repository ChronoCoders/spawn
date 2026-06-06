//! Unit-quaternion rotation type. Right-handed, radians, Hamilton product, `w` is the scalar part.

use crate::math::Vec3;
use std::ops::{Mul, Neg};

/// A quaternion `x*i + y*j + z*k + w`, where `w` is the scalar part.
///
/// Rotations are represented by unit quaternions. Methods documented as assuming a
/// unit quaternion (e.g. [`Quat::rotate`]) produce undefined results otherwise.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    pub const IDENTITY: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };

    pub const fn from_xyzw(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    /// Normalizes `axis` internally. `None` if `axis` is near zero (length below `1e-12`).
    pub fn from_axis_angle(axis: Vec3, radians: f32) -> Option<Self> {
        let axis = axis.normalize()?;
        let half = radians * 0.5;
        let s = half.sin();
        Some(Self {
            x: axis.x * s,
            y: axis.y * s,
            z: axis.z * s,
            w: half.cos(),
        })
    }

    pub fn from_rotation_x(radians: f32) -> Self {
        let half = radians * 0.5;
        Self {
            x: half.sin(),
            y: 0.0,
            z: 0.0,
            w: half.cos(),
        }
    }

    pub fn from_rotation_y(radians: f32) -> Self {
        let half = radians * 0.5;
        Self {
            x: 0.0,
            y: half.sin(),
            z: 0.0,
            w: half.cos(),
        }
    }

    pub fn from_rotation_z(radians: f32) -> Self {
        let half = radians * 0.5;
        Self {
            x: 0.0,
            y: 0.0,
            z: half.sin(),
            w: half.cos(),
        }
    }

    /// Intrinsic XYZ: rotate about body X, then the new body Y, then the new body Z.
    /// For column vectors this composes as `from_rotation_x(x) * from_rotation_y(y)
    /// * from_rotation_z(z)`, i.e. the Z rotation is applied to a vector first.
    pub fn from_euler_xyz(x: f32, y: f32, z: f32) -> Self {
        Self::from_rotation_x(x) * Self::from_rotation_y(y) * Self::from_rotation_z(z)
    }

    /// Angle in `[0, PI]`, axis unit length. Identity (or near-identity) returns `(Vec3::X, 0.0)`.
    pub fn to_axis_angle(self) -> (Vec3, f32) {
        let q = self.normalize().unwrap_or(Self::IDENTITY);
        let w = q.w.clamp(-1.0, 1.0);
        let sin_half_sq = 1.0 - w * w;
        if sin_half_sq <= 1e-12 {
            return (Vec3::X, 0.0);
        }
        let sin_half = sin_half_sq.sqrt();
        let angle = 2.0 * w.acos();
        let axis = Vec3::new(q.x / sin_half, q.y / sin_half, q.z / sin_half);
        (axis, angle)
    }

    pub fn dot(self, rhs: Self) -> f32 {
        self.x * rhs.x + self.y * rhs.y + self.z * rhs.z + self.w * rhs.w
    }

    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }

    /// `None` if length is below `1e-12`.
    pub fn normalize(self) -> Option<Self> {
        let len = self.length();
        if len < 1e-12 {
            None
        } else {
            let inv = 1.0 / len;
            Some(Self {
                x: self.x * inv,
                y: self.y * inv,
                z: self.z * inv,
                w: self.w * inv,
            })
        }
    }

    /// For a unit quaternion this equals the inverse rotation.
    pub fn conjugate(self) -> Self {
        Self {
            x: -self.x,
            y: -self.y,
            z: -self.z,
            w: self.w,
        }
    }

    /// `conjugate / length²`; `None` if near zero.
    pub fn inverse(self) -> Option<Self> {
        let len_sq = self.length_squared();
        if len_sq < 1e-12 {
            None
        } else {
            let inv = 1.0 / len_sq;
            let c = self.conjugate();
            Some(Self {
                x: c.x * inv,
                y: c.y * inv,
                z: c.z * inv,
                w: c.w * inv,
            })
        }
    }

    /// Shortest path. Falls back to [`Quat::nlerp`] when the endpoints are nearly
    /// parallel (`|dot| > 0.9995`) to avoid division by a near-zero `sin`.
    pub fn slerp(self, rhs: Self, t: f32) -> Self {
        let mut dot = self.dot(rhs);
        let mut end = rhs;
        if dot < 0.0 {
            end = -end;
            dot = -dot;
        }
        if dot > 0.9995 {
            return self.nlerp(end, t);
        }
        let theta_0 = dot.clamp(-1.0, 1.0).acos();
        let theta = theta_0 * t;
        let sin_theta_0 = theta_0.sin();
        let sin_theta = theta.sin();
        let s0 = theta.cos() - dot * (sin_theta / sin_theta_0);
        let s1 = sin_theta / sin_theta_0;
        Self {
            x: self.x * s0 + end.x * s1,
            y: self.y * s0 + end.y * s1,
            z: self.z * s0 + end.z * s1,
            w: self.w * s0 + end.w * s1,
        }
    }

    /// Shortest path.
    pub fn nlerp(self, rhs: Self, t: f32) -> Self {
        let mut end = rhs;
        if self.dot(rhs) < 0.0 {
            end = -end;
        }
        let result = Self {
            x: self.x + (end.x - self.x) * t,
            y: self.y + (end.y - self.y) * t,
            z: self.z + (end.z - self.z) * t,
            w: self.w + (end.w - self.w) * t,
        };
        result.normalize().unwrap_or(Self::IDENTITY)
    }

    /// Assumes a unit quaternion.
    pub fn rotate(self, v: Vec3) -> Vec3 {
        let u = Vec3::new(self.x, self.y, self.z);
        let t = u.cross(v) * 2.0;
        v + t * self.w + u.cross(t)
    }

    /// `|len² − 1| < 1e-4`.
    pub fn is_normalized(self) -> bool {
        (self.length_squared() - 1.0).abs() < 1e-4
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite() && self.w.is_finite()
    }
}

impl Default for Quat {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Mul<Quat> for Quat {
    type Output = Quat;

    /// Hamilton product: applies `rhs` first, then `self`.
    fn mul(self, rhs: Quat) -> Quat {
        Quat {
            x: self.w * rhs.x + self.x * rhs.w + self.y * rhs.z - self.z * rhs.y,
            y: self.w * rhs.y - self.x * rhs.z + self.y * rhs.w + self.z * rhs.x,
            z: self.w * rhs.z + self.x * rhs.y - self.y * rhs.x + self.z * rhs.w,
            w: self.w * rhs.w - self.x * rhs.x - self.y * rhs.y - self.z * rhs.z,
        }
    }
}

impl Mul<Vec3> for Quat {
    type Output = Vec3;

    /// Assumes a unit quaternion.
    fn mul(self, rhs: Vec3) -> Vec3 {
        self.rotate(rhs)
    }
}

impl Neg for Quat {
    type Output = Quat;

    fn neg(self) -> Quat {
        Quat {
            x: -self.x,
            y: -self.y,
            z: -self.z,
            w: -self.w,
        }
    }
}

const _: () = assert!(std::mem::size_of::<Quat>() == 16);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Mat3;
    use crate::traits::ApproxEq;
    use std::f32::consts::PI;

    #[test]
    fn from_xyzw_and_identity() {
        let q = Quat::from_xyzw(1.0, 2.0, 3.0, 4.0);
        assert_eq!((q.x, q.y, q.z, q.w), (1.0, 2.0, 3.0, 4.0));
        assert_eq!(Quat::default(), Quat::IDENTITY);
    }

    #[test]
    fn from_axis_angle_near_zero_is_none() {
        assert!(Quat::from_axis_angle(Vec3::ZERO, 1.0).is_none());
    }

    #[test]
    fn from_axis_angle_normalizes_axis() {
        let q = Quat::from_axis_angle(Vec3::new(0.0, 0.0, 5.0), PI / 2.0).unwrap();
        let expected = Quat::from_rotation_z(PI / 2.0);
        assert!(q.approx_eq_default(expected));
    }

    #[test]
    fn rotate_matches_mat3_from_quat() {
        let cases = [
            (Vec3::X, 0.3_f32),
            (Vec3::Y, 1.2),
            (Vec3::Z, -0.7),
            (Vec3::new(1.0, 2.0, 3.0), 2.4),
            (Vec3::new(-2.0, 0.5, 1.0), -1.1),
        ];
        let v = Vec3::new(0.7, -1.3, 2.1);
        for (axis, angle) in cases {
            let q = Quat::from_axis_angle(axis, angle).unwrap();
            let by_quat = q.rotate(v);
            let by_mat = Mat3::from_quat(q) * v;
            assert!(by_quat.approx_eq(by_mat, 1e-5));
        }
    }

    #[test]
    fn rotate_by_identity_is_identity() {
        let v = Vec3::new(1.0, -2.0, 3.0);
        assert!(Quat::IDENTITY.rotate(v).approx_eq_default(v));
        assert!((Quat::IDENTITY * v).approx_eq_default(v));
    }

    #[test]
    fn hamilton_product_composes() {
        let qx = Quat::from_rotation_x(0.5);
        let qy = Quat::from_rotation_y(0.9);
        let v = Vec3::new(0.4, 1.1, -0.6);
        let composed = (qx * qy).rotate(v);
        let sequential = qx.rotate(qy.rotate(v));
        assert!(composed.approx_eq(sequential, 1e-5));
    }

    #[test]
    fn from_euler_xyz_matches_composition() {
        let (x, y, z) = (0.3_f32, -0.8, 1.4);
        let q = Quat::from_euler_xyz(x, y, z);
        let composed =
            Quat::from_rotation_x(x) * Quat::from_rotation_y(y) * Quat::from_rotation_z(z);
        assert!(q.approx_eq_default(composed));

        let v = Vec3::new(1.0, -0.5, 0.25);
        let by_euler = q.rotate(v);
        let by_steps = Quat::from_rotation_x(x)
            .rotate(Quat::from_rotation_y(y).rotate(Quat::from_rotation_z(z).rotate(v)));
        assert!(by_euler.approx_eq(by_steps, 1e-5));
    }

    #[test]
    fn to_axis_angle_roundtrip() {
        let axis = Vec3::new(1.0, 2.0, -1.0).normalize().unwrap();
        let q = Quat::from_axis_angle(axis, 1.3).unwrap();
        let (out_axis, out_angle) = q.to_axis_angle();
        assert!(out_angle.approx_eq(1.3, 1e-5));
        assert!(out_axis.approx_eq(axis, 1e-5));
    }

    #[test]
    fn to_axis_angle_identity() {
        let (axis, angle) = Quat::IDENTITY.to_axis_angle();
        assert!(axis.approx_eq_default(Vec3::X));
        assert!(angle.approx_eq_default(0.0));
    }

    #[test]
    fn dot_length_and_squared() {
        let q = Quat::from_xyzw(0.0, 0.0, 0.0, 2.0);
        assert!(q.length_squared().approx_eq_default(4.0));
        assert!(q.length().approx_eq_default(2.0));
        assert!(q.dot(q).approx_eq_default(4.0));
    }

    #[test]
    fn normalize_basic_and_near_zero() {
        let q = Quat::from_xyzw(0.0, 0.0, 0.0, 3.0).normalize().unwrap();
        assert!(q.approx_eq_default(Quat::IDENTITY));
        assert!(Quat::from_xyzw(0.0, 0.0, 0.0, 0.0).normalize().is_none());
    }

    #[test]
    fn conjugate_and_inverse() {
        let q = Quat::from_axis_angle(Vec3::Y, 0.6).unwrap();
        assert!(q.conjugate().approx_eq_default(q.inverse().unwrap()));
        let prod = q * q.inverse().unwrap();
        assert!(prod.approx_eq(Quat::IDENTITY, 1e-5));
        assert!(Quat::from_xyzw(0.0, 0.0, 0.0, 0.0).inverse().is_none());
    }

    #[test]
    fn slerp_endpoints() {
        let a = Quat::from_rotation_x(0.2);
        let b = Quat::from_rotation_y(1.0);
        assert!(a.slerp(b, 0.0).approx_eq(a, 1e-5));
        assert!(a.slerp(b, 1.0).approx_eq(b, 1e-5));
    }

    #[test]
    fn slerp_negated_takes_shortest_path() {
        let a = Quat::from_rotation_x(0.2);
        let b = Quat::from_rotation_y(1.0);
        let v = Vec3::new(0.3, -1.0, 0.7);
        let direct = a.slerp(b, 0.5).rotate(v);
        let negated = a.slerp(-b, 0.5).rotate(v);
        assert!(direct.approx_eq(negated, 1e-5));
    }

    #[test]
    fn nlerp_endpoints_and_shortest_path() {
        let a = Quat::from_rotation_z(0.4);
        let b = Quat::from_rotation_z(1.5);
        assert!(a.nlerp(b, 0.0).approx_eq(a, 1e-5));
        assert!(a.nlerp(b, 1.0).approx_eq(b, 1e-5));
        let v = Vec3::new(1.0, 0.0, 0.0);
        let direct = a.nlerp(b, 0.5).rotate(v);
        let negated = a.nlerp(-b, 0.5).rotate(v);
        assert!(direct.approx_eq(negated, 1e-5));
    }

    #[test]
    fn is_normalized_and_is_finite() {
        assert!(Quat::IDENTITY.is_normalized());
        assert!(!Quat::from_xyzw(0.0, 0.0, 0.0, 2.0).is_normalized());
        assert!(Quat::IDENTITY.is_finite());
        assert!(!Quat::from_xyzw(f32::NAN, 0.0, 0.0, 1.0).is_finite());
        assert!(!Quat::from_xyzw(f32::INFINITY, 0.0, 0.0, 1.0).is_finite());
    }

    #[test]
    fn neg_negates_all_components() {
        let q = Quat::from_xyzw(1.0, -2.0, 3.0, -4.0);
        assert_eq!(-q, Quat::from_xyzw(-1.0, 2.0, -3.0, 4.0));
    }
}
