//! Frame lifecycle: surface acquire with loss recovery, pass execution, submit
//! and present.

use crate::camera::CameraUniform;
use crate::error::{RenderError, RenderResult};
use crate::graph::{CompiledGraph, PassKind};
use crate::passes::forward_opaque::RenderScene;
use crate::passes::{
    forward_lit, forward_opaque, forward_pbr, overlay, post, shadow_depth, transparent,
};
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

/// Rounds `value` up to the next multiple of `align` (a power of two ≥ 1). Used to
/// place each skinned draw's joint block at a storage-offset-aligned base.
fn align_up_u64(value: u64, align: u64) -> u64 {
    if align <= 1 {
        return value;
    }
    value.div_ceil(align) * align
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
        // Size the shared per-draw model buffer once, up front, for every list
        // that uses it so no pass reallocates mid-frame (which would invalidate
        // already-recorded bind groups). The lists occupy disjoint slot ranges:
        // `draws` `[0, D)`, `pbr_draws` `[D, D+P)`, `transparent` `[D+P, D+P+T)`,
        // `skinned` next, then `pbr_skinned` (instanced draws read the storage
        // buffer, not the model buffer).
        self.renderer.ensure_model_capacity(
            (scene.draws.len()
                + scene.pbr_draws.len()
                + scene.transparent.len()
                + scene.skinned.len()
                + scene.pbr_skinned.len()) as u32,
        );
        // Upload all instance data once, up front, into the shared storage buffer:
        // opaque batches first (`[0, total_opaque)`), then PBR batches. The opaque,
        // PBR, and shadow record paths recompute the same base offsets, so the
        // per-batch `draw_indexed(.., base..base+N)` ranges stay consistent.
        let opaque_instances: u32 = scene
            .instances
            .iter()
            .map(|b| b.instances.len() as u32)
            .sum();
        let pbr_instances: u32 = scene
            .pbr_instances
            .iter()
            .map(|b| b.instances.len() as u32)
            .sum();
        self.renderer
            .ensure_instance_capacity(opaque_instances + pbr_instances);
        {
            let mut base = 0u32;
            for batch in scene.instances {
                self.renderer.write_instances(base, batch.instances);
                base += batch.instances.len() as u32;
            }
            for batch in scene.pbr_instances {
                self.renderer.write_instances(base, batch.instances);
                base += batch.instances.len() as u32;
            }
        }
        // Upload skinned joint matrices once, up front: opaque-skinned draws then
        // PBR-skinned, each block aligned to the storage offset alignment with a
        // window of slack past the last block (the dynamic-offset binding window).
        // The per-draw byte bases are recorded for the record paths.
        {
            let align = self.renderer.joint_align();
            let max_joints = self.renderer.max_joints_per_draw();
            let window = max_joints * 64;
            let mut bases = std::mem::take(&mut self.renderer.joint_bases);
            bases.clear();
            let mut cur = 0u64;
            for draw in scene.skinned {
                if draw.joints.len() as u64 > max_joints {
                    self.renderer.joint_bases = bases;
                    return Err(RenderError::InstanceBufferOverflow {
                        context: "skinned draw exceeds the joint window",
                    });
                }
                bases.push(cur as u32);
                cur += align_up_u64(draw.joints.len() as u64 * 64, align);
            }
            for draw in scene.pbr_skinned {
                if draw.joints.len() as u64 > max_joints {
                    self.renderer.joint_bases = bases;
                    return Err(RenderError::InstanceBufferOverflow {
                        context: "skinned draw exceeds the joint window",
                    });
                }
                bases.push(cur as u32);
                cur += align_up_u64(draw.joints.len() as u64 * 64, align);
            }
            if !bases.is_empty() {
                self.renderer.ensure_joint_capacity(cur + window);
                let opaque_skinned = scene.skinned.len();
                for (i, draw) in scene.skinned.iter().enumerate() {
                    self.renderer.write_joints(u64::from(bases[i]), draw.joints);
                }
                for (i, draw) in scene.pbr_skinned.iter().enumerate() {
                    self.renderer
                        .write_joints(u64::from(bases[opaque_skinned + i]), draw.joints);
                }
            }
            self.renderer.joint_bases = bases;
        }
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
                PassKind::ForwardOpaque
                | PassKind::ForwardLit
                | PassKind::ForwardPbr
                | PassKind::Transparent => {
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
                    match pass.kind {
                        PassKind::ForwardLit => {
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
                        }
                        PassKind::ForwardPbr => {
                            let light_bind_group =
                                graph
                                    .light_bind_group()
                                    .ok_or(RenderError::InvalidArgument {
                                        context: "PBR pass requires a compiled light bind group",
                                    })?;
                            forward_pbr::record(
                                self.renderer,
                                encoder,
                                color_view,
                                color.clear,
                                clear_depth,
                                camera_offset,
                                light_bind_group,
                                scene,
                            )?;
                        }
                        PassKind::Transparent => {
                            let light_bind_group =
                                graph
                                    .light_bind_group()
                                    .ok_or(RenderError::InvalidArgument {
                                        context:
                                            "transparent pass requires a compiled light bind group",
                                    })?;
                            transparent::record(
                                self.renderer,
                                encoder,
                                color_view,
                                color.clear,
                                clear_depth,
                                camera_offset,
                                light_bind_group,
                                scene,
                            )?;
                        }
                        _ => {
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
                PassKind::BloomBright
                | PassKind::BloomBlur
                | PassKind::BloomComposite
                | PassKind::Tonemap
                | PassKind::Fxaa => {
                    let color = pass.color.ok_or(RenderError::InvalidArgument {
                        context: "post pass needs a color target",
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
                    let input_bind_group =
                        graph
                            .fullscreen_binding(pass_idx)
                            .ok_or(RenderError::InvalidArgument {
                                context: "post pass requires a compiled input bind group",
                            })?;
                    let pipeline_key = match pass.kind {
                        PassKind::BloomBright => self.renderer.bloom_bright_pipeline_key(),
                        PassKind::BloomBlur => self.renderer.bloom_blur_pipeline_key(),
                        PassKind::BloomComposite => self.renderer.bloom_composite_pipeline_key(),
                        PassKind::Fxaa => self.renderer.fxaa_pipeline_key(),
                        _ => self.renderer.tonemap_pipeline_key(),
                    };
                    post::record_fullscreen(
                        self.renderer,
                        encoder,
                        color_view,
                        color.clear,
                        pipeline_key,
                        input_bind_group,
                    )?;
                }
                PassKind::Overlay2D => {
                    // The overlay projects world-space lines with the scene
                    // camera; UI quads are already in clip space.
                    self.renderer.write_camera_slot(slot as u32, &scene_camera);
                    let color = pass.color.ok_or(RenderError::InvalidArgument {
                        context: "overlay pass needs a color target",
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
                    let data = scene.overlay.as_ref().ok_or(RenderError::InvalidArgument {
                        context: "overlay pass requires scene overlay data",
                    })?;
                    overlay::record(self.renderer, encoder, color_view, data, camera_offset)?;
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
