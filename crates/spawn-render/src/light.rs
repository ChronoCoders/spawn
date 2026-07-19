//! Directional lighting and its single shadow map.
//!
//! Honest 2b scope: **exactly one directional light and one shadow map** per
//! frame. The API exposes no list of lights, so the bound is structural, not just
//! documentary. No PBR, no cascades, no auto-fit: the light frustum is the
//! configured orthographic box ([`ShadowConfig`]).

use spawn_core::{Color, Mat4, Vec3, Vec4};

use crate::camera::Camera;
use crate::error::{RenderError, RenderResult};

/// A single directional light plus its shadow configuration.
#[derive(Debug, Clone, Copy)]
pub struct DirectionalLight {
    /// World-space travel direction of the light (normalized by the renderer
    /// before upload; a zero vector is a [`RenderError::ShadowConfigInvalid`]).
    pub direction: Vec3,
    pub color: Color,
    pub intensity: f32,
    /// Flat ambient term added regardless of the shadow test.
    pub ambient: Color,
    pub shadow: ShadowConfig,
}

/// The configured orthographic light frustum for the single shadow map. 2b does
/// not auto-fit to scene bounds (that is cascade work, out of scope).
#[derive(Debug, Clone, Copy)]
pub struct ShadowConfig {
    /// World point the light frustum is centered on.
    pub center: Vec3,
    /// Half-width of the orthographic frustum (must be `> 0`).
    pub extent: f32,
    pub near: f32,
    pub far: f32,
    /// Shadow map edge length in texels (must be `> 0`; a [`SizeSpec::Fixed`]
    /// transient is sized to this).
    ///
    /// [`SizeSpec::Fixed`]: crate::graph::SizeSpec::Fixed
    pub resolution: u32,
    /// Constant depth bias subtracted from the fragment's light-space depth to
    /// combat shadow acne.
    pub depth_bias: f32,
}

impl Default for ShadowConfig {
    fn default() -> Self {
        Self {
            center: Vec3::ZERO,
            extent: 10.0,
            near: 0.1,
            far: 50.0,
            resolution: 2048,
            depth_bias: 0.002,
        }
    }
}

impl ShadowConfig {
    /// Validates the frustum is non-degenerate. `Err(ShadowConfigInvalid)` for a
    /// non-positive extent, a zero resolution, or `far <= near`.
    pub fn validate(&self) -> RenderResult<()> {
        if self.extent.is_nan() || self.extent <= 0.0 {
            return Err(RenderError::ShadowConfigInvalid {
                context: "shadow frustum extent must be positive",
            });
        }
        if self.resolution == 0 {
            return Err(RenderError::ShadowConfigInvalid {
                context: "shadow map resolution must be non-zero",
            });
        }
        if self.near.is_nan() || self.far.is_nan() || self.far <= self.near {
            return Err(RenderError::ShadowConfigInvalid {
                context: "shadow frustum far must exceed near",
            });
        }
        Ok(())
    }
}

impl Default for DirectionalLight {
    fn default() -> Self {
        Self {
            direction: Vec3::new(-0.3, -1.0, -0.4),
            color: Color::WHITE,
            intensity: 1.0,
            ambient: Color::new(0.15, 0.15, 0.18, 1.0),
            shadow: ShadowConfig::default(),
        }
    }
}

/// The scene's lighting for a frame. 2b carries exactly one directional light.
#[derive(Debug, Clone, Copy, Default)]
pub struct Lighting {
    pub directional: DirectionalLight,
}

impl DirectionalLight {
    /// The orthographic light camera looking along [`direction`](Self::direction),
    /// centered on the shadow frustum. The shadow pass renders depth from this
    /// camera; the lit pass projects fragments into its clip space for the shadow
    /// test. `Err(ShadowConfigInvalid)` for a degenerate config or zero direction.
    pub(crate) fn shadow_camera(&self) -> RenderResult<Camera> {
        self.shadow.validate()?;
        let dir = self
            .direction
            .normalize()
            .ok_or(RenderError::ShadowConfigInvalid {
                context: "light direction must be non-zero",
            })?;
        // Place the eye so `center` sits at the middle of the [near, far] frustum,
        // looking along `dir` toward `center`.
        let mid = (self.shadow.near + self.shadow.far) * 0.5;
        let eye = self.shadow.center - dir * mid;
        // Any up vector not parallel to the view direction yields a valid basis.
        let up = if dir.cross(Vec3::Y).length_squared() < 1.0e-6 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        let e = self.shadow.extent;
        Camera::orthographic(
            eye,
            self.shadow.center,
            up,
            -e,
            e,
            -e,
            e,
            self.shadow.near,
            self.shadow.far,
        )
    }

