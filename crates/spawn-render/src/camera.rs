//! View/projection state and its GPU uniform.
//!
//! Conventions (inherited from spawn-core, normative here): right-handed,
//! column-major matrices (`M * v`), depth range `[0, 1]`. The view-projection is
//! `projection * view`.

use spawn_core::{Mat4, Vec3, Vec4};

use crate::error::{RenderError, RenderResult};

/// Camera transform pair. `view` is world→view, `projection` is view→clip.
pub struct Camera {
    pub view: Mat4,
    pub projection: Mat4,
}

/// GPU-side camera block. `#[repr(C)]` + `Pod`; field offsets asserted below for
/// std140-compatible layout (mat4 then vec4).
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CameraUniform {
    pub view_proj: [[f32; 4]; 4],
    pub view_pos: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<CameraUniform>() == 80);
const _: () = assert!(std::mem::offset_of!(CameraUniform, view_proj) == 0);
const _: () = assert!(std::mem::offset_of!(CameraUniform, view_pos) == 64);

fn columns(m: Mat4) -> [[f32; 4]; 4] {
    let c = |v: Vec4| [v.x, v.y, v.z, v.w];
    [c(m.cols[0]), c(m.cols[1]), c(m.cols[2]), c(m.cols[3])]
}

impl Camera {
    pub fn new(view: Mat4, projection: Mat4) -> Self {
        Self { view, projection }
    }

    /// Builds a perspective camera. `Err(InvalidArgument)` if `look_at_rh` or
    /// `perspective_rh` is degenerate (returns `None`).
    // Arity is fixed by the spec §4 signature (eye/target/up + fov/aspect/near/
    // far): each parameter is a distinct camera input, so grouping them into a
    // struct would only obscure the spec-mandated public signature.
    #[allow(clippy::too_many_arguments)]
    pub fn perspective(
        eye: Vec3,
        target: Vec3,
        up: Vec3,
        fov_y: f32,
        aspect: f32,
        near: f32,
        far: f32,
    ) -> RenderResult<Self> {
        let view = Mat4::look_at_rh(eye, target, up).ok_or(RenderError::InvalidArgument {
            context: "degenerate camera view (eye/target/up)",
        })?;
        let projection =
            Mat4::perspective_rh(fov_y, aspect, near, far).ok_or(RenderError::InvalidArgument {
                context: "invalid perspective projection parameters",
            })?;
        Ok(Self { view, projection })
    }

    /// Builds an orthographic camera. `Err(InvalidArgument)` if `look_at_rh` or
    /// `orthographic_rh` is degenerate.
    // Arity is fixed by the spec §4 signature (eye/target/up + the six ortho
    // frustum bounds): each parameter is a distinct camera input, so grouping
    // them into a struct would only obscure the spec-mandated public signature.
    #[allow(clippy::too_many_arguments)]
    pub fn orthographic(
        eye: Vec3,
        target: Vec3,
        up: Vec3,
        left: f32,
        right: f32,
        bottom: f32,
        top: f32,
        near: f32,
        far: f32,
    ) -> RenderResult<Self> {
        let view = Mat4::look_at_rh(eye, target, up).ok_or(RenderError::InvalidArgument {
            context: "degenerate camera view (eye/target/up)",
        })?;
        let projection = Mat4::orthographic_rh(left, right, bottom, top, near, far).ok_or(
            RenderError::InvalidArgument {
                context: "invalid orthographic projection parameters",
            },
        )?;
        Ok(Self { view, projection })
    }

    /// `projection * view` (column-major).
    pub fn view_projection(&self) -> Mat4 {
        self.projection * self.view
    }

    /// Eye position recovered from the inverse view, or the origin if the view
    /// is singular (never for a well-formed camera).
    fn eye_position(&self) -> [f32; 4] {
        match self.view.inverse() {
            Some(inv) => [inv.cols[3].x, inv.cols[3].y, inv.cols[3].z, 1.0],
            None => [0.0, 0.0, 0.0, 1.0],
        }
    }

    pub fn uniform(&self) -> CameraUniform {
        CameraUniform {
            view_proj: columns(self.view_projection()),
            view_pos: self.eye_position(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perspective_rejects_bad_params() {
        let e = Vec3::new(0.0, 0.0, 5.0);
        let t = Vec3::new(0.0, 0.0, 0.0);
        let up = Vec3::new(0.0, 1.0, 0.0);
        assert!(Camera::perspective(e, t, up, -1.0, 1.0, 0.1, 100.0).is_err());
        assert!(Camera::perspective(e, t, up, 1.0, 1.0, 100.0, 0.1).is_err());
        assert!(Camera::perspective(e, t, up, 1.0, 1.0, 0.1, 100.0).is_ok());
    }

    #[test]
    fn view_projection_is_projection_times_view() {
        let cam = Camera::new(
            Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0)),
            Mat4::IDENTITY,
        );
        assert_eq!(cam.view_projection(), cam.view);
    }

    #[test]
    fn uniform_recovers_eye_position() {
        let eye = Vec3::new(0.0, 0.0, 5.0);
        let cam = Camera::perspective(
            eye,
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            1.0,
            1.0,
            0.1,
            100.0,
        )
        .expect("valid camera");
        let u = cam.uniform();
        assert!((u.view_pos[2] - 5.0).abs() < 1e-4);
    }
}
