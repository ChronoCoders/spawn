//! The viewport orbit camera.
//!
//! Orbit/pan/dolly around a target, producing the `spawn_render::Camera` the lit
//! pass renders from and the world ray picking/gizmos consume. Pure math over
//! spawn-core types (headless-testable); free-fly and multi-viewport are deferred.

use spawn_core::{Mat4, Rect, Vec2, Vec3, Vec4};
use spawn_editor::Ray;
use spawn_render::Camera;

use crate::error::{ShellError, ShellResult};

const ORBIT_SENSITIVITY: f32 = 0.008;
const PAN_SENSITIVITY: f32 = 0.0015;
const DOLLY_SENSITIVITY: f32 = 0.12;
const MAX_PITCH: f32 = 1.5533; // ~89°
const MIN_DISTANCE: f32 = 0.25;
const MAX_DISTANCE: f32 = 5000.0;
const FOV_Y: f32 = std::f32::consts::FRAC_PI_3; // 60°
const NEAR: f32 = 0.05;
const FAR: f32 = 5000.0;

/// An orbit camera: a target point with yaw/pitch and a distance.
#[derive(Debug, Clone, Copy)]
pub struct EditorCamera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

impl Default for EditorCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            yaw: 0.7,
            pitch: 0.5,
            distance: 8.0,
        }
    }
}

impl EditorCamera {
    /// The world-space eye position derived from the orbit parameters.
    pub fn eye(&self) -> Vec3 {
        let cp = self.pitch.cos();
        let dir = Vec3::new(cp * self.yaw.sin(), self.pitch.sin(), cp * self.yaw.cos());
        self.target + dir * self.distance
    }

    /// Camera right/up basis (for panning), from the look direction and world up.
    fn basis(&self) -> (Vec3, Vec3) {
        let forward = (self.target - self.eye()).normalize_or_zero();
        let right = forward.cross(Vec3::Y).normalize_or_zero();
        let up = right.cross(forward).normalize_or_zero();
        (right, up)
    }

    /// Orbits by a pixel-space drag delta (yaw from x, pitch from y, clamped).
    pub fn orbit(&mut self, delta: Vec2) {
        self.yaw -= delta.x * ORBIT_SENSITIVITY;
        self.pitch = (self.pitch + delta.y * ORBIT_SENSITIVITY).clamp(-MAX_PITCH, MAX_PITCH);
    }

    /// Pans the target in the camera's right/up plane by a pixel-space delta,
    /// scaled by distance so panning feels constant on screen.
    pub fn pan(&mut self, delta: Vec2) {
        let (right, up) = self.basis();
        let scale = PAN_SENSITIVITY * self.distance;
        self.target = self.target + right * (-delta.x * scale) + up * (delta.y * scale);
    }

    /// Dollies in/out (`amount > 0` zooms in), clamped to a sane range.
    pub fn dolly(&mut self, amount: f32) {
        self.distance =
            (self.distance * (1.0 - amount * DOLLY_SENSITIVITY)).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// Builds the render camera for the given viewport aspect.
    pub fn camera(&self, aspect: f32) -> ShellResult<Camera> {
        Ok(Camera::perspective(
            self.eye(),
            self.target,
            Vec3::Y,
            FOV_Y,
            aspect.max(1.0e-3),
            NEAR,
            FAR,
        )?)
    }

    /// Unprojects a viewport-relative pixel into a world-space picking ray.
    pub fn ray(&self, pixel: Vec2, viewport: Rect, aspect: f32) -> ShellResult<Ray> {
        let inv =
            self.camera(aspect)?
                .view_projection()
                .inverse()
                .ok_or(ShellError::InvalidState {
                    context: "camera view-projection is not invertible",
                })?;
        let w = viewport.width().max(1.0);
        let h = viewport.height().max(1.0);
        let ndc_x = (pixel.x - viewport.min.x) / w * 2.0 - 1.0;
        let ndc_y = 1.0 - (pixel.y - viewport.min.y) / h * 2.0;
        let near = unproject(inv, ndc_x, ndc_y, 0.0);
        let far = unproject(inv, ndc_x, ndc_y, 1.0);
        Ok(Ray {
            origin: near,
            direction: (far - near).normalize_or_zero(),
        })
    }
}

/// Unprojects a clip-space point through the inverse view-projection, dividing by
/// `w`. A degenerate `w` falls back to `1` (the result is then a direction-only
/// approximation, never a panic).
fn unproject(inv: Mat4, ndc_x: f32, ndc_y: f32, ndc_z: f32) -> Vec3 {
    let p = inv * Vec4::new(ndc_x, ndc_y, ndc_z, 1.0);
    let w = if p.w.abs() > 1.0e-9 { p.w } else { 1.0 };
    Vec3::new(p.x / w, p.y / w, p.z / w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_core::ApproxEq;

    #[test]
    fn orbit_and_dolly_mutate_within_bounds() {
        let mut c = EditorCamera::default();
        let p0 = c.pitch;
        c.orbit(Vec2::new(10.0, 1000.0));
        assert!(c.pitch <= MAX_PITCH && c.pitch >= -MAX_PITCH);
        assert!(c.pitch != p0);
        let d0 = c.distance;
        c.dolly(1.0);
        assert!(c.distance < d0 && c.distance >= MIN_DISTANCE);
    }

    #[test]
    fn camera_builds_finite_view_projection() {
        let c = EditorCamera::default();
        let cam = c.camera(16.0 / 9.0).expect("camera");
        for col in cam.view_projection().cols {
            for v in [col.x, col.y, col.z, col.w] {
                assert!(v.is_finite());
            }
        }
    }

    #[test]
    fn center_ray_points_from_eye_toward_target() {
        // Camera looking at the origin; a ray through the viewport center should
        // start near the eye and point roughly toward the target.
        let c = EditorCamera {
            target: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            distance: 5.0,
        };
        let rect = Rect::new(Vec2::ZERO, Vec2::new(100.0, 100.0));
        let ray = c.ray(Vec2::new(50.0, 50.0), rect, 1.0).expect("ray");
        // eye is at +Z (yaw=pitch=0 → dir = +Z), so the center ray points -Z.
        assert!(ray.direction.z < 0.0);
        assert!(ray.direction.normalize().is_some());
    }

    #[test]
    fn pan_moves_target() {
        let mut c = EditorCamera::default();
        let t0 = c.target;
        c.pan(Vec2::new(50.0, 0.0));
        assert!(!c.target.approx_eq_default(t0));
    }
}
