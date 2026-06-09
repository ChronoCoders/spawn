//! Frame lifecycle: surface acquire with loss recovery, pass execution, submit
//! and present.

use crate::camera::CameraUniform;
use crate::error::{RenderError, RenderResult};
use crate::graph::{CompiledGraph, PassKind};
use crate::passes::forward_opaque::RenderScene;
use crate::passes::{forward_lit, forward_opaque, shadow_depth};
use crate::renderer::Renderer;

/// Holds the acquired surface texture, its view, and the command encoder for one
/// frame. Borrows the `&mut Renderer`. The surface texture is presented or
/// dropped at [`FrameContext::end_frame`] and is never retained across frames.
pub struct FrameContext<'a, 'w> {
    renderer: &'a mut Renderer<'w>,
    surface_texture: Option<wgpu::SurfaceTexture>,
    color_view: wgpu::TextureView,
    encoder: Option<wgpu::CommandEncoder>,
}

/// What `begin_frame` does with a `wgpu::SurfaceError` on first acquire.
///
/// Factored out as a pure decision so the recovery policy (§5) is unit-testable
/// without a live surface: `Lost`/`Outdated` are recoverable (reconfigure once,
/// retry once); the rest map directly to terminal errors for this acquire.
#[derive(Debug)]
pub(crate) enum SurfaceAction {
    /// Reconfigure the surface once and retry acquire once.
    Recover,
    /// Give up this acquire with the given error (non-recoverable here).
    Fail(RenderError),
}

/// Pure mapping from a first-acquire `wgpu::SurfaceError` to the recovery action.
///
/// `Lost`/`Outdated` → [`SurfaceAction::Recover`]; `Timeout` →
/// `Fail(SurfaceTimeout)`; `OutOfMemory` → `Fail(OutOfMemory)`.
pub(crate) fn surface_action(err: &wgpu::SurfaceError) -> SurfaceAction {
    match err {
        wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated => SurfaceAction::Recover,
        wgpu::SurfaceError::Timeout => SurfaceAction::Fail(RenderError::SurfaceTimeout),
        wgpu::SurfaceError::OutOfMemory => SurfaceAction::Fail(RenderError::OutOfMemory),
    }
}

/// Maps the renderer's device-lost flag to a frame result.
///
/// Device-lost detection contract (§5/§11): wgpu 22 `Queue::submit` returns a
/// `SubmissionIndex`, never a `Result`, so submission cannot report device loss.
/// The `Renderer` instead owns an atomic flag set by wgpu's device-lost callback
/// (registered in [`crate::renderer::Renderer::new`]). `begin_frame` and
/// `end_frame` consult this seam: `true` ⇒ `Err(RenderError::DeviceLost)`,
/// `false` ⇒ `Ok(())`. Factored out as a pure function so the flag→error mapping
/// is unit-testable without forcing a real (un-forceable headless) device loss.
pub(crate) fn device_lost_error(device_lost: bool) -> RenderResult<()> {
    if device_lost {
        Err(RenderError::DeviceLost)
    } else {
        Ok(())
    }
}

impl<'w> Renderer<'w> {
    /// Acquires the surface texture and creates the frame encoder.
    ///
    /// Returns [`RenderError::DeviceLost`] immediately if the device-lost flag is
    /// set (see [`device_lost_error`]). On `Lost`/`Outdated` the surface is
    /// reconfigured once and acquire is retried once; a second failure returns
    /// [`RenderError::Surface`]. `Timeout` maps to [`RenderError::SurfaceTimeout`]
    /// (skippable), `OutOfMemory` to [`RenderError::OutOfMemory`] (fatal). Never
    /// panics.
    pub fn begin_frame(&mut self) -> RenderResult<FrameContext<'_, 'w>> {
        device_lost_error(self.is_device_lost())?;
        let surface_texture = match self.surface.get_current_texture() {
            Ok(tex) => tex,
            Err(err) => match surface_action(&err) {
                SurfaceAction::Recover => {
                    self.reconfigure();
                    self.surface
                        .get_current_texture()
                        .map_err(|_| RenderError::Surface)?
                }
                SurfaceAction::Fail(mapped) => return Err(mapped),
            },
        };

        let color_view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("spawn-frame-encoder"),
            });

        Ok(FrameContext {
            renderer: self,
            surface_texture: Some(surface_texture),
            color_view,
            encoder: Some(encoder),
        })
    }
}

