//! Audio error layer. `context` is `&'static str` so construction never
//! allocates (matching `spawn_core::SpawnError`).

use std::fmt;

#[derive(Debug)]
#[non_exhaustive]
pub enum AudioError {
    /// Surfaced by lower-level backend probing and in `Display`/logging only.
    /// [`crate::AudioEngine::new`] never returns this, it falls back to a
    /// [`crate::BackendKind::Null`] backend instead.
    DeviceUnavailable {
        context: &'static str,
    },
    UnsupportedFormat {
        context: &'static str,
    },
    Decode {
        context: &'static str,
    },
    AssetNotLoaded,
    UnknownBus,
    DuplicateBus,
    ReservedBus,
    VoiceLimit,
    InvalidHandle,
    Backend {
        context: &'static str,
    },
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeviceUnavailable { context } => write!(f, "audio device unavailable: {context}"),
            Self::UnsupportedFormat { context } => {
                write!(f, "unsupported audio format: {context}")
            }
            Self::Decode { context } => write!(f, "audio decode error: {context}"),
            Self::AssetNotLoaded => write!(f, "audio source asset is not loaded"),
            Self::UnknownBus => write!(f, "unknown audio bus"),
            Self::DuplicateBus => write!(f, "duplicate audio bus id"),
            Self::ReservedBus => write!(f, "reserved audio bus id (master)"),
            Self::VoiceLimit => write!(f, "audio voice limit reached"),
            Self::InvalidHandle => write!(f, "invalid or stale sound handle"),
            Self::Backend { context } => write!(f, "audio backend error: {context}"),
        }
    }
}

impl std::error::Error for AudioError {}

impl From<AudioError> for spawn_core::SpawnError {
    fn from(err: AudioError) -> Self {
        match err {
            AudioError::DeviceUnavailable { context } | AudioError::Backend { context } => {
                Self::InvalidState { context }
            }
            AudioError::UnsupportedFormat { context } => Self::Unsupported { context },
            AudioError::Decode { context } => Self::Parse { context },
            AudioError::AssetNotLoaded => Self::NotFound {
                context: "audio source not loaded",
            },
            AudioError::UnknownBus => Self::NotFound {
                context: "unknown audio bus",
            },
            AudioError::DuplicateBus => Self::InvalidArgument {
                context: "duplicate audio bus id",
            },
            AudioError::ReservedBus => Self::InvalidArgument {
                context: "reserved audio bus id",
            },
            AudioError::VoiceLimit => Self::InvalidState {
                context: "audio voice limit reached",
            },
            AudioError::InvalidHandle => Self::InvalidArgument {
                context: "invalid sound handle",
            },
        }
    }
}

pub type AudioResult<T> = Result<T, AudioError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_non_empty_for_every_variant() {
        let variants = [
            AudioError::DeviceUnavailable { context: "a" },
            AudioError::UnsupportedFormat { context: "b" },
            AudioError::Decode { context: "c" },
            AudioError::AssetNotLoaded,
            AudioError::UnknownBus,
            AudioError::DuplicateBus,
            AudioError::ReservedBus,
            AudioError::VoiceLimit,
            AudioError::InvalidHandle,
            AudioError::Backend { context: "d" },
        ];
        for v in &variants {
            assert!(!v.to_string().is_empty());
        }
    }

    #[test]
    fn maps_into_spawn_error() {
        let e: spawn_core::SpawnError = AudioError::UnsupportedFormat { context: "x" }.into();
        assert!(matches!(e, spawn_core::SpawnError::Unsupported { .. }));
        let e: spawn_core::SpawnError = AudioError::AssetNotLoaded.into();
        assert!(matches!(e, spawn_core::SpawnError::NotFound { .. }));
        let e: spawn_core::SpawnError = AudioError::Decode { context: "x" }.into();
        assert!(matches!(e, spawn_core::SpawnError::Parse { .. }));
    }
}
