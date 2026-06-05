//! 4x4 column-major matrix.

use crate::math::{Quat, Vec3, Vec4};
use std::ops::{Add, Mul, Sub};

/// A 4x4 matrix stored in column-major order as four column vectors.
///
/// Used for 3D affine and projective transforms. Column vectors are
/// transformed as `M * v`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat4 {
    /// The four column vectors of the matrix.
    pub cols: [Vec4; 4],
}

const _: () = assert!(std::mem::size_of::<Mat4>() == 64);

impl Mat4 {
    /// The identity matrix.
    pub const IDENTITY: Self = Self {
        cols: [
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 1.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        ],
    };

    /// The zero matrix (all components zero).
    pub const ZERO: Self = Self {
        cols: [
            Vec4::new(0.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 0.0),
        ],
    };

    /// Creates a matrix from its four column vectors.
    pub const fn from_cols(c0: Vec4, c1: Vec4, c2: Vec4, c3: Vec4) -> Self {
        Self {
            cols: [c0, c1, c2, c3],
        }
    }

    /// Creates a diagonal matrix from the given diagonal entries.
    pub fn from_diagonal(d: Vec4) -> Self {
        Self {
            cols: [
                Vec4::new(d.x, 0.0, 0.0, 0.0),
                Vec4::new(0.0, d.y, 0.0, 0.0),
                Vec4::new(0.0, 0.0, d.z, 0.0),
                Vec4::new(0.0, 0.0, 0.0, d.w),
            ],
        }
    }

    /// Creates a translation matrix.
    pub fn from_translation(t: Vec3) -> Self {
        Self {
            cols: [
                Vec4::new(1.0, 0.0, 0.0, 0.0),
                Vec4::new(0.0, 1.0, 0.0, 0.0),
                Vec4::new(0.0, 0.0, 1.0, 0.0),
                Vec4::new(t.x, t.y, t.z, 1.0),
            ],
        }
    }

    /// Creates a non-uniform scale matrix.
    pub fn from_scale(s: Vec3) -> Self {
        Self {
            cols: [
                Vec4::new(s.x, 0.0, 0.0, 0.0),
                Vec4::new(0.0, s.y, 0.0, 0.0),
                Vec4::new(0.0, 0.0, s.z, 0.0),
                Vec4::new(0.0, 0.0, 0.0, 1.0),
            ],
        }
    }

    /// Creates a rotation matrix about the X axis (radians).
    pub fn from_rotation_x(radians: f32) -> Self {
        let (s, c) = radians.sin_cos();
        Self {
            cols: [
                Vec4::new(1.0, 0.0, 0.0, 0.0),
                Vec4::new(0.0, c, s, 0.0),
                Vec4::new(0.0, -s, c, 0.0),
                Vec4::new(0.0, 0.0, 0.0, 1.0),
            ],
        }
    }

    /// Creates a rotation matrix about the Y axis (radians).
    pub fn from_rotation_y(radians: f32) -> Self {
        let (s, c) = radians.sin_cos();
        Self {
            cols: [
                Vec4::new(c, 0.0, -s, 0.0),
                Vec4::new(0.0, 1.0, 0.0, 0.0),
                Vec4::new(s, 0.0, c, 0.0),
                Vec4::new(0.0, 0.0, 0.0, 1.0),
            ],
        }
    }

    /// Creates a rotation matrix about the Z axis (radians).
    pub fn from_rotation_z(radians: f32) -> Self {
        let (s, c) = radians.sin_cos();
        Self {
            cols: [
                Vec4::new(c, s, 0.0, 0.0),
                Vec4::new(-s, c, 0.0, 0.0),
                Vec4::new(0.0, 0.0, 1.0, 0.0),
                Vec4::new(0.0, 0.0, 0.0, 1.0),
            ],
        }
    }

