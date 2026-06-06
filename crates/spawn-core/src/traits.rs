//! Shared utility traits for approximate comparison and interpolation.

use crate::math::{self, Mat3, Mat4, Quat, Vec2, Vec3, Vec4};
use crate::primitives::{Color, Transform2D, Transform3D};

/// Approximate, epsilon-based equality for float-backed types.
///
/// This is the only sanctioned float-comparison path. Comparison is componentwise on the
/// absolute difference: each pair of components is equal when `(a - b).abs() <= epsilon`.
pub trait ApproxEq {
    fn approx_eq(self, rhs: Self, epsilon: f32) -> bool;

    /// Uses the default epsilon ([`math::EPSILON`]).
    fn approx_eq_default(self, rhs: Self) -> bool
    where
        Self: Sized,
    {
        self.approx_eq(rhs, math::EPSILON)
    }
}

impl ApproxEq for f32 {
    fn approx_eq(self, rhs: Self, epsilon: f32) -> bool {
        (self - rhs).abs() <= epsilon
    }
}

impl ApproxEq for Vec2 {
    fn approx_eq(self, rhs: Self, epsilon: f32) -> bool {
        self.x.approx_eq(rhs.x, epsilon) && self.y.approx_eq(rhs.y, epsilon)
    }
}

impl ApproxEq for Vec3 {
    fn approx_eq(self, rhs: Self, epsilon: f32) -> bool {
        self.x.approx_eq(rhs.x, epsilon)
            && self.y.approx_eq(rhs.y, epsilon)
            && self.z.approx_eq(rhs.z, epsilon)
    }
}

impl ApproxEq for Vec4 {
    fn approx_eq(self, rhs: Self, epsilon: f32) -> bool {
        self.x.approx_eq(rhs.x, epsilon)
            && self.y.approx_eq(rhs.y, epsilon)
            && self.z.approx_eq(rhs.z, epsilon)
            && self.w.approx_eq(rhs.w, epsilon)
    }
}

impl ApproxEq for Mat3 {
    fn approx_eq(self, rhs: Self, epsilon: f32) -> bool {
        self.cols[0].approx_eq(rhs.cols[0], epsilon)
            && self.cols[1].approx_eq(rhs.cols[1], epsilon)
            && self.cols[2].approx_eq(rhs.cols[2], epsilon)
    }
}

impl ApproxEq for Mat4 {
    fn approx_eq(self, rhs: Self, epsilon: f32) -> bool {
        self.cols[0].approx_eq(rhs.cols[0], epsilon)
            && self.cols[1].approx_eq(rhs.cols[1], epsilon)
            && self.cols[2].approx_eq(rhs.cols[2], epsilon)
            && self.cols[3].approx_eq(rhs.cols[3], epsilon)
    }
}

impl ApproxEq for Quat {
    fn approx_eq(self, rhs: Self, epsilon: f32) -> bool {
        self.x.approx_eq(rhs.x, epsilon)
            && self.y.approx_eq(rhs.y, epsilon)
            && self.z.approx_eq(rhs.z, epsilon)
            && self.w.approx_eq(rhs.w, epsilon)
    }
}

impl ApproxEq for Color {
    fn approx_eq(self, rhs: Self, epsilon: f32) -> bool {
        self.r.approx_eq(rhs.r, epsilon)
            && self.g.approx_eq(rhs.g, epsilon)
            && self.b.approx_eq(rhs.b, epsilon)
            && self.a.approx_eq(rhs.a, epsilon)
    }
}

impl ApproxEq for Transform2D {
    fn approx_eq(self, rhs: Self, epsilon: f32) -> bool {
        self.translation.approx_eq(rhs.translation, epsilon)
            && self.rotation.approx_eq(rhs.rotation, epsilon)
            && self.scale.approx_eq(rhs.scale, epsilon)
    }
}

impl ApproxEq for Transform3D {
    fn approx_eq(self, rhs: Self, epsilon: f32) -> bool {
        self.translation.approx_eq(rhs.translation, epsilon)
            && self.rotation.approx_eq(rhs.rotation, epsilon)
            && self.scale.approx_eq(rhs.scale, epsilon)
    }
}

/// Unclamped linear interpolation. Not implemented for [`Quat`] (use
/// `slerp`/`nlerp`; componentwise quaternion lerp is a correctness trap).
pub trait Lerp {
    fn lerp(self, rhs: Self, t: f32) -> Self;
}

impl Lerp for f32 {
    fn lerp(self, rhs: Self, t: f32) -> Self {
        math::lerp(self, rhs, t)
    }
}

impl Lerp for Vec2 {
    fn lerp(self, rhs: Self, t: f32) -> Self {
        Vec2::lerp(self, rhs, t)
    }
}

impl Lerp for Vec3 {
    fn lerp(self, rhs: Self, t: f32) -> Self {
        Vec3::lerp(self, rhs, t)
    }
}

impl Lerp for Vec4 {
    fn lerp(self, rhs: Self, t: f32) -> Self {
        Vec4::lerp(self, rhs, t)
    }
}

impl Lerp for Color {
    fn lerp(self, rhs: Self, t: f32) -> Self {
        Color::lerp(self, rhs, t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approx_eq_epsilon_boundary() {
        assert!(1.0_f32.approx_eq(1.25, 0.25));
        assert!(!1.0_f32.approx_eq(1.3, 0.25));
    }

    #[test]
    fn approx_eq_default_uses_default_epsilon() {
        assert!(1.0_f32.approx_eq_default(1.0 + 1e-6));
        assert!(!1.0_f32.approx_eq_default(1.0 + 1e-5));
    }

    #[test]
    fn approx_eq_componentwise_types() {
        assert!(Vec2::new(1.0, 2.0).approx_eq_default(Vec2::new(1.0, 2.0)));
        assert!(!Vec2::new(1.0, 2.0).approx_eq_default(Vec2::new(1.0, 2.1)));
        assert!(Vec3::ONE.approx_eq_default(Vec3::ONE));
        assert!(Vec4::ZERO.approx_eq_default(Vec4::ZERO));
        assert!(Mat3::IDENTITY.approx_eq_default(Mat3::IDENTITY));
        assert!(Mat4::IDENTITY.approx_eq_default(Mat4::IDENTITY));
        assert!(Quat::IDENTITY.approx_eq_default(Quat::IDENTITY));
        assert!(Color::WHITE.approx_eq_default(Color::WHITE));
        assert!(Transform2D::IDENTITY.approx_eq_default(Transform2D::IDENTITY));
        assert!(Transform3D::IDENTITY.approx_eq_default(Transform3D::IDENTITY));
    }

    #[test]
    fn lerp_matches_inherent() {
        assert!(Lerp::lerp(2.0_f32, 4.0, 0.5).approx_eq_default(3.0));
        assert!(Lerp::lerp(Vec2::ZERO, Vec2::new(2.0, 4.0), 0.5)
            .approx_eq_default(Vec2::new(2.0, 4.0).lerp(Vec2::ZERO, 0.5)));
        assert!(Lerp::lerp(Vec3::ZERO, Vec3::ONE, 0.25).approx_eq_default(Vec3::splat(0.25)));
        assert!(Lerp::lerp(Vec4::ZERO, Vec4::splat(8.0), 0.5).approx_eq_default(Vec4::splat(4.0)));
        assert!(Lerp::lerp(Color::BLACK, Color::WHITE, 0.5)
            .approx_eq_default(Color::BLACK.lerp(Color::WHITE, 0.5)));
    }
}
