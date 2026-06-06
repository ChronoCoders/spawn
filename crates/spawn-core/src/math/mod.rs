//! Math types and scalar helpers. Right-handed, column-major, column vectors, radians.

mod mat3;
mod mat4;
mod quat;
mod vec2;
mod vec3;
mod vec4;

pub use mat3::Mat3;
pub use mat4::Mat4;
pub use quat::Quat;
pub use vec2::Vec2;
pub use vec3::Vec3;
pub use vec4::Vec4;

pub const EPSILON: f32 = 1e-6;

/// Unclamped.
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Returns the interpolation factor of `v` between `a` and `b`, or `None` if `a == b`.
pub fn inverse_lerp(a: f32, b: f32, v: f32) -> Option<f32> {
    if a == b {
        None
    } else {
        Some((v - a) / (b - a))
    }
}

/// Remaps `v` from `[in_min, in_max]` to `[out_min, out_max]`, or `None` if the input range is empty.
pub fn remap(v: f32, in_min: f32, in_max: f32, out_min: f32, out_max: f32) -> Option<f32> {
    inverse_lerp(in_min, in_max, v).map(|t| lerp(out_min, out_max, t))
}

/// Wraps an angle in radians to `(-PI, PI]`.
pub fn wrap_angle(radians: f32) -> f32 {
    let two_pi = 2.0 * std::f32::consts::PI;
    let r = radians.rem_euclid(two_pi);
    if r > std::f32::consts::PI {
        r - two_pi
    } else {
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::ApproxEq;
    use std::f32::consts::PI;

    #[test]
    fn lerp_endpoints_and_midpoint() {
        assert!(lerp(2.0, 4.0, 0.0).approx_eq_default(2.0));
        assert!(lerp(2.0, 4.0, 1.0).approx_eq_default(4.0));
        assert!(lerp(2.0, 4.0, 0.5).approx_eq_default(3.0));
        assert!(lerp(2.0, 4.0, 2.0).approx_eq_default(6.0));
    }

    #[test]
    fn inverse_lerp_basic_and_degenerate() {
        assert!(inverse_lerp(2.0, 4.0, 3.0).is_some_and(|t| t.approx_eq_default(0.5)));
        assert!(inverse_lerp(5.0, 5.0, 5.0).is_none());
    }

    #[test]
    fn remap_basic_and_degenerate() {
        assert!(remap(5.0, 0.0, 10.0, 0.0, 100.0).is_some_and(|v| v.approx_eq_default(50.0)));
        assert!(remap(1.0, 3.0, 3.0, 0.0, 1.0).is_none());
    }

    #[test]
    fn wrap_angle_range() {
        assert!(wrap_angle(0.0).approx_eq_default(0.0));
        assert!(wrap_angle(PI).approx_eq_default(PI));
        assert!(wrap_angle(-PI).approx_eq_default(PI));
        assert!(wrap_angle(3.0 * PI).approx_eq_default(PI));
        assert!(wrap_angle(2.0 * PI).approx_eq_default(0.0));
        assert!(wrap_angle(-PI / 2.0).approx_eq_default(-PI / 2.0));
    }
}
