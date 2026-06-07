//! Affine transform types for 2D and 3D space.

use std::ops::Mul;

use crate::math::{Mat3, Mat4, Quat, Vec2, Vec3};

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform2D {
    pub translation: Vec2,
    /// Radians, counter-clockwise.
    pub rotation: f32,
    pub scale: Vec2,
}

const _: () = assert!(std::mem::size_of::<Transform2D>() == 20);

impl Transform2D {
    pub const IDENTITY: Self = Self {
        translation: Vec2::ZERO,
        rotation: 0.0,
        scale: Vec2::ONE,
    };

    pub fn from_translation(t: Vec2) -> Self {
        Self {
            translation: t,
            ..Self::IDENTITY
        }
    }

    pub fn from_rotation(radians: f32) -> Self {
        Self {
            rotation: radians,
            ..Self::IDENTITY
        }
    }

    pub fn from_scale(s: Vec2) -> Self {
        Self {
            scale: s,
            ..Self::IDENTITY
        }
    }

    /// Scale, then rotate, then translate.
    pub fn to_mat3(self) -> Mat3 {
        Mat3::from_translation_2d(self.translation)
            * Mat3::from_rotation_z(self.rotation)
            * Mat3::from_scale_2d(self.scale)
    }

    /// Scale, then rotate, then translate.
    pub fn transform_point(self, p: Vec2) -> Vec2 {
        self.transform_vector(p) + self.translation
    }

    /// Scale, then rotate (no translation).
    pub fn transform_vector(self, v: Vec2) -> Vec2 {
        let scaled = Vec2::new(v.x * self.scale.x, v.y * self.scale.y);
        let (sin, cos) = self.rotation.sin_cos();
        Vec2::new(
            scaled.x * cos - scaled.y * sin,
            scaled.x * sin + scaled.y * cos,
        )
    }

    /// `None` if any scale component magnitude is below `1e-12`.
    ///
    /// Exact when scale is uniform or rotation is zero; with non-uniform scale under rotation
    /// the true inverse is not representable as a TRS transform (same convention as [`Self::mul`]).
    pub fn inverse(self) -> Option<Self> {
        if self.scale.x.abs() < 1e-12 || self.scale.y.abs() < 1e-12 {
            return None;
        }
        let inv_scale = Vec2::new(1.0 / self.scale.x, 1.0 / self.scale.y);
        let inv_rotation = -self.rotation;
        let inv = Self {
            translation: Vec2::ZERO,
            rotation: inv_rotation,
            scale: inv_scale,
        };
        let translation = inv.transform_vector(-self.translation);
        Some(Self {
            translation,
            rotation: inv_rotation,
            scale: inv_scale,
        })
    }

    /// Composes `self` (parent) with `child`, yielding `parent * child`.
    ///
    /// Scale composes componentwise (engine convention, matching Bevy/Unity): non-uniform
    /// scale under rotation does not compose exactly through a TRS representation, so the
    /// resulting scale is the componentwise product of parent and child scale.
    // Spec §2.5 mandates both this inherent `mul` and the `Mul` operator impl; the lint
    // fires because of the operator overlap, but both are required by the public API.
    #[allow(clippy::should_implement_trait)]
    pub fn mul(self, child: Self) -> Self {
        Self {
            translation: self.transform_point(child.translation),
            rotation: self.rotation + child.rotation,
            scale: Vec2::new(self.scale.x * child.scale.x, self.scale.y * child.scale.y),
        }
    }
}

