//! The engine error type, composing the loop crates' errors so `?` works across
//! the frame pipeline.

use std::error::Error;
use std::fmt;

/// Errors surfaced by engine setup and the frame loop.
///
/// `#[non_exhaustive]`: later phases add variants. Runtime asset *load* failures
/// are observed by systems through `AssetServer::load_state`, not surfaced here;
/// audio device absence is a `NullBackend` fallback, never an error.
#[derive(Debug)]
#[non_exhaustive]
pub enum EngineError {
    /// A configuration value was rejected (e.g. a non-positive fixed timestep).
    InvalidConfig {
        /// Why the configuration is invalid.
        reason: &'static str,
    },
    /// A platform (window / run loop) operation failed.
    Platform(spawn_platform::PlatformError),
    /// A renderer operation failed.
    Render(spawn_render::RenderError),
    /// An ECS schedule/world operation failed.
    Ecs(spawn_ecs::EcsError),
    /// The input backend failed to initialize.
    Input(spawn_input::InputError),
    /// An asset-server operation failed during setup.
    Asset(spawn_asset::AssetError),
    /// An audio-engine pump failed during the frame.
    Audio(spawn_audio::AudioError),
    /// `run`/`run_headless` was called on an engine that is already running.
    AlreadyRunning,
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig { reason } => write!(f, "invalid engine config: {reason}"),
            Self::Platform(e) => write!(f, "platform error: {e}"),
            Self::Render(e) => write!(f, "render error: {e}"),
            Self::Ecs(e) => write!(f, "ecs error: {e}"),
            Self::Input(e) => write!(f, "input error: {e}"),
            Self::Asset(e) => write!(f, "asset error: {e}"),
            Self::Audio(e) => write!(f, "audio error: {e}"),
            Self::AlreadyRunning => write!(f, "engine is already running"),
        }
    }
}

impl Error for EngineError {}

impl From<spawn_platform::PlatformError> for EngineError {
    fn from(e: spawn_platform::PlatformError) -> Self {
        Self::Platform(e)
    }
}

impl From<spawn_render::RenderError> for EngineError {
    fn from(e: spawn_render::RenderError) -> Self {
        Self::Render(e)
    }
}

impl From<spawn_ecs::EcsError> for EngineError {
    fn from(e: spawn_ecs::EcsError) -> Self {
        Self::Ecs(e)
    }
}

impl From<spawn_input::InputError> for EngineError {
    fn from(e: spawn_input::InputError) -> Self {
        Self::Input(e)
    }
}

impl From<spawn_asset::AssetError> for EngineError {
    fn from(e: spawn_asset::AssetError) -> Self {
        Self::Asset(e)
    }
}

impl From<spawn_audio::AudioError> for EngineError {
    fn from(e: spawn_audio::AudioError) -> Self {
        Self::Audio(e)
    }
}

/// Result alias for fallible engine operations.
pub type EngineResult<T> = Result<T, EngineError>;
