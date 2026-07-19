//! Error and result types for the bit-level codec.

use std::error::Error;
use std::fmt;

use spawn_core::SpawnError;

/// A serialization failure. `Copy`, `&'static str` contexts, construction is
/// allocation-free; never used to panic.
///
/// `#[non_exhaustive]`: later phases may add variants (e.g. schema/versioning) via
/// approved specs, so external matches must carry a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SerializeError {
    /// A write would exceed the writer's buffer, or a read would pass the end of input.
    EndOfStream,
    /// A requested bit width was `0` or greater than `64`.
    InvalidWidth {
        /// The offending width.
        width: u32,
    },
    /// A value did not fit the declared bit width / bounded range.
    OutOfRange {
        /// Failure-class context.
        context: &'static str,
    },
}

/// Result alias for fallible codec operations.
pub type SerializeResult<T> = Result<T, SerializeError>;

impl fmt::Display for SerializeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EndOfStream => write!(f, "end of stream"),
            Self::InvalidWidth { width } => write!(f, "invalid bit width: {width}"),
            Self::OutOfRange { context } => write!(f, "{context}"),
        }
    }
}

impl Error for SerializeError {}

impl From<SerializeError> for SpawnError {
    fn from(err: SerializeError) -> Self {
        match err {
            SerializeError::EndOfStream => SpawnError::Parse {
                context: "serialize: end of stream",
            },
            SerializeError::InvalidWidth { .. } => SpawnError::InvalidArgument {
                context: "serialize: invalid bit width",
            },
            SerializeError::OutOfRange { context } => SpawnError::InvalidArgument { context },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_non_empty_for_every_variant() {
        let variants = [
            SerializeError::EndOfStream,
            SerializeError::InvalidWidth { width: 99 },
            SerializeError::OutOfRange { context: "x" },
        ];
        for v in &variants {
            assert!(!v.to_string().is_empty());
        }
    }

    #[test]
    fn maps_to_spawn_error() {
        let eos: SpawnError = SerializeError::EndOfStream.into();
        assert!(matches!(eos, SpawnError::Parse { .. }));
        let w: SpawnError = SerializeError::InvalidWidth { width: 0 }.into();
        assert!(matches!(w, SpawnError::InvalidArgument { .. }));
        let r: SpawnError = SerializeError::OutOfRange { context: "c" }.into();
        assert!(matches!(r, SpawnError::InvalidArgument { context: "c" }));
    }
}
