//! ECS integration: components linking entities to physics handles and the
//! registration / transform-sync passes.

mod components;

#[cfg(feature = "dim3")]
mod systems;

#[cfg(feature = "dim2")]
mod systems2d;

#[cfg(feature = "dim3")]
pub use components::{Collider, PhysicsBody, RigidBody};

#[cfg(feature = "dim2")]
pub use components::{Collider2D, PhysicsBody2D, RigidBody2D};

#[cfg(feature = "dim3")]
pub use systems::{
    register_physics_bodies, run_physics_fixed_update, sync_physics_to_transforms,
    sync_transforms_to_physics, PhysicsSyncState,
};

#[cfg(feature = "dim2")]
pub use systems2d::{
    register_physics_bodies_2d, run_physics_fixed_update_2d, sync_physics_to_transforms_2d,
    sync_transforms_to_physics_2d, PhysicsSyncState2D,
};
