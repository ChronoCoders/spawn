//! ECS components linking entities to physics bodies and colliders.
//!
//! `RigidBody`/`Collider` carry authoring descriptors consumed at registration;
//! `PhysicsBody` is the live handle link written back by the registration pass.

use spawn_ecs::Component;

use crate::handles::{ColliderHandle, RigidBodyHandle};

#[cfg(feature = "dim3")]
use crate::physics3d::{ColliderDesc, RigidBodyDesc};

#[cfg(feature = "dim2")]
use crate::physics2d::{ColliderDesc as ColliderDesc2D, RigidBodyDesc as RigidBodyDesc2D};

/// Authoring rigid-body data (3D); consumed when the entity is registered.
#[cfg(feature = "dim3")]
#[derive(Debug, Clone)]
pub struct RigidBody(pub RigidBodyDesc);

/// Authoring collider data (3D); consumed when the entity is registered.
#[cfg(feature = "dim3")]
#[derive(Debug, Clone)]
pub struct Collider(pub ColliderDesc);

/// Live link to a registered body and its optional collider (3D).
///
/// `collider` is `None` for collider-less bodies (a [`RigidBody`] registered
/// without an accompanying [`Collider`] component).
#[cfg(feature = "dim3")]
#[derive(Debug, Clone, Copy)]
pub struct PhysicsBody {
    pub body: RigidBodyHandle,
    pub collider: Option<ColliderHandle>,
}

#[cfg(feature = "dim3")]
impl Component for RigidBody {}
#[cfg(feature = "dim3")]
impl Component for Collider {}
#[cfg(feature = "dim3")]
impl Component for PhysicsBody {}

/// Authoring rigid-body data (2D); consumed when the entity is registered.
#[cfg(feature = "dim2")]
#[derive(Debug, Clone)]
pub struct RigidBody2D(pub RigidBodyDesc2D);

/// Authoring collider data (2D); consumed when the entity is registered.
#[cfg(feature = "dim2")]
#[derive(Debug, Clone)]
pub struct Collider2D(pub ColliderDesc2D);

/// Live link to a registered body and its optional collider (2D).
///
/// `collider` is `None` for collider-less bodies (a [`RigidBody2D`] registered
/// without an accompanying [`Collider2D`] component).
#[cfg(feature = "dim2")]
#[derive(Debug, Clone, Copy)]
pub struct PhysicsBody2D {
    pub body: RigidBodyHandle,
    pub collider: Option<ColliderHandle>,
}

#[cfg(feature = "dim2")]
impl Component for RigidBody2D {}
#[cfg(feature = "dim2")]
impl Component for Collider2D {}
#[cfg(feature = "dim2")]
impl Component for PhysicsBody2D {}