    /// Creates a rotation matrix from a quaternion (assumed unit length).
    pub fn from_quat(q: Quat) -> Self {
        let (x, y, z, w) = (q.x, q.y, q.z, q.w);
        let (xx, yy, zz) = (x * x, y * y, z * z);
        let (xy, xz, yz) = (x * y, x * z, y * z);
        let (wx, wy, wz) = (w * x, w * y, w * z);
        Self {
            cols: [
                Vec4::new(1.0 - 2.0 * (yy + zz), 2.0 * (xy + wz), 2.0 * (xz - wy), 0.0),
                Vec4::new(2.0 * (xy - wz), 1.0 - 2.0 * (xx + zz), 2.0 * (yz + wx), 0.0),
                Vec4::new(2.0 * (xz + wy), 2.0 * (yz - wx), 1.0 - 2.0 * (xx + yy), 0.0),
                Vec4::new(0.0, 0.0, 0.0, 1.0),
            ],
        }
    }

    /// Composes a transform from scale, then rotation, then translation (TRS).
    pub fn from_scale_rotation_translation(s: Vec3, r: Quat, t: Vec3) -> Self {
        let rot = Self::from_quat(r);
        Self {
            cols: [
                rot.cols[0] * s.x,
                rot.cols[1] * s.y,
                rot.cols[2] * s.z,
                Vec4::new(t.x, t.y, t.z, 1.0),
            ],
        }
    }

    /// Creates a right-handed look-at view matrix, or `None` if the inputs are
    /// degenerate (`eye == target`, or `up` parallel to the view direction).
    pub fn look_at_rh(eye: Vec3, target: Vec3, up: Vec3) -> Option<Self> {
        let f = (target - eye).normalize()?;
        let s = f.cross(up).normalize()?;
        let u = s.cross(f);
        Some(Self {
            cols: [
                Vec4::new(s.x, u.x, -f.x, 0.0),
                Vec4::new(s.y, u.y, -f.y, 0.0),
                Vec4::new(s.z, u.z, -f.z, 0.0),
                Vec4::new(-s.dot(eye), -u.dot(eye), f.dot(eye), 1.0),
            ],
        })
    }

    /// Creates a right-handed perspective projection matrix with depth range
    /// `[0, 1]` (wgpu convention). Returns `None` if any of `fov_y_radians`,
    /// `aspect`, `z_near`, `z_far` is non-positive or non-finite, or if
    /// `z_near >= z_far`.
    pub fn perspective_rh(
        fov_y_radians: f32,
        aspect: f32,
        z_near: f32,
        z_far: f32,
    ) -> Option<Self> {
        if !fov_y_radians.is_finite()
            || !aspect.is_finite()
            || !z_near.is_finite()
            || !z_far.is_finite()
            || fov_y_radians <= 0.0
            || aspect <= 0.0
            || z_near <= 0.0
            || z_far <= 0.0
            || z_near >= z_far
        {
            return None;
        }
        let f = 1.0 / (fov_y_radians * 0.5).tan();
        Some(Self {
            cols: [
                Vec4::new(f / aspect, 0.0, 0.0, 0.0),
                Vec4::new(0.0, f, 0.0, 0.0),
                Vec4::new(0.0, 0.0, z_far / (z_near - z_far), -1.0),
                Vec4::new(0.0, 0.0, z_near * z_far / (z_near - z_far), 0.0),
            ],
        })
    }

    /// Creates a right-handed orthographic projection matrix with depth range
    /// `[0, 1]` (wgpu convention). Returns `None` on degenerate extents
    /// (`left == right`, `bottom == top`, `z_near == z_far`) or non-finite
    /// inputs.
    pub fn orthographic_rh(
        left: f32,
        right: f32,
        bottom: f32,
        top: f32,
        z_near: f32,
        z_far: f32,
    ) -> Option<Self> {
        if !left.is_finite()
            || !right.is_finite()
            || !bottom.is_finite()
            || !top.is_finite()
            || !z_near.is_finite()
            || !z_far.is_finite()
            || left == right
            || bottom == top
            || z_near == z_far
        {
            return None;
        }
        let rl = right - left;
        let tb = top - bottom;
        let nf = z_near - z_far;
        Some(Self {
            cols: [
                Vec4::new(2.0 / rl, 0.0, 0.0, 0.0),
                Vec4::new(0.0, 2.0 / tb, 0.0, 0.0),
                Vec4::new(0.0, 0.0, 1.0 / nf, 0.0),
                Vec4::new(-(right + left) / rl, -(top + bottom) / tb, z_near / nf, 1.0),
            ],
        })
    }