impl Default for Transform2D {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Mul for Transform2D {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self {
        Transform2D::mul(self, rhs)
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform3D {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

const _: () = assert!(std::mem::size_of::<Transform3D>() == 40);

impl Transform3D {
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    pub fn from_translation(t: Vec3) -> Self {
        Self {
            translation: t,
            ..Self::IDENTITY
        }
    }

    pub fn from_rotation(rotation: Quat) -> Self {
        Self {
            rotation,
            ..Self::IDENTITY
        }
    }

    pub fn from_scale(s: Vec3) -> Self {
        Self {
            scale: s,
            ..Self::IDENTITY
        }
    }

    /// Scale, then rotate, then translate (TRS).
    pub fn to_mat4(self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }

    /// Scale, then rotate, then translate.
    pub fn transform_point(self, p: Vec3) -> Vec3 {
        self.transform_vector(p) + self.translation
    }

    /// Scale, then rotate (no translation).
    pub fn transform_vector(self, v: Vec3) -> Vec3 {
        let scaled = Vec3::new(v.x * self.scale.x, v.y * self.scale.y, v.z * self.scale.z);
        self.rotation.rotate(scaled)
    }

    /// `None` if any scale component magnitude is below `1e-12` or the rotation is not invertible.
    ///
    /// Exact when scale is uniform or rotation is identity; with non-uniform scale under rotation
    /// the true inverse is not representable as a TRS transform (same convention as [`Self::mul`]).
    pub fn inverse(self) -> Option<Self> {
        if self.scale.x.abs() < 1e-12 || self.scale.y.abs() < 1e-12 || self.scale.z.abs() < 1e-12 {
            return None;
        }
        let inv_rotation = self.rotation.inverse()?;
        let inv_scale = Vec3::new(1.0 / self.scale.x, 1.0 / self.scale.y, 1.0 / self.scale.z);
        let rotated = inv_rotation.rotate(-self.translation);
        let translation = Vec3::new(
            rotated.x * inv_scale.x,
            rotated.y * inv_scale.y,
            rotated.z * inv_scale.z,
        );
        Some(Self {
            translation,
            rotation: inv_rotation,
            scale: inv_scale,
        })
    }

    /// Composes `self` (parent) with `child`, yielding `parent * child`.
    ///
    /// Scale composes componentwise (engine convention, matching Bevy/Unity): non-uniform
    /// scale under rotation does not compose exactly through a TRS representation, so the
    /// resulting scale is the componentwise product of parent and child scale.
    // Spec §2.6 mandates both this inherent `mul` and the `Mul` operator impl; the lint
    // fires because of the operator overlap, but both are required by the public API.
    #[allow(clippy::should_implement_trait)]
    pub fn mul(self, child: Self) -> Self {
        Self {
            translation: self.transform_point(child.translation),
            rotation: self.rotation * child.rotation,
            scale: Vec3::new(
                self.scale.x * child.scale.x,
                self.scale.y * child.scale.y,
                self.scale.z * child.scale.z,
            ),
        }
    }
}

impl Default for Transform3D {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Mul for Transform3D {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self {
        Transform3D::mul(self, rhs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::ApproxEq;
    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn transform2d_rotate_maps_x_to_y() {
        let t = Transform2D::from_rotation(FRAC_PI_2);
        assert!(t.transform_point(Vec2::X).approx_eq_default(Vec2::Y));
        assert!(t.transform_vector(Vec2::X).approx_eq_default(Vec2::Y));
    }

    #[test]
    fn transform2d_point_full() {
        let t = Transform2D {
            translation: Vec2::new(10.0, 20.0),
            rotation: FRAC_PI_2,
            scale: Vec2::new(2.0, 3.0),
        };
        let p = Vec2::new(1.0, 1.0);
        let expected = Vec2::new(10.0 - 3.0, 20.0 + 2.0);
        assert!(t.transform_point(p).approx_eq_default(expected));
    }

    #[test]
    fn transform2d_mat3_agrees_with_transform_point() {
        let t = Transform2D {
            translation: Vec2::new(5.0, -2.0),
            rotation: 0.7,
            scale: Vec2::new(1.5, 0.5),
        };
        let m = t.to_mat3();
        for p in [
            Vec2::new(1.0, 0.0),
            Vec2::new(-3.0, 4.0),
            Vec2::new(0.0, 2.0),
        ] {
            assert!(m
                .transform_point_2d(p)
                .approx_eq_default(t.transform_point(p)));
        }
    }

    #[test]
    fn transform2d_inverse_roundtrip() {
        let uniform = Transform2D {
            translation: Vec2::new(5.0, -2.0),
            rotation: 0.7,
            scale: Vec2::new(1.5, 1.5),
        };
        let unrotated = Transform2D {
            translation: Vec2::new(5.0, -2.0),
            rotation: 0.0,
            scale: Vec2::new(1.5, 0.5),
        };
        for t in [uniform, unrotated] {
            let inv = t.inverse().unwrap();
            for p in [
                Vec2::new(1.0, 0.0),
                Vec2::new(-3.0, 4.0),
                Vec2::new(0.0, 2.0),
            ] {
                assert!(inv.transform_point(t.transform_point(p)).approx_eq(p, 1e-5));
            }
        }
    }

    #[test]
    fn transform2d_mul_composition() {
        let a = Transform2D {
            translation: Vec2::new(1.0, 2.0),
            rotation: 0.3,
            scale: Vec2::new(2.0, 2.0),
        };
        let b = Transform2D {
            translation: Vec2::new(-1.0, 0.5),
            rotation: -0.2,
            scale: Vec2::new(0.5, 1.5),
        };
        for p in [
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, -2.0),
            Vec2::new(3.0, 0.0),
        ] {
            assert!((a * b)
                .transform_point(p)
                .approx_eq_default(a.transform_point(b.transform_point(p))));
        }
    }

    #[test]
    fn transform2d_inverse_zero_scale_none() {
        let t = Transform2D::from_scale(Vec2::new(0.0, 1.0));
        assert!(t.inverse().is_none());
    }

    #[test]
    fn transform3d_rotate_maps_x_to_y() {
        let t = Transform3D::from_rotation(Quat::from_axis_angle(Vec3::Z, FRAC_PI_2).unwrap());
        assert!(t.transform_point(Vec3::X).approx_eq_default(Vec3::Y));
        assert!(t.transform_vector(Vec3::X).approx_eq_default(Vec3::Y));
    }

    #[test]
    fn transform3d_mat4_agrees_with_transform_point() {
        let t = Transform3D {
            translation: Vec3::new(5.0, -2.0, 1.0),
            rotation: Quat::from_axis_angle(Vec3::new(1.0, 1.0, 0.0), 0.6).unwrap(),
            scale: Vec3::new(1.5, 0.5, 2.0),
        };
        let m = t.to_mat4();
        for p in [
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(-3.0, 4.0, 2.0),
            Vec3::new(0.0, 2.0, -1.0),
        ] {
            assert!(m.transform_point(p).approx_eq_default(t.transform_point(p)));
        }
    }

    #[test]
    fn transform3d_inverse_roundtrip() {
        let uniform = Transform3D {
            translation: Vec3::new(5.0, -2.0, 1.0),
            rotation: Quat::from_axis_angle(Vec3::new(0.0, 1.0, 1.0), 0.9).unwrap(),
            scale: Vec3::new(1.5, 1.5, 1.5),
        };
        let unrotated = Transform3D {
            translation: Vec3::new(5.0, -2.0, 1.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::new(1.5, 0.5, 2.0),
        };
        for t in [uniform, unrotated] {
            let inv = t.inverse().unwrap();
            for p in [
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(-3.0, 4.0, 2.0),
                Vec3::new(0.0, 2.0, -1.0),
            ] {
                assert!(inv.transform_point(t.transform_point(p)).approx_eq(p, 1e-5));
            }
        }
    }

    #[test]
    fn transform3d_mul_composition() {
        let a = Transform3D {
            translation: Vec3::new(1.0, 2.0, -1.0),
            rotation: Quat::from_axis_angle(Vec3::Z, 0.3).unwrap(),
            scale: Vec3::new(2.0, 2.0, 2.0),
        };
        let b = Transform3D {
            translation: Vec3::new(-1.0, 0.5, 2.0),
            rotation: Quat::from_axis_angle(Vec3::Y, -0.4).unwrap(),
            scale: Vec3::new(0.5, 0.5, 0.5),
        };
        for p in [
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(0.0, -2.0, 1.0),
            Vec3::new(3.0, 0.0, -2.0),
        ] {
            assert!((a * b)
                .transform_point(p)
                .approx_eq_default(a.transform_point(b.transform_point(p))));
        }
    }

    #[test]
    fn transform3d_inverse_zero_scale_none() {
        let t = Transform3D::from_scale(Vec3::new(1.0, 0.0, 1.0));
        assert!(t.inverse().is_none());
    }

    #[test]
    fn defaults_are_identity() {
        assert_eq!(Transform2D::default(), Transform2D::IDENTITY);
        assert_eq!(Transform3D::default(), Transform3D::IDENTITY);
    }
}
