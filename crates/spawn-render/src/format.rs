//! Surface geometry, depth-format selection, and the curated wgpu enum
//! re-exports that form the engine's public rendering vocabulary.
//!
//! Downstream crates use these re-exports instead of depending on `wgpu`
//! directly, so the GPU backend stays an implementation detail of this crate.

pub use wgpu::{
    AddressMode, CompareFunction as CompareFn, FilterMode, PowerPreference, PresentMode,
    PrimitiveTopology as Topology, TextureFormat,
};

/// Face-culling selector. Maps to wgpu's `Option<Face>`; modeled as an engine
/// enum so it is `Copy + Eq + Hash` for the pipeline cache key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CullMode {
    None,
    Front,
    Back,
}

/// Physical surface dimensions in pixels. Zero in either axis means the window
/// is minimized; see [`crate::renderer::Renderer::resize`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SurfaceSize {
    pub width: u32,
    pub height: u32,
}

impl SurfaceSize {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// True when either dimension is zero (minimized); presentation is
    /// suppressed in this state.
    pub const fn is_zero(self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// Depth attachment format. Part of [`crate::pipeline::RenderStateKey`] so a
/// pipeline's depth-stencil state is captured by its cache key.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DepthFormat {
    #[default]
    Depth32Float,
    Depth24PlusStencil8,
}

impl DepthFormat {
    pub const fn to_wgpu(self) -> TextureFormat {
        match self {
            Self::Depth32Float => TextureFormat::Depth32Float,
            Self::Depth24PlusStencil8 => TextureFormat::Depth24PlusStencil8,
        }
    }
}
