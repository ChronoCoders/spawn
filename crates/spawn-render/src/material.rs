//! Material: a pipeline key plus its group-1 bind group (uniform + texture).

use wgpu::util::DeviceExt;

use crate::asset_handle::ShaderHandle;
use crate::error::RenderResult;
use crate::pipeline::{PipelineKey, RenderStateKey, VertexLayoutId};
use crate::renderer::Renderer;
use crate::texture::Texture;

/// Per-material shading parameters. `#[repr(C)]` + `Pod`; offsets asserted for
/// std140-compatible upload.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MaterialUniform {
    pub base_color: [f32; 4],
    pub params: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<MaterialUniform>() == 32);
const _: () = assert!(std::mem::offset_of!(MaterialUniform, base_color) == 0);
const _: () = assert!(std::mem::offset_of!(MaterialUniform, params) == 16);

impl Default for MaterialUniform {
    fn default() -> Self {
        Self {
            base_color: [1.0, 1.0, 1.0, 1.0],
            params: [0.0; 4],
        }
    }
}

/// A material owns its uniform buffer and group-1 bind group, and records the
/// [`PipelineKey`] used to look up its pipeline in the cache. It owns no
/// pipeline. The bind group holds the uniform buffer and the texture
/// view/sampler alive for the material's lifetime.
pub struct Material {
    pipeline_key: PipelineKey,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
}

impl Material {
    /// Builds the uniform buffer and the group-1 bind group, recording the
    /// pipeline key from `shader` + the fixed vertex layout + `state`.
    ///
    /// When `texture` is `None`, the engine's 1x1 white fallback texture
    /// (owned by the renderer) is bound so the shared bind-group layout is
    /// always satisfied.
    pub fn new(
        renderer: &Renderer,
        shader: ShaderHandle,
        uniform: MaterialUniform,
        texture: Option<&Texture>,
        state: RenderStateKey,
    ) -> RenderResult<Self> {
        let device = renderer.device();
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("spawn-material-uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let fallback = renderer.fallback_texture();
        let view = texture.map_or_else(|| fallback.view(), Texture::view);
        let sampler = texture.map_or_else(|| fallback.sampler(), Texture::sampler);

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("spawn-material-bg"),
            layout: &renderer.bind_group_layouts().material,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });

        Ok(Self {
            pipeline_key: PipelineKey {
                shader,
                vertex_layout: VertexLayoutId::PositionNormalUv,
                render_state: state,
            },
            bind_group,
            uniform_buffer,
        })
    }

    pub fn pipeline_key(&self) -> PipelineKey {
        self.pipeline_key
    }

    pub fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }

    /// Updates the uniform contents in place via `queue.write_buffer`; no
    /// reallocation.
    pub fn set_uniform(&self, queue: &wgpu::Queue, uniform: MaterialUniform) {
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniform));
    }
}
