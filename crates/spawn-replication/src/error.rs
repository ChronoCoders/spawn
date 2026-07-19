//! Error and result types for the replication layer.

use std::error::Error;
use std::fmt;

use spawn_core::SpawnError;
use spawn_serialize::SerializeError;

/// A replication failure. `Copy`, `&'static str` contexts, construction is
/// allocation-free; never used to panic.
///
/// `#[non_exhaustive]`: later modules (prediction, RPC, drivers) add variants via the
/// approved spec, so external matches must carry a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReplError {
    /// A bit-codec failure from `spawn-serialize` (buffer exhausted, bad width, …).
    Serialize(SerializeError),
    /// A snapshot referenced a component wire id that is not registered.
    UnknownComponent {
        /// The unregistered on-wire component index.
        wire_id: u16,
    },
    /// A component expected on an entity was absent (encode), or applying a decoded
    /// component to an entity failed.
    Component {
        /// Failure-class context.
        context: &'static str,
    },
    /// The peer referenced a replicated entity this side does not have (a desync the
    /// driver's baseline agreement is meant to prevent).
    Desync {
        /// Failure-class context.
        context: &'static str,
    },
    /// A malformed or unauthorized RPC (bad kind on the wire, unregistered id, or a
    /// server RPC on an entity the sender does not own).
    Rpc {
        /// Failure-class context.
        context: &'static str,
    },
    /// A `spawn-net` transport error surfaced while sending/polling (the underlying
    /// `NetError` is not retained, it is not `Copy`, only the failure class).
    Transport {
        /// Failure-class context.
        context: &'static str,
    },
}

/// Result alias for fallible replication operations.
pub type ReplResult<T> = Result<T, ReplError>;

impl From<SerializeError> for ReplError {
    fn from(err: SerializeError) -> Self {
        ReplError::Serialize(err)
    }
}

impl fmt::Display for ReplError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize(err) => write!(f, "serialize: {err}"),
            Self::UnknownComponent { wire_id } => {
                write!(f, "unknown replicated component wire id {wire_id}")
            }
            Self::Component { context } => write!(f, "{context}"),
            Self::Desync { context } => write!(f, "{context}"),
            Self::Rpc { context } => write!(f, "{context}"),
            Self::Transport { context } => write!(f, "{context}"),
        }
    }
}

impl Error for ReplError {}

impl From<ReplError> for SpawnError {
    fn from(err: ReplError) -> Self {
        match err {
            ReplError::Serialize(e) => e.into(),
            ReplError::UnknownComponent { .. } => SpawnError::Parse {
                context: "replication: unknown component wire id",
            },
            ReplError::Component { context } => SpawnError::InvalidState { context },
            ReplError::Desync { context } => SpawnError::InvalidState { context },
            ReplError::Rpc { context } => SpawnError::InvalidState { context },
            ReplError::Transport { context } => SpawnError::InvalidState { context },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_and_conversions() {
        let e = ReplError::from(SerializeError::EndOfStream);
        assert!(matches!(e, ReplError::Serialize(_)));
        assert!(!e.to_string().is_empty());
        let s: SpawnError = ReplError::UnknownComponent { wire_id: 3 }.into();
        assert!(matches!(s, SpawnError::Parse { .. }));
        let s2: SpawnError = ReplError::Component { context: "x" }.into();
        assert!(matches!(s2, SpawnError::InvalidState { context: "x" }));
    }
}
