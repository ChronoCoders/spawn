//! Ray and ray-hit types for 2D spatial queries.

use spawn_core::Vec2;

use crate::handles::ColliderHandle;

/// A ray for [`PhysicsWorld::ray_cast`](super::world::PhysicsWorld::ray_cast).
/// `dir` need not be unit length; it is normalized internally.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    pub origin: Vec2,
    pub dir: Vec2,
}

impl Ray {
    pub const fn new(origin: Vec2, dir: Vec2) -> Self {
        Self { origin, dir }
    }
}

/// The nearest collider hit by a ray.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    pub collider: ColliderHandle,
    /// Time-of-impact along the normalized direction (world distance).
    pub toi: f32,
    pub point: Vec2,
    /// Outward surface normal at the hit point.
    pub normal: Vec2,
}
