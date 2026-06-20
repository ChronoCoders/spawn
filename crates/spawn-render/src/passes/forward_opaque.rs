//! The forward opaque pass: depth-tested (`Less`, write on), unblended draws.
//! The camera is bound via a per-pass dynamic offset (the graph writes each
//! pass's view-projection into its own slot), so multi-pass graphs do not clobber
//! a shared camera buffer.

use spawn_core::{Color, Mat4};

use crate::camera::Camera;
use crate::error::RenderResult;
use crate::light::Lighting;
use crate::material::{Material, PbrMaterial};
use crate::mesh::Mesh;
use crate::passes::overlay::Overlay;
use crate::pipeline::ModelUniform;
use crate::renderer::Renderer;
use crate::skeleton::GpuJoint;

/// The scene to render: one active camera, optional lighting (required by the
/// shadow, lit, and PBR passes), the caller-ordered unlit/lit draws, the PBR
/// draws (consumed by the `ForwardPbr` pass), the transparent draws (consumed by
/// the `Transparent` pass), and optional overlay data (the `spawn_ui` draw list +
/// editor lines, consumed by the `Overlay2D` pass).
pub struct RenderScene<'a> {
    pub camera: &'a Camera,
    pub lighting: Option<&'a Lighting>,
    pub draws: &'a [DrawItem<'a>],
    pub pbr_draws: &'a [PbrDrawItem<'a>],
    /// Alpha-blended draws shaded by the `Transparent` pass, sorted back-to-front
    /// each frame. Distinct from the opaque `draws`; they do not cast shadows.
    pub transparent: &'a [DrawItem<'a>],
    /// Unlit instanced batches: each `(mesh, material)` collapses to one
    /// `draw_indexed(.., 0..N)` over the per-instance storage buffer. Recorded in
    /// the `ForwardOpaque` pass and cast into the shadow map.
    pub instances: &'a [InstanceBatch<'a>],
    /// Physically based instanced batches, recorded in the `ForwardPbr` pass and
    /// cast into the shadow map.
    pub pbr_instances: &'a [PbrInstanceBatch<'a>],
    /// Unlit skinned draws (GPU vertex skinning), recorded in the `ForwardOpaque`
    /// pass and cast into the shadow map.
    pub skinned: &'a [SkinnedDrawItem<'a>],
    /// Physically based skinned draws, recorded in the `ForwardPbr` pass and cast
    /// into the shadow map.
    pub pbr_skinned: &'a [PbrSkinnedDrawItem<'a>],
    pub overlay: Option<Overlay<'a>>,
}

/// A single opaque draw: mesh + material + model-to-world transform.
pub struct DrawItem<'a> {
    pub mesh: &'a Mesh,
    pub material: &'a Material,
    pub model: Mat4,
}

/// A single physically based draw: mesh + PBR material + model-to-world
/// transform, shaded by the `ForwardPbr` pass and cast into the shadow map.
pub struct PbrDrawItem<'a> {
    pub mesh: &'a Mesh,
    pub material: &'a PbrMaterial,
    pub model: Mat4,
}

/// Per-instance data uploaded to the renderer's instance storage buffer and
/// indexed in the vertex shader by `@builtin(instance_index)`. `#[repr(C)]` +
/// `Pod`; member offsets asserted std430-compatible (16-byte-aligned lanes).
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct InstanceData {
    pub model: [[f32; 4]; 4],
    /// Per-instance color multiplier (applied to the base color); `[1,1,1,1]`
    /// leaves the material color unchanged (instanced == per-draw).
    pub tint: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<InstanceData>() == 80);
const _: () = assert!(std::mem::offset_of!(InstanceData, model) == 0);
const _: () = assert!(std::mem::offset_of!(InstanceData, tint) == 64);

impl InstanceData {
    /// An instance at `model` with a neutral (white) tint.
    pub fn from_model(model: Mat4) -> Self {
        let c = |v: spawn_core::Vec4| [v.x, v.y, v.z, v.w];
        Self {
            model: [
                c(model.cols[0]),
                c(model.cols[1]),
                c(model.cols[2]),
                c(model.cols[3]),
            ],
            tint: [1.0, 1.0, 1.0, 1.0],
        }
    }
}

