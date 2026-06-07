#![deny(warnings)]

//! Deterministic fixed-step rigid-body physics for the Spawn engine.
//!
//! Wraps Rapier behind a spawn-core-typed API. Two mirrored modules,
//! [`physics3d`] and [`physics2d`], expose the same surface over 3D and 2D math
//! respectively; dimension-agnostic types (handles, body type, collision groups,
//! query filter, collision events, errors) live at the crate root and are reused
//! by both. No Rapier or nalgebra type appears in any public signature.
//!
//! Determinism: Rapier's `enhanced-determinism` is enabled on both backends so
//! the fixed-step substrate is bit-for-bit reproducible across platforms given
//! identical insertion order and per-tick inputs — the foundation spawn-net's
//! rollback netcode replays.

#[cfg(not(any(feature = "dim2", feature = "dim3")))]
compile_error!(
    "spawn-physics requires at least one of the `dim2` or `dim3` features to be enabled"
);

pub mod ecs;
pub mod error;
pub mod handles;
pub mod math;
pub mod shared;

#[cfg(feature = "dim3")]
pub mod physics3d;

#[cfg(feature = "dim2")]
pub mod physics2d;

pub use error::{PhysicsError, PhysicsResult};
pub use handles::{ColliderHandle, JointHandle, RigidBodyHandle};
pub use math::{BVec2, BVec3};
pub use shared::{BodyType, CollisionEvent, CollisionGroups, QueryFilter};