    /// Returns column `i`, or `None` if out of range.
    pub fn col(self, i: usize) -> Option<Vec4> {
        self.cols.get(i).copied()
    }

    /// Returns row `i`, or `None` if out of range.
    pub fn row(self, i: usize) -> Option<Vec4> {
        match i {
            0 => Some(Vec4::new(
                self.cols[0].x,
                self.cols[1].x,
                self.cols[2].x,
                self.cols[3].x,
            )),
            1 => Some(Vec4::new(
                self.cols[0].y,
                self.cols[1].y,
                self.cols[2].y,
                self.cols[3].y,
            )),
            2 => Some(Vec4::new(
                self.cols[0].z,
                self.cols[1].z,
                self.cols[2].z,
                self.cols[3].z,
            )),
            3 => Some(Vec4::new(
                self.cols[0].w,
                self.cols[1].w,
                self.cols[2].w,
                self.cols[3].w,
            )),
            _ => None,
        }
    }

    /// Returns the transpose of this matrix.
    pub fn transpose(self) -> Self {
        Self {
            cols: [
                Vec4::new(
                    self.cols[0].x,
                    self.cols[1].x,
                    self.cols[2].x,
                    self.cols[3].x,
                ),
                Vec4::new(
                    self.cols[0].y,
                    self.cols[1].y,
                    self.cols[2].y,
                    self.cols[3].y,
                ),
                Vec4::new(
                    self.cols[0].z,
                    self.cols[1].z,
                    self.cols[2].z,
                    self.cols[3].z,
                ),
                Vec4::new(
                    self.cols[0].w,
                    self.cols[1].w,
                    self.cols[2].w,
                    self.cols[3].w,
                ),
            ],
        }
    }

    /// Returns the determinant of this matrix.
    pub fn determinant(self) -> f32 {
        let m = &self.cols;
        let (m00, m10, m20, m30) = (m[0].x, m[0].y, m[0].z, m[0].w);
        let (m01, m11, m21, m31) = (m[1].x, m[1].y, m[1].z, m[1].w);
        let (m02, m12, m22, m32) = (m[2].x, m[2].y, m[2].z, m[2].w);
        let (m03, m13, m23, m33) = (m[3].x, m[3].y, m[3].z, m[3].w);

        let a2323 = m22 * m33 - m23 * m32;
        let a1323 = m21 * m33 - m23 * m31;
        let a1223 = m21 * m32 - m22 * m31;
        let a0323 = m20 * m33 - m23 * m30;
        let a0223 = m20 * m32 - m22 * m30;
        let a0123 = m20 * m31 - m21 * m30;

        m00 * (m11 * a2323 - m12 * a1323 + m13 * a1223)
            - m01 * (m10 * a2323 - m12 * a0323 + m13 * a0223)
            + m02 * (m10 * a1323 - m11 * a0323 + m13 * a0123)
            - m03 * (m10 * a1223 - m11 * a0223 + m12 * a0123)
    }