/// An unlit instanced batch: one `(mesh, material)` drawn once for every entry in
/// `instances` via a single `draw_indexed(.., 0..instances.len())`.
pub struct InstanceBatch<'a> {
    pub mesh: &'a Mesh,
    pub material: &'a Material,
    pub instances: &'a [InstanceData],
}

/// A physically based instanced batch (the `ForwardPbr` analogue of
/// [`InstanceBatch`]).
pub struct PbrInstanceBatch<'a> {
    pub mesh: &'a Mesh,
    pub material: &'a PbrMaterial,
    pub instances: &'a [InstanceData],
}

/// An unlit skinned draw: a `SkinnedVertex` mesh plus the pre-composed per-joint
/// skinning matrices for this instance (uploaded to the joint storage buffer). The
/// `joints` length must match the skeleton that produced them.
pub struct SkinnedDrawItem<'a> {
    pub mesh: &'a Mesh,
    pub material: &'a Material,
    pub model: Mat4,
    pub joints: &'a [GpuJoint],
}

/// A physically based skinned draw (the `ForwardPbr` analogue of
/// [`SkinnedDrawItem`]).
pub struct PbrSkinnedDrawItem<'a> {
    pub mesh: &'a Mesh,
    pub material: &'a PbrMaterial,
    pub model: Mat4,
    pub joints: &'a [GpuJoint],
}

pub(crate) fn model_uniform(model: Mat4) -> ModelUniform {
    let c = |v: spawn_core::Vec4| [v.x, v.y, v.z, v.w];
    ModelUniform {
        model: [
            c(model.cols[0]),
            c(model.cols[1]),
            c(model.cols[2]),
            c(model.cols[3]),
        ],
    }
}

