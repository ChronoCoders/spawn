//! The lit forward pass: depth-tested opaque draws shaded with Lambert diffuse +
//! flat ambient, modulated by a PCF shadow lookup. Uses the renderer's built-in
//! lit pipeline for every draw (materials supply group 1); group 2 is the
//! graph-owned light bind group (light uniform + shadow map + comparison sampler).

use spawn_core::Color;

use crate::error::RenderResult;
use crate::passes::forward_opaque::{model_uniform, RenderScene};
use crate::renderer::Renderer;

/// Records the lit pass into `encoder` against `color_view`, with the renderer's
/// primary depth buffer as the depth attachment. `camera_offset` selects this
/// pass's camera slot (the scene camera); `light_bind_group` is group 2. Clears
/// per `clear_color`/`clear_depth` (`None` ⇒ load). Uses the built-in lit
/// pipeline; never builds here. No heap allocation.
// One more parameter than the opaque pass: the lit pass additionally binds the
// graph-owned light bind group (group 2). Each argument is a distinct attachment,
// offset, or bind target; bundling them would only obscure the record call.
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
    renderer.ensure_model_capacity(scene.draws.len() as u32);
    for (i, draw) in scene.draws.iter().enumerate() {
        renderer.write_model(i as u32, &model_uniform(draw.model));
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
    let cache = &renderer.cache;
    let model_stride = renderer.model_stride();
    let pipeline = cache.get(&renderer.lit_pipeline_key())?;

    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("spawn-forward-lit"),
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
    for (i, draw) in scene.draws.iter().enumerate() {
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

    Ok(())
}