    /// Returns the inverse of this matrix, or `None` if it is singular
    /// (`|det| < 1e-12`).
    pub fn inverse(self) -> Option<Self> {
        let m = &self.cols;
        let (m00, m10, m20, m30) = (m[0].x, m[0].y, m[0].z, m[0].w);
        let (m01, m11, m21, m31) = (m[1].x, m[1].y, m[1].z, m[1].w);
        let (m02, m12, m22, m32) = (m[2].x, m[2].y, m[2].z, m[2].w);
        let (m03, m13, m23, m33) = (m[3].x, m[3].y, m[3].z, m[3].w);

        let a2323 = m22 * m33 - m23 * m32;
        let a1323 = m21 * m33 - m23 * m31;
        let a1223 = m21 * m32 - m22 * m31;
        let a0323 = m20 * m33 - m23 * m30;
        let a0223 = m20 * m32 - m22 * m30;
        let a0123 = m20 * m31 - m21 * m30;
        let a2313 = m12 * m33 - m13 * m32;
        let a1313 = m11 * m33 - m13 * m31;
        let a1213 = m11 * m32 - m12 * m31;
        let a2312 = m12 * m23 - m13 * m22;
        let a1312 = m11 * m23 - m13 * m21;
        let a1212 = m11 * m22 - m12 * m21;
        let a0313 = m10 * m33 - m13 * m30;
        let a0213 = m10 * m32 - m12 * m30;
        let a0312 = m10 * m23 - m13 * m20;
        let a0212 = m10 * m22 - m12 * m20;
        let a0113 = m10 * m31 - m11 * m30;
        let a0112 = m10 * m21 - m11 * m20;

        let det = m00 * (m11 * a2323 - m12 * a1323 + m13 * a1223)
            - m01 * (m10 * a2323 - m12 * a0323 + m13 * a0223)
            + m02 * (m10 * a1323 - m11 * a0323 + m13 * a0123)
            - m03 * (m10 * a1223 - m11 * a0223 + m12 * a0123);

        if det.abs() < 1e-12 {
            return None;
        }
        let inv_det = 1.0 / det;

        let i00 = (m11 * a2323 - m12 * a1323 + m13 * a1223) * inv_det;
        let i01 = -(m01 * a2323 - m02 * a1323 + m03 * a1223) * inv_det;
        let i02 = (m01 * a2313 - m02 * a1313 + m03 * a1213) * inv_det;
        let i03 = -(m01 * a2312 - m02 * a1312 + m03 * a1212) * inv_det;

        let i10 = -(m10 * a2323 - m12 * a0323 + m13 * a0223) * inv_det;
        let i11 = (m00 * a2323 - m02 * a0323 + m03 * a0223) * inv_det;
        let i12 = -(m00 * a2313 - m02 * a0313 + m03 * a0213) * inv_det;
        let i13 = (m00 * a2312 - m02 * a0312 + m03 * a0212) * inv_det;

        let i20 = (m10 * a1323 - m11 * a0323 + m13 * a0123) * inv_det;
        let i21 = -(m00 * a1323 - m01 * a0323 + m03 * a0123) * inv_det;
        let i22 = (m00 * a1313 - m01 * a0313 + m03 * a0113) * inv_det;
        let i23 = -(m00 * a1312 - m01 * a0312 + m03 * a0112) * inv_det;

        let i30 = -(m10 * a1223 - m11 * a0223 + m12 * a0123) * inv_det;
        let i31 = (m00 * a1223 - m01 * a0223 + m02 * a0123) * inv_det;
        let i32 = -(m00 * a1213 - m01 * a0213 + m02 * a0113) * inv_det;
        let i33 = (m00 * a1212 - m01 * a0212 + m02 * a0112) * inv_det;

        Some(Self {
            cols: [
                Vec4::new(i00, i10, i20, i30),
                Vec4::new(i01, i11, i21, i31),
                Vec4::new(i02, i12, i22, i32),
                Vec4::new(i03, i13, i23, i33),
            ],
        })
    }

    /// Transforms a point (implicit `w = 1`), without the perspective divide.
    pub fn transform_point(self, p: Vec3) -> Vec3 {
        let r = self * Vec4::new(p.x, p.y, p.z, 1.0);
        Vec3::new(r.x, r.y, r.z)
    }

    /// Transforms a point as `w = 1` and applies the perspective divide.
    /// Returns `None` if the resulting `w` is near zero (`|w| < 1e-12`).
    pub fn project_point(self, p: Vec3) -> Option<Vec3> {
        let r = self * Vec4::new(p.x, p.y, p.z, 1.0);
        if r.w.abs() < 1e-12 {
            return None;
        }
        let inv_w = 1.0 / r.w;
        Some(Vec3::new(r.x * inv_w, r.y * inv_w, r.z * inv_w))
    }