/// Records the opaque pass into `encoder` against `color_view`, with the
/// renderer's primary depth buffer as the depth attachment. `camera_offset` is
/// the dynamic offset of this pass's camera slot (written by the graph executor
/// before this call). Clears are applied per `clear_color`/`clear_depth`
/// (`None` ⇒ load). Looks up each material's pipeline in the cache — never builds
/// here; a miss is [`crate::error::RenderError::PipelineNotCached`]. No heap
/// allocation occurs.
pub(crate) fn record(
    renderer: &mut Renderer,
    encoder: &mut wgpu::CommandEncoder,
    color_view: &wgpu::TextureView,
    clear_color: Option<Color>,
    clear_depth: Option<f32>,
    camera_offset: u32,
    scene: &RenderScene,
) -> RenderResult<()> {
    renderer.ensure_model_capacity(scene.draws.len() as u32);
    for (i, draw) in scene.draws.iter().enumerate() {
        renderer.write_model(i as u32, &model_uniform(draw.model));
    }
    let skinned_base = skinned_model_base(scene);
    for (i, draw) in scene.skinned.iter().enumerate() {
        renderer.write_model(skinned_base + i as u32, &model_uniform(draw.model));
    }

    let color_load = match clear_color {
        Some(c) => wgpu::LoadOp::Clear(wgpu::Color {
            r: c.r as f64,
            g: c.g as f64,
            b: c.b as f64,
            a: c.a as f64,
        }),
        None => wgpu::LoadOp::Load,
    };
    let depth_load = match clear_depth {
        Some(d) => wgpu::LoadOp::Clear(d),
        None => wgpu::LoadOp::Load,
    };

    let depth_view = &renderer.depth_view;
    let camera_bind_group = &renderer.camera_bind_group;
    let instance_bind_group = renderer.instance_bind_group();
    let instanced_key = renderer.instanced_opaque_pipeline_key();
    let joint_bind_group = renderer.joint_bind_group();
    let skinned_key = renderer.skinned_opaque_pipeline_key();
    let joint_bases = &renderer.joint_bases;
    let cache = &renderer.cache;
    let model_stride = renderer.model_stride();

    let mut last_pipeline = None;

    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("spawn-forward-opaque"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: color_view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: color_load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: depth_view,
            depth_ops: Some(wgpu::Operations {
                load: depth_load,
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
    });

    for (i, draw) in scene.draws.iter().enumerate() {
        let key = draw.material.pipeline_key();
        if last_pipeline != Some(key) {
            let pipeline = cache.get(&key)?;
            pass.set_pipeline(pipeline);
            last_pipeline = Some(key);
        }
        let model_offset = (i as u64 * model_stride) as u32;
        pass.set_bind_group(0, camera_bind_group, &[camera_offset, model_offset]);
        pass.set_bind_group(1, draw.material.bind_group(), &[]);
        pass.set_vertex_buffer(0, draw.mesh.vertex_buffer().slice(..));
        pass.set_index_buffer(
            draw.mesh.index_buffer().slice(..),
            wgpu::IndexFormat::Uint32,
        );
        pass.draw_indexed(0..draw.mesh.index_count(), 0, 0..1);
    }

    // Instanced opaque batches: one `draw_indexed(.., base..base+N)` per batch over
    // the shared instance storage buffer (group 2). Slot `base` matches the upload
    // order in the frame executor: opaque batches occupy `[0, total_opaque)`.
    if !scene.instances.is_empty() {
        let pipeline = cache.get(&instanced_key)?;
        pass.set_pipeline(pipeline);
        pass.set_bind_group(2, instance_bind_group, &[]);
        let mut base = 0u32;
        for batch in scene.instances {
            let count = batch.instances.len() as u32;
            pass.set_bind_group(0, camera_bind_group, &[camera_offset, 0]);
            pass.set_bind_group(1, batch.material.bind_group(), &[]);
            pass.set_vertex_buffer(0, batch.mesh.vertex_buffer().slice(..));
            pass.set_index_buffer(
                batch.mesh.index_buffer().slice(..),
                wgpu::IndexFormat::Uint32,
            );
            pass.draw_indexed(0..batch.mesh.index_count(), 0, base..base + count);
            base += count;
        }
    }

    // Skinned opaque draws: each binds the joint storage at its dynamic-offset
    // base (group 2) and reads its model from the per-draw slot. One draw each.
    if !scene.skinned.is_empty() {
        let pipeline = cache.get(&skinned_key)?;
        pass.set_pipeline(pipeline);
        for (i, draw) in scene.skinned.iter().enumerate() {
            let model_offset = (u64::from(skinned_base + i as u32) * model_stride) as u32;
            pass.set_bind_group(0, camera_bind_group, &[camera_offset, model_offset]);
            pass.set_bind_group(1, draw.material.bind_group(), &[]);
            pass.set_bind_group(2, joint_bind_group, &[joint_bases[i]]);
            pass.set_vertex_buffer(0, draw.mesh.vertex_buffer().slice(..));
            pass.set_index_buffer(
                draw.mesh.index_buffer().slice(..),
                wgpu::IndexFormat::Uint32,
            );
            pass.draw_indexed(0..draw.mesh.index_count(), 0, 0..1);
        }
    }

    Ok(())
}

/// Total number of opaque instances across `scene.instances`. PBR instances are
/// uploaded into the shared storage buffer immediately after this range, so the
/// PBR and shadow passes use this as their instance base.
pub(crate) fn opaque_instance_total(scene: &RenderScene) -> u32 {
    scene
        .instances
        .iter()
        .map(|b| b.instances.len() as u32)
        .sum()
}

/// First model-buffer slot for the opaque-skinned draws (after the lit, PBR, and
/// transparent draws, which also use the per-draw model buffer).
pub(crate) fn skinned_model_base(scene: &RenderScene) -> u32 {
    (scene.draws.len() + scene.pbr_draws.len() + scene.transparent.len()) as u32
}

/// First model-buffer slot for the PBR-skinned draws (after the opaque-skinned
/// draws).
pub(crate) fn pbr_skinned_model_base(scene: &RenderScene) -> u32 {
    skinned_model_base(scene) + scene.skinned.len() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_core::Vec3;

    #[test]
    fn instance_from_model_is_identity_with_white_tint() {
        let i = InstanceData::from_model(Mat4::IDENTITY);
        assert_eq!(i.tint, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(
            i.model,
            [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ]
        );
    }

    #[test]
    fn instance_from_model_preserves_translation_column() {
        let m = Mat4::from_translation(Vec3::new(2.0, 3.0, 4.0));
        let i = InstanceData::from_model(m);
        assert_eq!(i.model[3], [2.0, 3.0, 4.0, 1.0]);
    }

    #[test]
    fn instance_data_packs_to_eighty_bytes() {
        // std430 array stride for the storage buffer: model (64) + tint (16).
        assert_eq!(std::mem::size_of::<InstanceData>(), 80);
        assert_eq!(
            bytemuck::bytes_of(&InstanceData::from_model(Mat4::IDENTITY)).len(),
            80
        );
    }
}
