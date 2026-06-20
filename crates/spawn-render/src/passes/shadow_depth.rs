//! The shadow caster pass: renders the scene depth-only from the light's
//! orthographic camera into the shadow map. No color attachment, no material or
//! light binds — only group 0 (the light view-projection in this pass's camera
//! slot, plus the per-draw model). The lit pass samples the resulting depth.

use crate::error::RenderResult;
use crate::passes::forward_opaque::{
    model_uniform, opaque_instance_total, pbr_skinned_model_base, skinned_model_base, RenderScene,
};
use crate::renderer::Renderer;

/// Records the depth-only shadow pass into `encoder` against `depth_view` (the
/// transient shadow map). `camera_offset` selects this pass's camera slot, which
/// the executor has filled with the light view-projection. Uses the renderer's
/// built-in shadow pipeline; never builds a pipeline here. No heap allocation.
pub(crate) fn record(
    renderer: &mut Renderer,
    encoder: &mut wgpu::CommandEncoder,
    depth_view: &wgpu::TextureView,
    clear_depth: Option<f32>,
    camera_offset: u32,
    scene: &RenderScene,
) -> RenderResult<()> {
    let base = scene.draws.len() as u32;
    renderer.ensure_model_capacity(base + scene.pbr_draws.len() as u32);
    for (i, draw) in scene.draws.iter().enumerate() {
        renderer.write_model(i as u32, &model_uniform(draw.model));
    }
    for (i, draw) in scene.pbr_draws.iter().enumerate() {
        renderer.write_model(base + i as u32, &model_uniform(draw.model));
    }
    let skinned_base = skinned_model_base(scene);
    for (i, draw) in scene.skinned.iter().enumerate() {
        renderer.write_model(skinned_base + i as u32, &model_uniform(draw.model));
    }
    let pbr_skinned_base = pbr_skinned_model_base(scene);
    for (i, draw) in scene.pbr_skinned.iter().enumerate() {
        renderer.write_model(pbr_skinned_base + i as u32, &model_uniform(draw.model));
    }

    let depth_load = match clear_depth {
        Some(d) => wgpu::LoadOp::Clear(d),
        None => wgpu::LoadOp::Load,
    };

    let camera_bind_group = &renderer.camera_bind_group;
    let instance_bind_group = renderer.instance_bind_group();
    let instanced_key = renderer.instanced_shadow_pipeline_key();
    let joint_bind_group = renderer.joint_bind_group();
    let skinned_key = renderer.skinned_shadow_pipeline_key();
    let joint_bases = &renderer.joint_bases;
    let opaque_skinned = scene.skinned.len();
    let cache = &renderer.cache;
    let model_stride = renderer.model_stride();
    let pipeline = cache.get(&renderer.shadow_pipeline_key())?;

    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("spawn-shadow-depth"),
        color_attachments: &[],
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

    pass.set_pipeline(pipeline);
    for (i, draw) in scene.draws.iter().enumerate() {
        let model_offset = (i as u64 * model_stride) as u32;
        pass.set_bind_group(0, camera_bind_group, &[camera_offset, model_offset]);
        pass.set_vertex_buffer(0, draw.mesh.vertex_buffer().slice(..));
        pass.set_index_buffer(
            draw.mesh.index_buffer().slice(..),
            wgpu::IndexFormat::Uint32,
        );
        pass.draw_indexed(0..draw.mesh.index_count(), 0, 0..1);
    }
    for (i, draw) in scene.pbr_draws.iter().enumerate() {
        let model_offset = (u64::from(base + i as u32) * model_stride) as u32;
        pass.set_bind_group(0, camera_bind_group, &[camera_offset, model_offset]);
        pass.set_vertex_buffer(0, draw.mesh.vertex_buffer().slice(..));
        pass.set_index_buffer(
            draw.mesh.index_buffer().slice(..),
            wgpu::IndexFormat::Uint32,
        );
        pass.draw_indexed(0..draw.mesh.index_count(), 0, 0..1);
    }

    // Instanced casters (opaque then PBR batches), matching the shared storage
    // buffer's upload order so each batch's instance range is correct: opaque in
    // `[0, total_opaque)`, PBR after. The instanced shadow pipeline reads the model
    // from group 1.
    if !scene.instances.is_empty() || !scene.pbr_instances.is_empty() {
        let instanced_pipeline = cache.get(&instanced_key)?;
        pass.set_pipeline(instanced_pipeline);
        pass.set_bind_group(1, instance_bind_group, &[]);
        let mut instance_base = 0u32;
        for batch in scene.instances {
            let count = batch.instances.len() as u32;
            pass.set_bind_group(0, camera_bind_group, &[camera_offset, 0]);
            pass.set_vertex_buffer(0, batch.mesh.vertex_buffer().slice(..));
            pass.set_index_buffer(
                batch.mesh.index_buffer().slice(..),
                wgpu::IndexFormat::Uint32,
            );
            pass.draw_indexed(
                0..batch.mesh.index_count(),
                0,
                instance_base..instance_base + count,
            );
            instance_base += count;
        }
        instance_base = opaque_instance_total(scene);
        for batch in scene.pbr_instances {
            let count = batch.instances.len() as u32;
            pass.set_bind_group(0, camera_bind_group, &[camera_offset, 0]);
            pass.set_vertex_buffer(0, batch.mesh.vertex_buffer().slice(..));
            pass.set_index_buffer(
                batch.mesh.index_buffer().slice(..),
                wgpu::IndexFormat::Uint32,
            );
            pass.draw_indexed(
                0..batch.mesh.index_count(),
                0,
                instance_base..instance_base + count,
            );
            instance_base += count;
        }
    }

    // Skinned casters (opaque-skinned then PBR-skinned), matching the joint
    // buffer's upload order. The skinned shadow pipeline reads the joint storage at
    // group 1 (dynamic offset) and the model from the per-draw slot.
    if !scene.skinned.is_empty() || !scene.pbr_skinned.is_empty() {
        let skinned_pipeline = cache.get(&skinned_key)?;
        pass.set_pipeline(skinned_pipeline);
        for (i, draw) in scene.skinned.iter().enumerate() {
            let model_offset = (u64::from(skinned_base + i as u32) * model_stride) as u32;
            pass.set_bind_group(0, camera_bind_group, &[camera_offset, model_offset]);
            pass.set_bind_group(1, joint_bind_group, &[joint_bases[i]]);
            pass.set_vertex_buffer(0, draw.mesh.vertex_buffer().slice(..));
            pass.set_index_buffer(
                draw.mesh.index_buffer().slice(..),
                wgpu::IndexFormat::Uint32,
            );
            pass.draw_indexed(0..draw.mesh.index_count(), 0, 0..1);
        }
        for (i, draw) in scene.pbr_skinned.iter().enumerate() {
            let model_offset = (u64::from(pbr_skinned_base + i as u32) * model_stride) as u32;
            pass.set_bind_group(0, camera_bind_group, &[camera_offset, model_offset]);
            pass.set_bind_group(1, joint_bind_group, &[joint_bases[opaque_skinned + i]]);
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
