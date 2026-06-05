//! 3x3 column-major matrix.

use crate::math::{Quat, Vec2, Vec3};
use std::ops::{Add, Mul, Sub};

/// A 3x3 matrix stored in column-major order as three column vectors.
///
/// Used both for 3D linear transforms (rotation/scale) and as a 2D
/// homogeneous affine transform (the third column is translation).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat3 {
    /// The three column vectors of the matrix.
    pub cols: [Vec3; 3],
}

const _: () = assert!(std::mem::size_of::<Mat3>() == 36);

impl Mat3 {
    /// The identity matrix.
    pub const IDENTITY: Self = Self {
        cols: [
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
        ],
    };

    /// The zero matrix (all components zero).
    pub const ZERO: Self = Self {
        cols: [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
        ],
    };

    /// Creates a matrix from its three column vectors.
    pub const fn from_cols(c0: Vec3, c1: Vec3, c2: Vec3) -> Self {
        Self { cols: [c0, c1, c2] }
    }

    /// Creates a matrix from its three row vectors.
    pub fn from_rows(r0: Vec3, r1: Vec3, r2: Vec3) -> Self {
        Self {
            cols: [
                Vec3::new(r0.x, r1.x, r2.x),
                Vec3::new(r0.y, r1.y, r2.y),
                Vec3::new(r0.z, r1.z, r2.z),
            ],
        }
    }

    /// Creates a diagonal matrix from the given diagonal entries.
    pub fn from_diagonal(d: Vec3) -> Self {
        Self {
            cols: [
                Vec3::new(d.x, 0.0, 0.0),
                Vec3::new(0.0, d.y, 0.0),
                Vec3::new(0.0, 0.0, d.z),
            ],
        }
    }

    /// Creates a rotation matrix about the X axis (radians).
    pub fn from_rotation_x(radians: f32) -> Self {
        let (s, c) = radians.sin_cos();
        Self {
            cols: [
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, c, s),
                Vec3::new(0.0, -s, c),
            ],
        }
    }

    /// Creates a rotation matrix about the Y axis (radians).
    pub fn from_rotation_y(radians: f32) -> Self {
        let (s, c) = radians.sin_cos();
        Self {
            cols: [
                Vec3::new(c, 0.0, -s),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(s, 0.0, c),
            ],
        }
    }

    /// Creates a rotation matrix about the Z axis (radians).
    pub fn from_rotation_z(radians: f32) -> Self {
        let (s, c) = radians.sin_cos();
        Self {
            cols: [
                Vec3::new(c, s, 0.0),
                Vec3::new(-s, c, 0.0),
                Vec3::new(0.0, 0.0, 1.0),
            ],
        }
    }

    /// Creates a 2D homogeneous affine matrix that scales by `scale`.
    pub fn from_scale_2d(scale: Vec2) -> Self {
        Self {
            cols: [
                Vec3::new(scale.x, 0.0, 0.0),
                Vec3::new(0.0, scale.y, 0.0),
                Vec3::new(0.0, 0.0, 1.0),
            ],
        }
    }

    /// Creates a 2D homogeneous affine matrix that translates by `t`.
    pub fn from_translation_2d(t: Vec2) -> Self {
        Self {
            cols: [
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(t.x, t.y, 1.0),
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
                Vec3::new(1.0 - 2.0 * (yy + zz), 2.0 * (xy + wz), 2.0 * (xz - wy)),
                Vec3::new(2.0 * (xy - wz), 1.0 - 2.0 * (xx + zz), 2.0 * (yz + wx)),
                Vec3::new(2.0 * (xz + wy), 2.0 * (yz - wx), 1.0 - 2.0 * (xx + yy)),
            ],
        }
    }

    /// Returns column `i`, or `None` if out of range.
    pub fn col(self, i: usize) -> Option<Vec3> {
        self.cols.get(i).copied()
    }

    /// Returns row `i`, or `None` if out of range.
    pub fn row(self, i: usize) -> Option<Vec3> {
        match i {
            0 => Some(Vec3::new(self.cols[0].x, self.cols[1].x, self.cols[2].x)),
            1 => Some(Vec3::new(self.cols[0].y, self.cols[1].y, self.cols[2].y)),
            2 => Some(Vec3::new(self.cols[0].z, self.cols[1].z, self.cols[2].z)),
            _ => None,
        }
    }

    /// Returns the transpose of this matrix.
    pub fn transpose(self) -> Self {
        Self {
            cols: [
                Vec3::new(self.cols[0].x, self.cols[1].x, self.cols[2].x),
                Vec3::new(self.cols[0].y, self.cols[1].y, self.cols[2].y),
                Vec3::new(self.cols[0].z, self.cols[1].z, self.cols[2].z),
            ],
        }
    }

    /// Returns the determinant of this matrix.
    pub fn determinant(self) -> f32 {
        let [a, b, c] = self.cols;
        a.x * (b.y * c.z - c.y * b.z) - b.x * (a.y * c.z - c.y * a.z)
            + c.x * (a.y * b.z - b.y * a.z)
    }

    /// Returns the inverse of this matrix, or `None` if it is singular
    /// (`|det| < 1e-12`).
    pub fn inverse(self) -> Option<Self> {
        let det = self.determinant();
        if det.abs() < 1e-12 {
            return None;
        }
        let inv_det = 1.0 / det;
        let [a, b, c] = self.cols;
        // Cofactor matrix entries, transposed (adjugate), scaled by 1/det.
        let m00 = b.y * c.z - c.y * b.z;
        let m01 = c.x * b.z - b.x * c.z;
        let m02 = b.x * c.y - c.x * b.y;
        let m10 = c.y * a.z - a.y * c.z;
        let m11 = a.x * c.z - c.x * a.z;
        let m12 = c.x * a.y - a.x * c.y;
        let m20 = a.y * b.z - b.y * a.z;
        let m21 = b.x * a.z - a.x * b.z;
        let m22 = a.x * b.y - b.x * a.y;
        Some(Self {
            cols: [
                Vec3::new(m00 * inv_det, m10 * inv_det, m20 * inv_det),
                Vec3::new(m01 * inv_det, m11 * inv_det, m21 * inv_det),
                Vec3::new(m02 * inv_det, m12 * inv_det, m22 * inv_det),
            ],
        })
    }

    /// Transforms a 2D point, treating the matrix as a homogeneous affine
    /// transform (applies translation).
    pub fn transform_point_2d(self, p: Vec2) -> Vec2 {
        let r = self * Vec3::new(p.x, p.y, 1.0);
        Vec2::new(r.x, r.y)
    }

    /// Transforms a 2D direction vector, treating the matrix as a homogeneous
    /// affine transform (ignores translation).
    pub fn transform_vector_2d(self, v: Vec2) -> Vec2 {
        let r = self * Vec3::new(v.x, v.y, 0.0);
        Vec2::new(r.x, r.y)
    }

    /// Returns `true` if every component of the matrix is finite.
    pub fn is_finite(self) -> bool {
        self.cols[0].is_finite() && self.cols[1].is_finite() && self.cols[2].is_finite()
    }
}

