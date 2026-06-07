//! Editor error type and result alias.
//!
//! Names are carried as `&'static str` so construction never allocates, matching
//! the spawn-core/spawn-ecs convention.

use spawn_ecs::{EcsError, Entity};
use std::error::Error;
use std::fmt;

/// Errors returned by fallible editor operations.
///
/// `#[non_exhaustive]`: later phases add variants, so matches must include a
/// wildcard arm. Not `Clone`/`PartialEq` to stay forward-compatible and because
/// the [`Ecs`](EditorError::Ecs) variant wraps a non-comparable inner error.
#[derive(Debug)]
#[non_exhaustive]
pub enum EditorError {
    /// An entity referenced by a command or operation is stale or never existed.
    EntityNotFound {
        /// The offending identity.
        entity: Entity,
    },
    /// A command required a component the entity does not have.
    ComponentMissing {
        /// The entity that lacked the component.
        entity: Entity,
        /// Type name of the missing component.
        component: &'static str,
    },
    /// `undo` was requested with an empty undo history.
    NothingToUndo,
    /// `redo` was requested with an empty redo history.
    NothingToRedo,
    /// A mode-gated operation was invoked in the wrong [`EditorMode`](crate::EditorMode).
    InvalidMode {
        /// Description of the disallowed transition.
        context: &'static str,
    },
    /// An underlying ECS operation failed.
    Ecs(EcsError),
}

impl fmt::Display for EditorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EntityNotFound { entity } => write!(
                f,
                "entity not found: index {} generation {}",
                entity.index(),
                entity.generation()
            ),
            Self::ComponentMissing { entity, component } => write!(
                f,
                "component '{}' missing on entity index {} generation {}",
                component,
                entity.index(),
                entity.generation()
            ),
            Self::NothingToUndo => write!(f, "nothing to undo"),
            Self::NothingToRedo => write!(f, "nothing to redo"),
            Self::InvalidMode { context } => write!(f, "invalid editor mode: {context}"),
            Self::Ecs(inner) => write!(f, "ecs error: {inner}"),
        }
    }
}

impl Error for EditorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Ecs(inner) => Some(inner),
            _ => None,
        }
    }
}

impl From<EcsError> for EditorError {
    fn from(value: EcsError) -> Self {
        Self::Ecs(value)
    }
}

/// Result alias for fallible editor operations.
pub type EditorResult<T> = Result<T, EditorError>;
