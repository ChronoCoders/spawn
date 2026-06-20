//! The physically based forward pass: depth-tested opaque draws shaded with
//! Cook-Torrance specular (GGX/Smith/Fresnel) + energy-conserving Lambert
//! diffuse, the group-2 directional light + PCF shadow, written into the linear
//! HDR scene transient. Uses the renderer's built-in PBR pipeline for every draw
//! (PBR materials supply group 1); group 2 is the graph-owned light bind group.
//!
//! PBR draws occupy model-buffer slots after the `draws` list (`[D, D+P)`), so a
//! graph that mixes lit and PBR draws never collides in the shared per-draw model
//! buffer; the executor sizes the buffer for both lists up front.

use spawn_core::Color;

use crate::error::RenderResult;
use crate::passes::forward_opaque::{model_uniform, opaque_instance_total, RenderScene};
use crate::renderer::Renderer;

/// Records the PBR pass into `encoder` against `color_view` (the HDR scene
/// transient), with the renderer's primary depth buffer as the depth attachment.
/// `camera_offset` selects this pass's camera slot (the scene camera);
/// `light_bind_group` is group 2. Clears per `clear_color`/`clear_depth`
/// (`None` ⇒ load). Uses the built-in PBR pipeline; never builds here. No heap
/// allocation.
#[allow(clippy::too_many_arguments)]
pub(crate) fn record(
    renderer: &mut Renderer,
    encoder: &mut wgpu::CommandEncoder,
    color_view: &wgpu::TextureView,
    clear_color: Option<Color>,
    clear_depth: Option<f32>,
    camera_offset: u32,
    light_bind_group: &wgpu::BindGroup,
    scene: &RenderScene,
) -> RenderResult<()> {
    let base = scene.draws.len() as u32;
    renderer.ensure_model_capacity(base + scene.pbr_draws.len() as u32);
    for (i, draw) in scene.pbr_draws.iter().enumerate() {
        renderer.write_model(base + i as u32, &model_uniform(draw.model));
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
    let instanced_key = renderer.instanced_pbr_pipeline_key();
    let cache = &renderer.cache;
    let model_stride = renderer.model_stride();
    let pipeline = cache.get(&renderer.pbr_pipeline_key())?;

    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("spawn-forward-pbr"),
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

    pass.set_pipeline(pipeline);
    pass.set_bind_group(2, light_bind_group, &[]);
    for (i, draw) in scene.pbr_draws.iter().enumerate() {
        let model_offset = (u64::from(base + i as u32) * model_stride) as u32;
        pass.set_bind_group(0, camera_bind_group, &[camera_offset, model_offset]);
        pass.set_bind_group(1, draw.material.bind_group(), &[]);
        pass.set_vertex_buffer(0, draw.mesh.vertex_buffer().slice(..));
        pass.set_index_buffer(
            draw.mesh.index_buffer().slice(..),
            wgpu::IndexFormat::Uint32,
        );
        pass.draw_indexed(0..draw.mesh.index_count(), 0, 0..1);
    }

    // Instanced PBR batches: one `draw_indexed(.., base..base+N)` per batch over the
    // instance storage (group 3). PBR instances follow the opaque instances in the
    // shared buffer, so the base starts at the opaque total.
    if !scene.pbr_instances.is_empty() {
        let instanced_pipeline = cache.get(&instanced_key)?;
        pass.set_pipeline(instanced_pipeline);
        pass.set_bind_group(2, light_bind_group, &[]);
        pass.set_bind_group(3, instance_bind_group, &[]);
        let mut base = opaque_instance_total(scene);
        for batch in scene.pbr_instances {
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

    Ok(())
}
