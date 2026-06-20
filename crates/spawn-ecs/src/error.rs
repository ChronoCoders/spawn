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
    /// A system's `Res`/`ResMut` parameter named a resource type that was never
    /// inserted into the world.
    ResourceNotRegistered {
        /// Type name of the unregistered resource.
        resource: &'static str,
    },
    /// A system's `EventWriter`/`EventReader` named an event type for which
    /// `World::init_event` was never called.
    EventsNotInitialized {
        /// Type name of the uninitialized event.
        event: &'static str,
    },
    /// A whole-world (de)serialization codec failure (e.g. a truncated buffer).
    Serialize(spawn_serialize::SerializeError),
    /// A loaded payload named a wire id not registered in this world — the
    /// registration-order-mismatch diagnostic.
    UnknownWireId {
        /// The unregistered on-wire component index.
        wire: u16,
    },
    /// A component was asked to serialize but has no registered codec.
    ComponentNotSerializable {
        /// Type name of the component.
        component: &'static str,
    },
    /// A reparent was rejected because it would form a cycle (the parent is the
    /// child itself or one of its descendants).
    HierarchyCycle {
        /// The child whose reparent was rejected.
        entity: Entity,
    },
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
            Self::ResourceNotRegistered { resource } => {
                write!(f, "resource not registered: {resource}")
            }
            Self::EventsNotInitialized { event } => {
                write!(f, "events not initialized: {event}")
            }
            Self::Serialize(err) => write!(f, "serialize: {err}"),
            Self::UnknownWireId { wire } => write!(f, "unknown wire id: {wire}"),
            Self::ComponentNotSerializable { component } => {
                write!(f, "component not serializable: {component}")
            }
            Self::HierarchyCycle { entity } => {
                write!(
                    f,
                    "hierarchy cycle rejected for entity: index {} generation {}",
                    entity.index(),
                    entity.generation()
                )
            }
        }
    }
}

impl Error for EcsError {}

impl From<spawn_serialize::SerializeError> for EcsError {
    fn from(err: spawn_serialize::SerializeError) -> Self {
        EcsError::Serialize(err)
    }
}

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
            EcsError::ResourceNotRegistered { resource: "R" },
            EcsError::EventsNotInitialized { event: "E" },
            EcsError::Serialize(spawn_serialize::SerializeError::EndOfStream),
            EcsError::UnknownWireId { wire: 7 },
            EcsError::ComponentNotSerializable { component: "C" },
            EcsError::HierarchyCycle {
                entity: Entity::PLACEHOLDER,
            },
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
