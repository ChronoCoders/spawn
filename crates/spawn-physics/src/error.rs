//! Error and result types for the physics crate.

use std::error::Error;
use std::fmt;

use spawn_core::SpawnError;

/// Errors surfaced by the physics API. All fallible operations return this or an
/// `Option`; the crate never panics on invalid input or stale handles.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PhysicsError {
    /// A [`PhysicsConfig`](crate::PhysicsConfig) value was rejected (non-positive
    /// timestep, non-finite gravity).
    InvalidConfig { context: &'static str },
    /// A handle did not resolve to a live body/collider/joint.
    InvalidHandle,
    /// A collider shape could not be cooked (e.g. a degenerate convex hull).
    InvalidShape { context: &'static str },
    /// A joint could not be constructed from the given parameters.
    InvalidJoint { context: &'static str },
}

impl fmt::Display for PhysicsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig { context } => write!(f, "invalid physics config: {context}"),
            Self::InvalidHandle => write!(f, "invalid physics handle"),
            Self::InvalidShape { context } => write!(f, "invalid collider shape: {context}"),
            Self::InvalidJoint { context } => write!(f, "invalid joint: {context}"),
        }
    }
}

impl Error for PhysicsError {}

impl From<PhysicsError> for SpawnError {
    fn from(err: PhysicsError) -> Self {
        match err {
            PhysicsError::InvalidConfig { context } => SpawnError::InvalidArgument { context },
            PhysicsError::InvalidHandle => SpawnError::InvalidState {
                context: "physics handle",
            },
            PhysicsError::InvalidShape { context } => SpawnError::InvalidArgument { context },
            PhysicsError::InvalidJoint { context } => SpawnError::InvalidArgument { context },
        }
    }
}

/// Result alias for fallible physics operations.
pub type PhysicsResult<T> = Result<T, PhysicsError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_non_empty() {
        let variants = [
            PhysicsError::InvalidConfig { context: "a" },
            PhysicsError::InvalidHandle,
            PhysicsError::InvalidShape { context: "b" },
            PhysicsError::InvalidJoint { context: "c" },
        ];
        for v in &variants {
            assert!(!v.to_string().is_empty());
        }
    }

    #[test]
    fn maps_to_spawn_error() {
        assert!(matches!(
            SpawnError::from(PhysicsError::InvalidConfig { context: "x" }),
            SpawnError::InvalidArgument { .. }
        ));
        assert!(matches!(
            SpawnError::from(PhysicsError::InvalidHandle),
            SpawnError::InvalidState { .. }
        ));
        assert!(matches!(
            SpawnError::from(PhysicsError::InvalidShape { context: "x" }),
            SpawnError::InvalidArgument { .. }
        ));
        assert!(matches!(
            SpawnError::from(PhysicsError::InvalidJoint { context: "x" }),
            SpawnError::InvalidArgument { .. }
        ));
    }
}
