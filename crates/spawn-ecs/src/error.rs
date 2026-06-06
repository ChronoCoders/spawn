//! ECS error type and result alias.
//!
//! Names are carried as `&'static str` so construction never allocates, matching
//! spawn-core's error convention.

use crate::entity::Entity;
use std::error::Error;
use std::fmt;

/// Errors returned by fallible ECS operations.
///
/// `#[non_exhaustive]`: later phases add variants, so matches must include a
/// wildcard arm. Not `Clone`/`PartialEq` to stay forward-compatible with future
/// variants that wrap non-comparable inner errors.
#[derive(Debug)]
#[non_exhaustive]
pub enum EcsError {
    /// The entity is stale or was never alive (generation mismatch or freed slot).
    EntityNotFound {
        /// The offending identity.
        entity: Entity,
    },
    /// A component used in a query or schedule build was never registered.
    ComponentNotRegistered {
        /// Type name of the unregistered component.
        component: &'static str,
    },
    /// Two systems were asserted conflict-free in strict-build mode but conflict.
    AccessConflict {
        /// First system name.
        system_a: &'static str,
        /// Second system name.
        system_b: &'static str,
    },
    /// A system thread panicked; the panic was caught at the scope join.
    SystemPanicked {
        /// Name of the panicking system.
        system: &'static str,
    },
    /// A schedule was run before `build` sized its buffers and masks.
    ScheduleNotBuilt,
}

impl fmt::Display for EcsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EntityNotFound { entity } => {
                write!(
                    f,
                    "entity not found: index {} generation {}",
                    entity.index(),
                    entity.generation()
                )
            }
            Self::ComponentNotRegistered { component } => {
                write!(f, "component not registered: {component}")
            }
            Self::AccessConflict { system_a, system_b } => {
                write!(f, "access conflict between '{system_a}' and '{system_b}'")
            }
            Self::SystemPanicked { system } => write!(f, "system panicked: {system}"),
            Self::ScheduleNotBuilt => write!(f, "schedule not built"),
        }
    }
}

impl Error for EcsError {}

/// Result alias for fallible ECS operations.
pub type EcsResult<T> = Result<T, EcsError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_non_empty_for_every_variant() {
        let variants = [
            EcsError::EntityNotFound {
                entity: Entity::PLACEHOLDER,
            },
            EcsError::ComponentNotRegistered { component: "C" },
            EcsError::AccessConflict {
                system_a: "a",
                system_b: "b",
            },
            EcsError::SystemPanicked { system: "s" },
            EcsError::ScheduleNotBuilt,
        ];
        for v in &variants {
            assert!(!v.to_string().is_empty());
        }
    }

    #[test]
    fn is_std_error() {
        let e = EcsError::ScheduleNotBuilt;
        let _: &dyn Error = &e;
        assert!(e.source().is_none());
    }
}