    /// Builds the GPU light block from this light and its precomputed light
    /// view-projection. The direction is normalized here (a degenerate direction
    /// falls back to straight down, but [`shadow_camera`](Self::shadow_camera)
    /// has already rejected that case before this is reached).
    pub(crate) fn light_uniform(&self, light_view_proj: Mat4) -> LightUniform {
        let dir = self
            .direction
            .normalize()
            .unwrap_or_else(|| Vec3::new(0.0, -1.0, 0.0));
        let texel = 1.0 / self.shadow.resolution.max(1) as f32;
        LightUniform {
            direction: [dir.x, dir.y, dir.z, 0.0],
            color: [self.color.r, self.color.g, self.color.b, self.intensity],
            ambient: [self.ambient.r, self.ambient.g, self.ambient.b, 1.0],
            light_view_proj: columns(light_view_proj),
            shadow_params: [texel, self.shadow.depth_bias, PCF_RADIUS as f32, 0.0],
        }
    }
}

/// Fixed PCF kernel radius: a `(2R+1)×(2R+1)` comparison filter. `1` ⇒ 3×3.
const PCF_RADIUS: i32 = 1;

fn columns(m: Mat4) -> [[f32; 4]; 4] {
    let c = |v: Vec4| [v.x, v.y, v.z, v.w];
    [c(m.cols[0]), c(m.cols[1]), c(m.cols[2]), c(m.cols[3])]
}

/// GPU light block (group 2, binding 0). `#[repr(C)]` + `Pod`; std140-compatible
/// (every member 16-byte aligned), offsets asserted below. `color.w` carries
/// intensity; `shadow_params` is `[texel_size, depth_bias, pcf_radius, _]`.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct LightUniform {
    pub direction: [f32; 4],
    pub color: [f32; 4],
    pub ambient: [f32; 4],
    pub light_view_proj: [[f32; 4]; 4],
    pub shadow_params: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<LightUniform>() == 128);
const _: () = assert!(std::mem::offset_of!(LightUniform, direction) == 0);
const _: () = assert!(std::mem::offset_of!(LightUniform, color) == 16);
const _: () = assert!(std::mem::offset_of!(LightUniform, ambient) == 32);
const _: () = assert!(std::mem::offset_of!(LightUniform, light_view_proj) == 48);
const _: () = assert!(std::mem::offset_of!(LightUniform, shadow_params) == 112);

#[cfg(test)]
mod tests {
    use super::*;

    fn light() -> DirectionalLight {
        DirectionalLight {
            direction: Vec3::new(0.0, -1.0, -0.2),
            color: Color::WHITE,
            intensity: 1.0,
            ambient: Color::new(0.1, 0.1, 0.1, 1.0),
            shadow: ShadowConfig::default(),
        }
    }

    #[test]
    fn valid_config_builds_finite_shadow_camera() {
        let cam = light().shadow_camera().expect("valid shadow camera");
        let vp = cam.view_projection();
        for col in vp.cols {
            for v in [col.x, col.y, col.z, col.w] {
                assert!(v.is_finite(), "shadow view-projection must be finite");
            }
        }
    }

    #[test]
    fn zero_extent_is_rejected() {
        let mut l = light();
        l.shadow.extent = 0.0;
        assert!(matches!(
            l.shadow_camera(),
            Err(RenderError::ShadowConfigInvalid { .. })
        ));
    }

    #[test]
    fn zero_resolution_is_rejected() {
        let mut l = light();
        l.shadow.resolution = 0;
        assert!(matches!(
            l.shadow_camera(),
            Err(RenderError::ShadowConfigInvalid { .. })
        ));
    }

    #[test]
    fn far_not_exceeding_near_is_rejected() {
        let mut l = light();
        l.shadow.near = 10.0;
        l.shadow.far = 10.0;
        assert!(matches!(
            l.shadow_camera(),
            Err(RenderError::ShadowConfigInvalid { .. })
        ));
    }

    #[test]
    fn zero_direction_is_rejected() {
        let mut l = light();
        l.direction = Vec3::ZERO;
        assert!(matches!(
            l.shadow_camera(),
            Err(RenderError::ShadowConfigInvalid { .. })
        ));
    }

    #[test]
    fn uniform_carries_intensity_in_color_w_and_texel_size() {
        let l = light();
        let vp = l.shadow_camera().unwrap().view_projection();
        let u = l.light_uniform(vp);
        assert_eq!(u.color[3], 1.0, "intensity in color.w");
        assert!(
            (u.shadow_params[0] - 1.0 / 2048.0).abs() < 1e-9,
            "texel size"
        );
        assert_eq!(u.shadow_params[2], 1.0, "pcf radius");
    }
}