impl Default for Mat3 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Mul<Mat3> for Mat3 {
    type Output = Mat3;
    fn mul(self, rhs: Mat3) -> Mat3 {
        Mat3 {
            cols: [self * rhs.cols[0], self * rhs.cols[1], self * rhs.cols[2]],
        }
    }
}

impl Mul<Vec3> for Mat3 {
    type Output = Vec3;
    fn mul(self, rhs: Vec3) -> Vec3 {
        self.cols[0] * rhs.x + self.cols[1] * rhs.y + self.cols[2] * rhs.z
    }
}

impl Mul<f32> for Mat3 {
    type Output = Mat3;
    fn mul(self, rhs: f32) -> Mat3 {
        Mat3 {
            cols: [self.cols[0] * rhs, self.cols[1] * rhs, self.cols[2] * rhs],
        }
    }
}

impl Add for Mat3 {
    type Output = Mat3;
    fn add(self, rhs: Mat3) -> Mat3 {
        Mat3 {
            cols: [
                self.cols[0] + rhs.cols[0],
                self.cols[1] + rhs.cols[1],
                self.cols[2] + rhs.cols[2],
            ],
        }
    }
}

impl Sub for Mat3 {
    type Output = Mat3;
    fn sub(self, rhs: Mat3) -> Mat3 {
        Mat3 {
            cols: [
                self.cols[0] - rhs.cols[0],
                self.cols[1] - rhs.cols[1],
                self.cols[2] - rhs.cols[2],
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::ApproxEq;
    use std::f32::consts::PI;

    #[test]
    fn size_is_36_bytes() {
        assert_eq!(std::mem::size_of::<Mat3>(), 36);
    }

    #[test]
    fn default_is_identity() {
        assert_eq!(Mat3::default(), Mat3::IDENTITY);
    }

    #[test]
    fn from_cols_and_col() {
        let m = Mat3::from_cols(
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(4.0, 5.0, 6.0),
            Vec3::new(7.0, 8.0, 9.0),
        );
        assert_eq!(m.col(0), Some(Vec3::new(1.0, 2.0, 3.0)));
        assert_eq!(m.col(2), Some(Vec3::new(7.0, 8.0, 9.0)));
        assert_eq!(m.col(3), None);
    }

    #[test]
    fn from_rows_and_row() {
        let m = Mat3::from_rows(
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(4.0, 5.0, 6.0),
            Vec3::new(7.0, 8.0, 9.0),
        );
        assert_eq!(m.row(0), Some(Vec3::new(1.0, 2.0, 3.0)));
        assert_eq!(m.row(1), Some(Vec3::new(4.0, 5.0, 6.0)));
        assert_eq!(m.row(2), Some(Vec3::new(7.0, 8.0, 9.0)));
        assert_eq!(m.row(3), None);
    }

    #[test]
    fn from_diagonal() {
        let m = Mat3::from_diagonal(Vec3::new(2.0, 3.0, 4.0));
        assert!((m * Vec3::new(1.0, 1.0, 1.0)).approx_eq_default(Vec3::new(2.0, 3.0, 4.0)));
    }

    #[test]
    fn rotation_z_quarter_turn() {
        let m = Mat3::from_rotation_z(PI / 2.0);
        assert!((m * Vec3::X).approx_eq_default(Vec3::Y));
    }

    #[test]
    fn rotation_x_quarter_turn() {
        let m = Mat3::from_rotation_x(PI / 2.0);
        assert!((m * Vec3::Y).approx_eq_default(Vec3::Z));
    }

    #[test]
    fn rotation_y_quarter_turn() {
        let m = Mat3::from_rotation_y(PI / 2.0);
        assert!((m * Vec3::Z).approx_eq_default(Vec3::X));
    }

    #[test]
    fn from_quat_matches_rotation() {
        let q = Quat::from_rotation_z(PI / 3.0);
        let mq = Mat3::from_quat(q);
        let mz = Mat3::from_rotation_z(PI / 3.0);
        assert!((mq * Vec3::X).approx_eq_default(mz * Vec3::X));
        assert!((mq * Vec3::Y).approx_eq_default(mz * Vec3::Y));
    }

    #[test]
    fn transpose() {
        let m = Mat3::from_cols(
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(4.0, 5.0, 6.0),
            Vec3::new(7.0, 8.0, 9.0),
        );
        let t = m.transpose();
        assert_eq!(t.row(0), Some(Vec3::new(1.0, 2.0, 3.0)));
        assert_eq!(t.col(0), Some(Vec3::new(1.0, 4.0, 7.0)));
    }

    #[test]
    fn determinant_known() {
        let m = Mat3::from_cols(
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(0.0, 3.0, 0.0),
            Vec3::new(0.0, 0.0, 4.0),
        );
        assert!(m.determinant().approx_eq_default(24.0));
        assert!(Mat3::IDENTITY.determinant().approx_eq_default(1.0));
    }

    #[test]
    fn inverse_times_matrix_is_identity() {
        let m = Mat3::from_cols(
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(0.0, 1.0, 4.0),
            Vec3::new(5.0, 6.0, 0.0),
        );
        let inv = m.inverse().unwrap();
        let prod = m * inv;
        for i in 0..3 {
            assert!(prod
                .col(i)
                .unwrap()
                .approx_eq_default(Mat3::IDENTITY.col(i).unwrap()));
        }
    }

    #[test]
    fn inverse_singular_is_none() {
        let m = Mat3::from_cols(
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(2.0, 4.0, 6.0),
            Vec3::new(0.0, 0.0, 0.0),
        );
        assert!(m.inverse().is_none());
    }

    #[test]
    fn transform_point_2d_applies_translation() {
        let m = Mat3::from_translation_2d(Vec2::new(5.0, 7.0));
        assert!(m
            .transform_point_2d(Vec2::new(1.0, 2.0))
            .approx_eq_default(Vec2::new(6.0, 9.0)));
    }

    #[test]
    fn transform_vector_2d_ignores_translation() {
        let m = Mat3::from_translation_2d(Vec2::new(5.0, 7.0));
        assert!(m
            .transform_vector_2d(Vec2::new(1.0, 2.0))
            .approx_eq_default(Vec2::new(1.0, 2.0)));
    }

    #[test]
    fn from_scale_2d() {
        let m = Mat3::from_scale_2d(Vec2::new(2.0, 3.0));
        assert!(m
            .transform_point_2d(Vec2::new(1.0, 1.0))
            .approx_eq_default(Vec2::new(2.0, 3.0)));
    }

    #[test]
    fn matrix_mul() {
        let a = Mat3::from_rotation_z(PI / 2.0);
        let b = Mat3::from_rotation_z(PI / 2.0);
        let c = a * b;
        assert!((c * Vec3::X).approx_eq_default(Vec3::NEG_X));
    }

    #[test]
    fn scalar_add_sub() {
        let m = Mat3::IDENTITY * 2.0;
        assert!(m
            .col(0)
            .unwrap()
            .approx_eq_default(Vec3::new(2.0, 0.0, 0.0)));
        let sum = Mat3::IDENTITY + Mat3::IDENTITY;
        assert!(sum
            .col(1)
            .unwrap()
            .approx_eq_default(Vec3::new(0.0, 2.0, 0.0)));
        let diff = sum - Mat3::IDENTITY;
        assert!(diff
            .col(2)
            .unwrap()
            .approx_eq_default(Vec3::new(0.0, 0.0, 1.0)));
    }

    #[test]
    fn is_finite() {
        assert!(Mat3::IDENTITY.is_finite());
        let bad = Mat3::from_cols(Vec3::new(f32::NAN, 0.0, 0.0), Vec3::ZERO, Vec3::ZERO);
        assert!(!bad.is_finite());
    }
}
