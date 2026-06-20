//! The transparent pass: alpha-blended forward draws recorded after the
//! opaque/PBR pass. Lit + PCF-shadowed (the built-in lit shader) but blended
//! `SrcAlpha`/`OneMinusSrcAlpha` with depth-write off, so transparent fragments
//! occlude against opaque geometry yet not against each other. Draws are sorted
//! back-to-front by squared camera distance each frame (deterministic: farther
//! first, ties by draw index). Reads the HDR scene color + depth, writes the HDR
//! color. Transparent draws occupy model-buffer slots after the lit + PBR lists.

use spawn_core::{Color, Mat4, Vec3};

use crate::error::RenderResult;
use crate::passes::forward_opaque::{model_uniform, RenderScene};
use crate::renderer::Renderer;

fn translation(model: Mat4) -> Vec3 {
    Vec3::new(model.cols[3].x, model.cols[3].y, model.cols[3].z)
}

/// Fills `scratch` with `(squared_distance, index)` for indices `0..count` and
/// sorts it back-to-front: descending squared distance from `eye`, ties broken by
/// ascending index (so the order is deterministic without a stable sort). Pure and
/// allocation-free once `scratch` has capacity — the renderer reuses one buffer.
pub(crate) fn order_back_to_front(
    eye: Vec3,
    count: u32,
    translation_of: impl Fn(u32) -> Vec3,
    scratch: &mut Vec<(f32, u32)>,
) {
    scratch.clear();
    for i in 0..count {
        let t = translation_of(i);
        let dx = t.x - eye.x;
        let dy = t.y - eye.y;
        let dz = t.z - eye.z;
        scratch.push((dx * dx + dy * dy + dz * dz, i));
    }
    scratch.sort_unstable_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });
}

/// Records the transparent pass into `encoder` against `color_view` (the HDR scene
/// transient), with the renderer's primary depth buffer (depth-test, no write) as
/// the depth attachment. `camera_offset` selects this pass's camera slot;
/// `light_bind_group` is group 2. Clears per `clear_color`/`clear_depth`
/// (`None` ⇒ load). Uses the built-in transparent pipeline; never builds here. No
/// heap allocation (the sort reuses the renderer's scratch buffer).
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
    let base = (scene.draws.len() + scene.pbr_draws.len()) as u32;
    renderer.ensure_model_capacity(base + scene.transparent.len() as u32);
    for (i, draw) in scene.transparent.iter().enumerate() {
        renderer.write_model(base + i as u32, &model_uniform(draw.model));
    }

    let eye = {
        let p = scene.camera.uniform().view_pos;
        Vec3::new(p[0], p[1], p[2])
    };
    let mut scratch = std::mem::take(&mut renderer.transparent_scratch);
    order_back_to_front(
        eye,
        scene.transparent.len() as u32,
        |i| translation(scene.transparent[i as usize].model),
        &mut scratch,
    );

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
    let pipeline = cache.get(&renderer.transparent_pipeline_key())?;

    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("spawn-transparent"),
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
    for &(_, idx) in &scratch {
        let draw = &scene.transparent[idx as usize];
        let model_offset = (u64::from(base + idx) * model_stride) as u32;
        pass.set_bind_group(0, camera_bind_group, &[camera_offset, model_offset]);
        pass.set_bind_group(1, draw.material.bind_group(), &[]);
        pass.set_vertex_buffer(0, draw.mesh.vertex_buffer().slice(..));
        pass.set_index_buffer(
            draw.mesh.index_buffer().slice(..),
            wgpu::IndexFormat::Uint32,
        );
        pass.draw_indexed(0..draw.mesh.index_count(), 0, 0..1);
    }

    drop(pass);
    renderer.transparent_scratch = scratch;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_far_to_near_with_index_tiebreak() {
        let eye = Vec3::new(0.0, 0.0, 0.0);
        // Two near (dist 1) at indices 0 and 2, one far (dist 100) at index 1.
        let positions = [
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        ];
        let mut scratch = Vec::new();
        order_back_to_front(
            eye,
            positions.len() as u32,
            |i| positions[i as usize],
            &mut scratch,
        );
        let order: Vec<u32> = scratch.iter().map(|&(_, i)| i).collect();
        // Farthest (index 1) first, then the two equidistant ones in index order.
        assert_eq!(order, vec![1, 0, 2]);
    }

    #[test]
    fn empty_input_yields_empty_order() {
        let mut scratch = vec![(0.0, 7)];
        order_back_to_front(Vec3::ZERO, 0, |_| Vec3::ZERO, &mut scratch);
        assert!(scratch.is_empty());
    }

    #[test]
    fn reused_scratch_does_not_grow_in_steady_state() {
        let eye = Vec3::ZERO;
        let positions: Vec<Vec3> = (0..16).map(|i| Vec3::new(i as f32, 0.0, 0.0)).collect();
        let mut scratch = Vec::new();
        order_back_to_front(
            eye,
            positions.len() as u32,
            |i| positions[i as usize],
            &mut scratch,
        );
        let cap = scratch.capacity();
        order_back_to_front(
            eye,
            positions.len() as u32,
            |i| positions[i as usize],
            &mut scratch,
        );
        assert_eq!(scratch.capacity(), cap, "second pass must not reallocate");
    }
}
