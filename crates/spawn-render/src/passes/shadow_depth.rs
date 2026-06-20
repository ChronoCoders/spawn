//! The shadow caster pass: renders the scene depth-only from the light's
//! orthographic camera into the shadow map. No color attachment, no material or
//! light binds — only group 0 (the light view-projection in this pass's camera
//! slot, plus the per-draw model). The lit pass samples the resulting depth.

use crate::error::RenderResult;
use crate::passes::forward_opaque::{model_uniform, RenderScene};
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

    let depth_load = match clear_depth {
        Some(d) => wgpu::LoadOp::Clear(d),
        None => wgpu::LoadOp::Load,
    };

    let camera_bind_group = &renderer.camera_bind_group;
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

    Ok(())
}
