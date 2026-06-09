//! The forward opaque pass: depth-tested (`Less`, write on), unblended draws.
//! The camera is bound via a per-pass dynamic offset (the graph writes each
//! pass's view-projection into its own slot), so multi-pass graphs do not clobber
//! a shared camera buffer.

use spawn_core::{Color, Mat4};

use crate::camera::Camera;
use crate::error::RenderResult;
use crate::material::Material;
use crate::mesh::Mesh;
use crate::pipeline::ModelUniform;
use crate::renderer::Renderer;

/// The opaque scene to render: one active camera and the caller-ordered draws.
pub struct RenderScene<'a> {
    pub camera: &'a Camera,
    pub draws: &'a [DrawItem<'a>],
}

/// A single opaque draw: mesh + material + model-to-world transform.
pub struct DrawItem<'a> {
    pub mesh: &'a Mesh,
    pub material: &'a Material,
    pub model: Mat4,
}

fn model_uniform(model: Mat4) -> ModelUniform {
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

    Ok(())
}
