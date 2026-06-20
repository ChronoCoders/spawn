//! Fullscreen post-processing passes.
//!
//! Each post pass draws a single screen-covering triangle (generated from the
//! vertex index, no vertex buffer) sampling its input transient. This module
//! currently provides the minimal tonemap that reduces the linear HDR scene to
//! the LDR surface; the configurable exposure/ACES chain (with bloom and FXAA) is
//! layered on in the post-processing phase.

use spawn_core::Color;

use crate::error::RenderResult;
use crate::renderer::Renderer;

/// Records the tonemap pass into `encoder` against `color_view` (the LDR
/// surface). `input_bind_group` is the graph-built group-0 binding sampling the
/// HDR scene transient. Clears per `clear_color` (`None` ⇒ load); the fullscreen
/// triangle overwrites every pixel regardless. Uses the built-in tonemap
/// pipeline; never builds here. No depth, no vertex buffer, no heap allocation.
pub(crate) fn record_tonemap(
    renderer: &Renderer,
    encoder: &mut wgpu::CommandEncoder,
    color_view: &wgpu::TextureView,
    clear_color: Option<Color>,
    input_bind_group: &wgpu::BindGroup,
) -> RenderResult<()> {
    let color_load = match clear_color {
        Some(c) => wgpu::LoadOp::Clear(wgpu::Color {
            r: c.r as f64,
            g: c.g as f64,
            b: c.b as f64,
            a: c.a as f64,
        }),
        None => wgpu::LoadOp::Load,
    };
    let pipeline = renderer.cache.get(&renderer.tonemap_pipeline_key())?;

    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("spawn-tonemap"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: color_view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: color_load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });

    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, input_bind_group, &[]);
    pass.draw(0..3, 0..1);

    Ok(())
}