    /// Transforms a direction vector (implicit `w = 0`); ignores translation.
    pub fn transform_vector(self, v: Vec3) -> Vec3 {
        let r = self * Vec4::new(v.x, v.y, v.z, 0.0);
        Vec3::new(r.x, r.y, r.z)
    }

    /// Returns `true` if every component of the matrix is finite.
    pub fn is_finite(self) -> bool {
        self.cols[0].is_finite()
            && self.cols[1].is_finite()
            && self.cols[2].is_finite()
            && self.cols[3].is_finite()
    }
}

impl Default for Mat4 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Mul<Mat4> for Mat4 {
    type Output = Mat4;
    fn mul(self, rhs: Mat4) -> Mat4 {
        Mat4 {
            cols: [
                self * rhs.cols[0],
                self * rhs.cols[1],
                self * rhs.cols[2],
                self * rhs.cols[3],
            ],
        }
    }
}

impl Mul<Vec4> for Mat4 {
    type Output = Vec4;
    fn mul(self, rhs: Vec4) -> Vec4 {
        self.cols[0] * rhs.x + self.cols[1] * rhs.y + self.cols[2] * rhs.z + self.cols[3] * rhs.w
    }
}

impl Mul<f32> for Mat4 {
    type Output = Mat4;
    fn mul(self, rhs: f32) -> Mat4 {
        Mat4 {
            cols: [
                self.cols[0] * rhs,
                self.cols[1] * rhs,
                self.cols[2] * rhs,
                self.cols[3] * rhs,
            ],
        }
    }
}

impl Add for Mat4 {
    type Output = Mat4;
    fn add(self, rhs: Mat4) -> Mat4 {
        Mat4 {
            cols: [
                self.cols[0] + rhs.cols[0],
                self.cols[1] + rhs.cols[1],
                self.cols[2] + rhs.cols[2],
                self.cols[3] + rhs.cols[3],
            ],
        }
    }
}

