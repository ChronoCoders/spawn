#![deny(warnings)]

//! wgpu-backed renderer for the Spawn engine: surface lifecycle, an explicit
//! ordered render-graph-lite, a forward opaque pass, meshes/materials/textures,
//! a camera, and a key-addressed pipeline cache.
//!
//! All GPU access goes through `wgpu`; there is no `unsafe` in this crate.
//! Pipelines are built once and cached; shaders compile at load. The per-frame
//! path (`begin_frame`..`end_frame`) performs no engine heap allocation in
//! steady state. Conventions are inherited from `spawn-core`: right-handed,
//! column-major matrices, depth range `[0, 1]`.

mod asset_handle;
mod camera;
mod error;
mod format;
mod frame;
mod graph;
mod material;
mod mesh;
mod passes;
mod pipeline;
mod renderer;
mod texture;

pub use asset_handle::ShaderHandle;
pub use camera::{Camera, CameraUniform};
pub use error::{RenderError, RenderResult, SourceLocation};
pub use format::{
    AddressMode, CompareFn, CullMode, DepthFormat, FilterMode, PowerPreference, PresentMode,
    SurfaceSize, TextureFormat, Topology,
};
pub use frame::FrameContext;
pub use graph::{
    ColorWrite, CompiledGraph, DepthWrite, PassDesc, PassKind, RenderGraph, ResourceDesc,
    ResourceId, ResourceKind, SizeSpec,
};
pub use material::{Material, MaterialUniform};
pub use mesh::{Mesh, Vertex};
pub use passes::forward_opaque::{DrawItem, RenderScene};
pub use pipeline::{
    BindGroupLayouts, PipelineCache, PipelineKey, RenderStateKey, ShaderStore, VertexLayoutId,
};
pub use renderer::{HasWindowHandleSet, Renderer, RendererConfig};
pub use texture::{SamplerConfig, Texture};
