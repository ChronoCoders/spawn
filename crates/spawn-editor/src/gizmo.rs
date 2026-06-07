//! Pure gizmo math over spawn-core types.
//!
//! No rendering, no state, no [`World`](spawn_ecs::World). These closed-form,
//! allocation-free functions compute the geometry a later-phase gizmo renderer
//! and drag controller consume. All inputs/outputs are spawn-core math types and
//! obey the engine's right-handed, radians conventions. Every function is total:
//! degeneracies return a documented sentinel (`None`/`0.0`/`Vec3::ZERO`), never
//! a panic.

use spawn_core::{Vec3, EPSILON};

/// A ray with an origin and a direction assumed to be unit length.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    /// Starting point.
    pub origin: Vec3,
    /// Travel direction, assumed normalized.
    pub direction: Vec3,
}

impl Ray {
    /// The point `origin + direction * t`.
    pub fn at(self, t: f32) -> Vec3 {
        self.origin + self.direction * t
    }
}

/// A principal coordinate axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    /// The corresponding unit vector.
    pub fn unit(self) -> Vec3 {
        match self {
            Axis::X => Vec3::X,
            Axis::Y => Vec3::Y,
            Axis::Z => Vec3::Z,
        }
    }
}

/// Closest points between `ray` and the infinite line through `line_origin` with
/// unit direction `line_dir`. Returns `(ray_t, line_s)` ray/line parameters, or
/// `None` if the two are parallel.
fn closest_params(ray: Ray, line_origin: Vec3, line_dir: Vec3) -> Option<(f32, f32)> {
    let d1 = ray.direction;
    let d2 = line_dir;
    let r = ray.origin - line_origin;
    let a = d1.dot(d1);
    let b = d1.dot(d2);
    let c = d2.dot(d2);
    let d = d1.dot(r);
    let e = d2.dot(r);
    let denom = a * c - b * b;
    if denom.abs() <= EPSILON {
        return None;
    }
    let ray_t = (b * e - c * d) / denom;
    let line_s = (a * e - b * d) / denom;
    Some((ray_t, line_s))
}

/// Tests `ray` against a finite axis handle approximated as a cylinder: the
/// segment `handle_origin → handle_origin + axis*length` with thickness
/// `radius`. Returns the ray parameter `t` at the closest approach if that
/// approach lies within `radius` of the segment and within the segment span;
/// else `None`.
pub fn ray_axis_handle_hit(
    ray: Ray,
    handle_origin: Vec3,
    axis: Axis,
    length: f32,
    radius: f32,
) -> Option<f32> {
    let dir = axis.unit();
    let (ray_t, line_s) = closest_params(ray, handle_origin, dir)?;
    if ray_t < 0.0 {
        return None;
    }
    let clamped = line_s.clamp(0.0, length);
    let on_ray = ray.at(ray_t);
    let on_segment = handle_origin + dir * clamped;
    if on_ray.distance(on_segment) <= radius {
        Some(ray_t)
    } else {
        None
    }
}

/// Scalar translation along `axis` between the closest points of `prev_ray` and
/// `curr_ray` on the axis line through `handle_origin`. Multiply by
/// `axis.unit()` for the world-space offset. Returns `0.0` if either ray is
/// parallel to the axis (degenerate).
pub fn axis_drag_delta(prev_ray: Ray, curr_ray: Ray, handle_origin: Vec3, axis: Axis) -> f32 {
    let dir = axis.unit();
    let prev = match closest_params(prev_ray, handle_origin, dir) {
        Some((_, s)) => s,
        None => return 0.0,
    };
    let curr = match closest_params(curr_ray, handle_origin, dir) {
        Some((_, s)) => s,
        None => return 0.0,
    };
    curr - prev
}

/// Ray parameter `t` at which `ray` meets the plane through `plane_origin` with
/// unit normal `plane_normal`. `None` if the ray is parallel to the plane or the
/// intersection is behind the origin (`t < 0`).
pub fn ray_plane_intersection(ray: Ray, plane_origin: Vec3, plane_normal: Vec3) -> Option<f32> {
    let denom = ray.direction.dot(plane_normal);
    if denom.abs() <= EPSILON {
        return None;
    }
    let t = (plane_origin - ray.origin).dot(plane_normal) / denom;
    if t < 0.0 {
        None
    } else {
        Some(t)
    }
}