impl Sub for Mat4 {
    type Output = Mat4;
    fn sub(self, rhs: Mat4) -> Mat4 {
        Mat4 {
            cols: [
                self.cols[0] - rhs.cols[0],
                self.cols[1] - rhs.cols[1],
                self.cols[2] - rhs.cols[2],
                self.cols[3] - rhs.cols[3],
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Vec2;
    use crate::traits::ApproxEq;
    use std::f32::consts::PI;

    #[test]
    fn size_is_64_bytes() {
        assert_eq!(std::mem::size_of::<Mat4>(), 64);
    }

    #[test]
    fn default_is_identity() {
        assert_eq!(Mat4::default(), Mat4::IDENTITY);
    }

    #[test]
    fn from_cols_col_row() {
        let m = Mat4::from_cols(
            Vec4::new(1.0, 2.0, 3.0, 4.0),
            Vec4::new(5.0, 6.0, 7.0, 8.0),
            Vec4::new(9.0, 10.0, 11.0, 12.0),
            Vec4::new(13.0, 14.0, 15.0, 16.0),
        );
        assert_eq!(m.col(0), Some(Vec4::new(1.0, 2.0, 3.0, 4.0)));
        assert_eq!(m.col(4), None);
        assert_eq!(m.row(0), Some(Vec4::new(1.0, 5.0, 9.0, 13.0)));
        assert_eq!(m.row(3), Some(Vec4::new(4.0, 8.0, 12.0, 16.0)));
        assert_eq!(m.row(4), None);
    }

    #[test]
    fn from_diagonal() {
        let m = Mat4::from_diagonal(Vec4::new(2.0, 3.0, 4.0, 1.0));
        assert!(m
            .transform_point(Vec3::new(1.0, 1.0, 1.0))
            .approx_eq_default(Vec3::new(2.0, 3.0, 4.0)));
    }

    #[test]
    fn translation_and_transform_point() {
        let m = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        assert!(m
            .transform_point(Vec3::new(10.0, 20.0, 30.0))
            .approx_eq_default(Vec3::new(11.0, 22.0, 33.0)));
    }

    #[test]
    fn transform_vector_ignores_translation() {
        let m = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        assert!(m
            .transform_vector(Vec3::new(10.0, 20.0, 30.0))
            .approx_eq_default(Vec3::new(10.0, 20.0, 30.0)));
    }

    #[test]
    fn scale() {
        let m = Mat4::from_scale(Vec3::new(2.0, 3.0, 4.0));
        assert!(m
            .transform_point(Vec3::new(1.0, 1.0, 1.0))
            .approx_eq_default(Vec3::new(2.0, 3.0, 4.0)));
    }

    #[test]
    fn rotations_quarter_turns() {
        assert!(
            (Mat4::from_rotation_x(PI / 2.0).transform_vector(Vec3::Y)).approx_eq_default(Vec3::Z)
        );
        assert!(
            (Mat4::from_rotation_y(PI / 2.0).transform_vector(Vec3::Z)).approx_eq_default(Vec3::X)
        );
        assert!(
            (Mat4::from_rotation_z(PI / 2.0).transform_vector(Vec3::X)).approx_eq_default(Vec3::Y)
        );
    }

    #[test]
    fn from_quat_matches_rotation_z() {
        let q = Quat::from_rotation_z(PI / 3.0);
        let mq = Mat4::from_quat(q);
        let mz = Mat4::from_rotation_z(PI / 3.0);
        assert!(mq
            .transform_vector(Vec3::X)
            .approx_eq_default(mz.transform_vector(Vec3::X)));
    }

    #[test]
    fn trs_compose() {
        let m = Mat4::from_scale_rotation_translation(
            Vec3::new(2.0, 2.0, 2.0),
            Quat::from_rotation_z(PI / 2.0),
            Vec3::new(1.0, 0.0, 0.0),
        );
        // Point (1,0,0): scale->(2,0,0), rotate Z 90deg->(0,2,0), translate->(1,2,0).
        assert!(m
            .transform_point(Vec3::new(1.0, 0.0, 0.0))
            .approx_eq_default(Vec3::new(1.0, 2.0, 0.0)));
    }

    #[test]
    fn transpose() {
        let m = Mat4::from_cols(
            Vec4::new(1.0, 2.0, 3.0, 4.0),
            Vec4::new(5.0, 6.0, 7.0, 8.0),
            Vec4::new(9.0, 10.0, 11.0, 12.0),
            Vec4::new(13.0, 14.0, 15.0, 16.0),
        );
        assert_eq!(m.transpose().row(0), m.col(0));
    }

    #[test]
    fn determinant_known() {
        assert!(Mat4::IDENTITY.determinant().approx_eq_default(1.0));
        let m = Mat4::from_diagonal(Vec4::new(2.0, 3.0, 4.0, 5.0));
        assert!(m.determinant().approx_eq_default(120.0));
    }

    #[test]
    fn inverse_times_matrix_is_identity() {
        let m = Mat4::from_scale_rotation_translation(
            Vec3::new(2.0, 3.0, 0.5),
            Quat::from_rotation_y(0.7),
            Vec3::new(5.0, -2.0, 1.0),
        );
        let inv = m.inverse().unwrap();
        let prod = m * inv;
        for i in 0..4 {
            assert!(prod
                .col(i)
                .unwrap()
                .approx_eq_default(Mat4::IDENTITY.col(i).unwrap()));
        }
    }

    #[test]
    fn inverse_singular_is_none() {
        let m = Mat4::from_scale(Vec3::new(1.0, 1.0, 0.0));
        assert!(m.inverse().is_none());
    }

    #[test]
    fn look_at_basic() {
        let m = Mat4::look_at_rh(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, Vec3::Y).unwrap();
        // Eye maps to origin in view space.
        assert!(m
            .transform_point(Vec3::new(0.0, 0.0, 5.0))
            .approx_eq_default(Vec3::ZERO));
        // Target is in front of camera (negative z in view space).
        let t = m.transform_point(Vec3::ZERO);
        assert!(t.z.approx_eq_default(-5.0));
    }

    #[test]
    fn look_at_degenerate_is_none() {
        assert!(Mat4::look_at_rh(Vec3::ZERO, Vec3::ZERO, Vec3::Y).is_none());
        // up parallel to forward.
        assert!(Mat4::look_at_rh(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), Vec3::Z).is_none());
    }

    #[test]
    fn perspective_depth_range() {
        let near = 0.1;
        let far = 100.0;
        let m = Mat4::perspective_rh(PI / 2.0, 16.0 / 9.0, near, far).unwrap();
        // Point on near plane maps to depth 0.
        let d_near = m.project_point(Vec3::new(0.0, 0.0, -near)).unwrap();
        assert!(d_near.z.approx_eq_default(0.0));
        // Point on far plane maps to depth 1.
        let d_far = m.project_point(Vec3::new(0.0, 0.0, -far)).unwrap();
        assert!(d_far.z.approx_eq(1.0, 1e-4));
    }

    #[test]
    fn perspective_invalid_inputs() {
        assert!(Mat4::perspective_rh(0.0, 1.0, 0.1, 100.0).is_none());
        assert!(Mat4::perspective_rh(PI / 2.0, 0.0, 0.1, 100.0).is_none());
        assert!(Mat4::perspective_rh(PI / 2.0, 1.0, -0.1, 100.0).is_none());
        assert!(Mat4::perspective_rh(PI / 2.0, 1.0, 100.0, 0.1).is_none());
        assert!(Mat4::perspective_rh(f32::NAN, 1.0, 0.1, 100.0).is_none());
    }

    #[test]
    fn orthographic_depth_range() {
        let near = 0.0;
        let far = 100.0;
        let m = Mat4::orthographic_rh(-1.0, 1.0, -1.0, 1.0, near, far).unwrap();
        let d_near = m.transform_point(Vec3::new(0.0, 0.0, -near));
        assert!(d_near.z.approx_eq_default(0.0));
        let d_far = m.transform_point(Vec3::new(0.0, 0.0, -far));
        assert!(d_far.z.approx_eq_default(1.0));
        // Corner mapping.
        let corner = m.transform_point(Vec3::new(1.0, 1.0, 0.0));
        assert!(Vec2::new(corner.x, corner.y).approx_eq_default(Vec2::new(1.0, 1.0)));
    }

    #[test]
    fn orthographic_degenerate_is_none() {
        assert!(Mat4::orthographic_rh(1.0, 1.0, -1.0, 1.0, 0.0, 1.0).is_none());
        assert!(Mat4::orthographic_rh(-1.0, 1.0, 2.0, 2.0, 0.0, 1.0).is_none());
        assert!(Mat4::orthographic_rh(-1.0, 1.0, -1.0, 1.0, 5.0, 5.0).is_none());
        assert!(Mat4::orthographic_rh(f32::INFINITY, 1.0, -1.0, 1.0, 0.0, 1.0).is_none());
    }

    #[test]
    fn project_point_near_zero_w_is_none() {
        // A matrix whose row-3 dot yields w = 0 for the given point.
        let m = Mat4::from_cols(
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 1.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 1.0),
            Vec4::new(0.0, 0.0, 0.0, 0.0),
        );
        assert!(m.project_point(Vec3::new(0.0, 0.0, 0.0)).is_none());
    }

    #[test]
    fn matrix_mul_associative_with_identity() {
        let m = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(m * Mat4::IDENTITY, m);
        assert_eq!(Mat4::IDENTITY * m, m);
    }

    #[test]
    fn scalar_add_sub() {
        let m = Mat4::IDENTITY * 2.0;
        assert!(m
            .col(0)
            .unwrap()
            .approx_eq_default(Vec4::new(2.0, 0.0, 0.0, 0.0)));
        let sum = Mat4::IDENTITY + Mat4::IDENTITY;
        let diff = sum - Mat4::IDENTITY;
        assert_eq!(diff, Mat4::IDENTITY);
    }

    #[test]
    fn is_finite() {
        assert!(Mat4::IDENTITY.is_finite());
        let bad = Mat4 {
            cols: [
                Vec4::new(f32::INFINITY, 0.0, 0.0, 0.0),
                Vec4::ZERO,
                Vec4::ZERO,
                Vec4::ZERO,
            ],
        };
        assert!(!bad.is_finite());
    }
}
