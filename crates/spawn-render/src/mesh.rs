//! The fixed Phase 1 vertex format and GPU-resident [`Mesh`].

use wgpu::util::DeviceExt;

use crate::error::{RenderError, RenderResult};

/// Interleaved position/normal/uv vertex. `#[repr(C)]` + `Pod` so it casts to
/// bytes via `bytemuck` for upload; layout is asserted at compile time below.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

const _: () = assert!(std::mem::size_of::<Vertex>() == 32);
const _: () = assert!(std::mem::align_of::<Vertex>() == 4);
const _: () = assert!(std::mem::offset_of!(Vertex, position) == 0);
const _: () = assert!(std::mem::offset_of!(Vertex, normal) == 12);
const _: () = assert!(std::mem::offset_of!(Vertex, uv) == 24);

impl Vertex {
    /// Attribute table for the fixed layout: location 0 position, 1 normal,
    /// 2 uv. A `const` so [`Vertex::layout`] allocates nothing at call time.
    pub const ATTRIBUTES: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![
        0 => Float32x3,
        1 => Float32x3,
        2 => Float32x2,
    ];

    pub const fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// GPU-resident mesh: owns its vertex and index buffers. Dropping the `Mesh`
/// drops the wrapped `wgpu::Buffer`s; wgpu defers GPU-side destruction until the
/// GPU is idle on them.
pub struct Mesh {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

impl Mesh {
    /// Uploads `vertices`/`indices` to GPU buffers.
    ///
    /// `Err(InvalidArgument)` if either slice is empty or any index is out of
    /// range for the vertex count.
    pub fn new(device: &wgpu::Device, vertices: &[Vertex], indices: &[u32]) -> RenderResult<Self> {
        if vertices.is_empty() {
            return Err(RenderError::InvalidArgument {
                context: "mesh has no vertices",
            });
        }
        if indices.is_empty() {
            return Err(RenderError::InvalidArgument {
                context: "mesh has no indices",
            });
        }
        let vertex_count = vertices.len() as u32;
        if indices.iter().any(|&i| i >= vertex_count) {
            return Err(RenderError::InvalidArgument {
                context: "mesh index out of range",
            });
        }

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("spawn-mesh-vertex"),
            contents: bytemuck::cast_slice(vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("spawn-mesh-index"),
            contents: bytemuck::cast_slice(indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        Ok(Self {
            vertex_buffer,
            index_buffer,
            index_count: indices.len() as u32,
        })
    }

    pub fn index_count(&self) -> u32 {
        self.index_count
    }

    pub fn vertex_buffer(&self) -> &wgpu::Buffer {
        &self.vertex_buffer
    }

    pub fn index_buffer(&self) -> &wgpu::Buffer {
        &self.index_buffer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_describes_three_attributes() {
        let layout = Vertex::layout();
        assert_eq!(layout.array_stride, 32);
        assert_eq!(layout.attributes.len(), 3);
        assert_eq!(layout.attributes[0].shader_location, 0);
        assert_eq!(layout.attributes[0].offset, 0);
        assert_eq!(layout.attributes[1].offset, 12);
        assert_eq!(layout.attributes[2].offset, 24);
        assert_eq!(layout.step_mode, wgpu::VertexStepMode::Vertex);
    }
}