/// World-space offset `(curr ray ∩ plane) − (prev ray ∩ plane)` for the plane
/// through `plane_origin` with unit normal `plane_normal`. Returns `Vec3::ZERO`
/// if either ray is parallel to (or behind) the plane.
pub fn plane_drag_delta(
    prev_ray: Ray,
    curr_ray: Ray,
    plane_origin: Vec3,
    plane_normal: Vec3,
) -> Vec3 {
    let prev = match ray_plane_intersection(prev_ray, plane_origin, plane_normal) {
        Some(t) => prev_ray.at(t),
        None => return Vec3::ZERO,
    };
    let curr = match ray_plane_intersection(curr_ray, plane_origin, plane_normal) {
        Some(t) => curr_ray.at(t),
        None => return Vec3::ZERO,
    };
    curr - prev
}

/// Signed angle (radians, right-handed about `axis`) swept between the
/// intersections of `prev_ray` and `curr_ray` with the plane through `center`
/// whose normal is `axis.unit()`, measured around `center`. Returns `0.0` on
/// degeneracy (a ray parallel to the plane, or an intersection coincident with
/// `center`).
pub fn rotation_angle_around_axis(prev_ray: Ray, curr_ray: Ray, center: Vec3, axis: Axis) -> f32 {
    let normal = axis.unit();
    let prev = match ray_plane_intersection(prev_ray, center, normal) {
        Some(t) => prev_ray.at(t) - center,
        None => return 0.0,
    };
    let curr = match ray_plane_intersection(curr_ray, center, normal) {
        Some(t) => curr_ray.at(t) - center,
        None => return 0.0,
    };
    if prev.length() <= EPSILON || curr.length() <= EPSILON {
        return 0.0;
    }
    let cross = prev.cross(curr).dot(normal);
    let dot = prev.dot(curr);
    cross.atan2(dot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_core::ApproxEq;
    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn axis_unit_and_ray_at() {
        assert_eq!(Axis::X.unit(), Vec3::X);
        assert_eq!(Axis::Y.unit(), Vec3::Y);
        assert_eq!(Axis::Z.unit(), Vec3::Z);
        let r = Ray {
            origin: Vec3::ZERO,
            direction: Vec3::X,
        };
        assert!(r.at(3.0).approx_eq_default(Vec3::new(3.0, 0.0, 0.0)));
    }

    #[test]
    fn ray_plane_intersection_known_t() {
        let ray = Ray {
            origin: Vec3::new(0.0, 5.0, 0.0),
            direction: Vec3::NEG_Y,
        };
        let t = ray_plane_intersection(ray, Vec3::ZERO, Vec3::Y).unwrap();
        assert!(t.approx_eq_default(5.0));
    }

    #[test]
    fn ray_plane_parallel_and_behind() {
        let parallel = Ray {
            origin: Vec3::new(0.0, 5.0, 0.0),
            direction: Vec3::X,
        };
        assert!(ray_plane_intersection(parallel, Vec3::ZERO, Vec3::Y).is_none());
        let behind = Ray {
            origin: Vec3::new(0.0, 5.0, 0.0),
            direction: Vec3::Y,
        };
        assert!(ray_plane_intersection(behind, Vec3::ZERO, Vec3::Y).is_none());
    }

    #[test]
    fn ray_axis_handle_hit_and_miss() {
        // Straight-down ray through x=1 crosses the X axis at (1,0,0): a direct hit.
        let hit = Ray {
            origin: Vec3::new(1.0, 1.0, 0.0),
            direction: Vec3::NEG_Y,
        };
        let t = ray_axis_handle_hit(hit, Vec3::ZERO, Axis::X, 5.0, 0.25).unwrap();
        assert!(t.approx_eq_default(1.0));

        // Offset in Z so the closest approach is 0.3 away — outside a 0.1 radius.
        let miss_far = Ray {
            origin: Vec3::new(1.0, 1.0, 0.3),
            direction: Vec3::NEG_Y,
        };
        assert!(ray_axis_handle_hit(miss_far, Vec3::ZERO, Axis::X, 5.0, 0.1).is_none());

        // Closest point on the segment is clamped beyond its end at x=5.
        let beyond = Ray {
            origin: Vec3::new(9.0, 1.0, 0.0),
            direction: Vec3::NEG_Y,
        };
        assert!(ray_axis_handle_hit(beyond, Vec3::ZERO, Axis::X, 5.0, 0.25).is_none());
    }

    #[test]
    fn ray_axis_handle_radius_boundary() {
        // Z-offset of exactly 0.5 is the perpendicular distance to the X axis.
        let ray = Ray {
            origin: Vec3::new(1.0, 1.0, 0.5),
            direction: Vec3::NEG_Y,
        };
        assert!(ray_axis_handle_hit(ray, Vec3::ZERO, Axis::X, 5.0, 0.5).is_some());
        assert!(ray_axis_handle_hit(ray, Vec3::ZERO, Axis::X, 5.0, 0.4999).is_none());
    }

    #[test]
    fn axis_drag_delta_known_scalar() {
        let prev = Ray {
            origin: Vec3::new(2.0, 1.0, 0.0),
            direction: Vec3::NEG_Y,
        };
        let curr = Ray {
            origin: Vec3::new(5.0, 1.0, 0.0),
            direction: Vec3::NEG_Y,
        };
        let d = axis_drag_delta(prev, curr, Vec3::ZERO, Axis::X);
        assert!(d.approx_eq_default(3.0));
    }

    #[test]
    fn axis_drag_delta_parallel_is_zero() {
        let prev = Ray {
            origin: Vec3::new(0.0, 1.0, 0.0),
            direction: Vec3::X,
        };
        let curr = Ray {
            origin: Vec3::new(0.0, 2.0, 0.0),
            direction: Vec3::X,
        };
        assert!(axis_drag_delta(prev, curr, Vec3::ZERO, Axis::X).approx_eq_default(0.0));
    }

    #[test]
    fn plane_drag_delta_ground_plane() {
        let prev = Ray {
            origin: Vec3::new(1.0, 5.0, 1.0),
            direction: Vec3::NEG_Y,
        };
        let curr = Ray {
            origin: Vec3::new(4.0, 5.0, -2.0),
            direction: Vec3::NEG_Y,
        };
        let delta = plane_drag_delta(prev, curr, Vec3::ZERO, Vec3::Y);
        assert!(delta.approx_eq_default(Vec3::new(3.0, 0.0, -3.0)));
    }

    #[test]
    fn plane_drag_delta_parallel_is_zero() {
        let prev = Ray {
            origin: Vec3::new(1.0, 5.0, 1.0),
            direction: Vec3::X,
        };
        let curr = Ray {
            origin: Vec3::new(4.0, 5.0, -2.0),
            direction: Vec3::X,
        };
        assert!(plane_drag_delta(prev, curr, Vec3::ZERO, Vec3::Y).approx_eq_default(Vec3::ZERO));
    }

    #[test]
    fn rotation_angle_ninety_degrees_signed() {
        let prev = Ray {
            origin: Vec3::new(1.0, 5.0, 0.0),
            direction: Vec3::NEG_Y,
        };
        let curr = Ray {
            origin: Vec3::new(0.0, 5.0, 1.0),
            direction: Vec3::NEG_Y,
        };
        let angle = rotation_angle_around_axis(prev, curr, Vec3::ZERO, Axis::Y);
        assert!(angle.approx_eq(-FRAC_PI_2, 1e-5));
        let reverse = rotation_angle_around_axis(curr, prev, Vec3::ZERO, Axis::Y);
        assert!(reverse.approx_eq(FRAC_PI_2, 1e-5));
    }

    #[test]
    fn rotation_angle_degenerate_returns_zero() {
        let parallel = Ray {
            origin: Vec3::new(1.0, 5.0, 0.0),
            direction: Vec3::X,
        };
        let any = Ray {
            origin: Vec3::new(0.0, 5.0, 1.0),
            direction: Vec3::NEG_Y,
        };
        assert!(
            rotation_angle_around_axis(parallel, any, Vec3::ZERO, Axis::Y).approx_eq_default(0.0)
        );

        let at_center = Ray {
            origin: Vec3::new(0.0, 5.0, 0.0),
            direction: Vec3::NEG_Y,
        };
        assert!(
            rotation_angle_around_axis(at_center, any, Vec3::ZERO, Axis::Y).approx_eq_default(0.0)
        );
    }
}
