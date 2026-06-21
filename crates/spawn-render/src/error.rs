//! Rendering error layer.
//!
//! Every fallible rendering operation returns [`RenderResult`]. No rendering
//! code path uses `unwrap`/`expect`/`panic` outside tests; failures surface as
//! [`RenderError`] variants. Surface-loss recovery semantics live on the frame
//! lifecycle (see [`crate::frame`]); the variants here describe the outcomes the
//! caller observes.

use crate::asset_handle::ShaderHandle;
use crate::pipeline::PipelineKey;
use spawn_asset::AssetError;

/// Source position reported by the shader compiler for a WGSL diagnostic, when
/// the backend provides one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceLocation {
    pub line: u32,
    pub column: u32,
}

/// Rendering failure.
///
/// `#[non_exhaustive]` because later phases add variants (lighting, etc.).
/// Recovery contracts: [`RenderError::Surface`] is returned only after a
/// reconfigure-and-retry already failed; [`RenderError::SurfaceTimeout`] is
/// non-fatal and the caller may skip the frame; [`RenderError::DeviceLost`] and
/// [`RenderError::OutOfMemory`] are fatal and the caller decides.
/// [`RenderError::PipelineNotCached`] signals a programmer error: a draw
/// referenced a pipeline that was never built at load time (pipelines are never
/// built mid-frame).
#[derive(Debug)]
#[non_exhaustive]
pub enum RenderError {
    NoAdapter,
    DeviceRequest {
        message: String,
    },
    DeviceLost,
    Surface,
    SurfaceTimeout,
    OutOfMemory,
    ShaderCompile {
        handle: ShaderHandle,
        message: String,
        location: Option<SourceLocation>,
    },
    PipelineNotCached(PipelineKey),
    InvalidArgument {
        context: &'static str,
    },
    Asset(AssetError),
    /// The derived render-graph dependency graph contains a cycle.
    GraphCycle,
    /// A pass reads a resource produced by no earlier pass.
    GraphResourceNotProduced {
        resource: &'static str,
    },
    /// A transient resource is written but never read, or read but never written.
    GraphDanglingResource {
        resource: &'static str,
    },
    /// A directional light's shadow frustum is degenerate (non-positive extent,
    /// zero resolution, `far <= near`, or a zero light direction).
    ShadowConfigInvalid {
        context: &'static str,
    },
    /// A skeleton is empty, has a parent index out of range, or is not
    /// topologically ordered (a joint's parent must precede it).
    SkeletonInvalid {
        context: &'static str,
    },
    /// An animation clip has an empty or non-finite track, mismatched keyframe
    /// array lengths, or a joint count that disagrees with its skeleton.
    AnimationInvalid {
        context: &'static str,
    },
    /// An instance or joint batch exceeds a fixed capacity (e.g. a skinned draw's
    /// skeleton is larger than the joint dynamic-offset window).
    InstanceBufferOverflow {
        context: &'static str,
    },
    /// A post-processing chain is misconfigured (e.g. bloom enabled with zero
    /// iterations, or a non-finite exposure / threshold).
    PostConfigInvalid {
        context: &'static str,
    },
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoAdapter => write!(f, "no compatible GPU adapter found"),
            Self::DeviceRequest { message } => write!(f, "device request failed: {message}"),
            Self::DeviceLost => write!(f, "GPU device lost"),
            Self::Surface => write!(f, "surface configure/acquire failed after recovery"),
            Self::SurfaceTimeout => write!(f, "surface acquire timed out"),
            Self::OutOfMemory => write!(f, "GPU out of memory"),
            Self::ShaderCompile {
                handle,
                message,
                location,
            } => match location {
                Some(loc) => write!(
                    f,
                    "shader {handle:?} compile error at {}:{}: {message}",
                    loc.line, loc.column
                ),
                None => write!(f, "shader {handle:?} compile error: {message}"),
            },
            Self::PipelineNotCached(key) => {
                write!(
                    f,
                    "pipeline not cached for key {key:?} (build at load time)"
                )
            }
            Self::InvalidArgument { context } => write!(f, "invalid argument: {context}"),
            Self::Asset(err) => write!(f, "asset error: {err}"),
            Self::GraphCycle => write!(f, "render graph has a dependency cycle"),
            Self::GraphResourceNotProduced { resource } => {
                write!(
                    f,
                    "render graph resource '{resource}' is read but not produced"
                )
            }
            Self::GraphDanglingResource { resource } => {
                write!(f, "render graph transient '{resource}' is written-never-read or read-never-written")
            }
            Self::ShadowConfigInvalid { context } => {
                write!(f, "invalid shadow configuration: {context}")
            }
            Self::SkeletonInvalid { context } => write!(f, "invalid skeleton: {context}"),
            Self::AnimationInvalid { context } => write!(f, "invalid animation clip: {context}"),
            Self::InstanceBufferOverflow { context } => {
                write!(f, "instance/joint buffer overflow: {context}")
            }
            Self::PostConfigInvalid { context } => {
                write!(f, "invalid post-processing config: {context}")
            }
        }
    }
}

impl std::error::Error for RenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Asset(err) => Some(err),
            _ => None,
        }
    }
}

impl From<AssetError> for RenderError {
    fn from(err: AssetError) -> Self {
        Self::Asset(err)
    }
}

pub type RenderResult<T> = Result<T, RenderError>;