impl FrameContext<'_, '_> {
    /// Records every pass of the compiled `graph` in derived execution order
    /// against `scene`. Each pass's camera view-projection is written into its own
    /// dynamic-offset slot (no clobber across passes). Color targets resolve to
    /// the live surface or a compiled transient; the primary depth buffer is the
    /// renderer's. No heap allocation occurs in this path.
    pub fn execute(&mut self, graph: &CompiledGraph, scene: &RenderScene) -> RenderResult<()> {
        self.renderer
            .ensure_camera_capacity(graph.order().len() as u32);
        let camera_stride = self.renderer.camera_stride();
        let scene_camera = scene.camera.uniform();

        // Lighting is shared across the shadow and lit passes: the shadow pass
        // renders from the light's view-projection, the lit pass projects
        // fragments into that same clip space. Compute both once; upload the light
        // block once.
        let shadow_camera: Option<CameraUniform> = match scene.lighting {
            Some(lighting) => {
                let cam = lighting.directional.shadow_camera()?;
                self.renderer
                    .write_light(&lighting.directional.light_uniform(cam.view_projection()));
                Some(cam.uniform())
            }
            None => None,
        };

        let encoder = self.encoder.as_mut().ok_or(RenderError::InvalidArgument {
            context: "frame encoder already consumed",
        })?;
        for (slot, &pass_idx) in graph.order().iter().enumerate() {
            let pass = graph.pass(pass_idx);
            let camera_offset = (slot as u64 * camera_stride) as u32;
            match pass.kind {
                PassKind::ShadowDepth => {
                    let camera = shadow_camera.ok_or(RenderError::InvalidArgument {
                        context: "shadow pass requires scene lighting",
                    })?;
                    self.renderer.write_camera_slot(slot as u32, &camera);
                    let depth = pass.depth.ok_or(RenderError::InvalidArgument {
                        context: "shadow pass needs a depth target",
                    })?;
                    let depth_view =
                        graph
                            .transient_view(depth.target)
                            .ok_or(RenderError::InvalidArgument {
                                context: "shadow depth target is not a known transient",
                            })?;
                    shadow_depth::record(
                        self.renderer,
                        encoder,
                        depth_view,
                        depth.clear,
                        camera_offset,
                        scene,
                    )?;
                }
                PassKind::ForwardOpaque | PassKind::ForwardLit => {
                    self.renderer.write_camera_slot(slot as u32, &scene_camera);
                    let color = pass.color.ok_or(RenderError::InvalidArgument {
                        context: "forward pass needs a color target",
                    })?;
                    let color_view: &wgpu::TextureView = if graph.is_surface(color.target) {
                        &self.color_view
                    } else {
                        graph
                            .transient_view(color.target)
                            .ok_or(RenderError::InvalidArgument {
                                context:
                                    "color target is neither the surface nor a known transient",
                            })?
                    };
                    let clear_depth = pass.depth.and_then(|d| d.clear);
                    if pass.kind == PassKind::ForwardLit {
                        let light_bind_group =
                            graph
                                .light_bind_group()
                                .ok_or(RenderError::InvalidArgument {
                                    context: "lit pass requires a compiled light bind group",
                                })?;
                        forward_lit::record(
                            self.renderer,
                            encoder,
                            color_view,
                            color.clear,
                            clear_depth,
                            camera_offset,
                            light_bind_group,
                            scene,
                        )?;
                    } else {
                        forward_opaque::record(
                            self.renderer,
                            encoder,
                            color_view,
                            color.clear,
                            clear_depth,
                            camera_offset,
                            scene,
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Submits exactly one command buffer and presents the surface texture.
    ///
    /// Returns [`RenderError::DeviceLost`] if the device-lost flag was set before
    /// submission. Because wgpu 22 `Queue::submit` returns a `SubmissionIndex`
    /// (not a `Result`), loss detected during submission is observed on the next
    /// frame via the device-lost callback rather than from `submit` itself (see
    /// [`device_lost_error`]). The surface texture is consumed here and never
    /// retained.
    pub fn end_frame(mut self) -> RenderResult<()> {
        let encoder = self.encoder.take().ok_or(RenderError::InvalidArgument {
            context: "frame encoder already consumed",
        })?;
        let surface_texture = self
            .surface_texture
            .take()
            .ok_or(RenderError::InvalidArgument {
                context: "surface texture already presented",
            })?;
        device_lost_error(self.renderer.is_device_lost())?;
        self.renderer
            .queue
            .submit(std::iter::once(encoder.finish()));
        surface_texture.present();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lost_and_outdated_recover_with_one_reconfigure_retry() {
        assert!(matches!(
            surface_action(&wgpu::SurfaceError::Lost),
            SurfaceAction::Recover
        ));
        assert!(matches!(
            surface_action(&wgpu::SurfaceError::Outdated),
            SurfaceAction::Recover
        ));
    }

    #[test]
    fn timeout_maps_to_surface_timeout() {
        assert!(matches!(
            surface_action(&wgpu::SurfaceError::Timeout),
            SurfaceAction::Fail(RenderError::SurfaceTimeout)
        ));
    }

    #[test]
    fn out_of_memory_maps_to_out_of_memory() {
        assert!(matches!(
            surface_action(&wgpu::SurfaceError::OutOfMemory),
            SurfaceAction::Fail(RenderError::OutOfMemory)
        ));
    }

    #[test]
    fn device_lost_flag_maps_to_device_lost_error() {
        // The callback sets the flag; the seam consulted by begin_frame/end_frame
        // turns a set flag into RenderError::DeviceLost and a clear flag into Ok.
        assert!(device_lost_error(false).is_ok());
        assert!(matches!(
            device_lost_error(true),
            Err(RenderError::DeviceLost)
        ));
    }
}
