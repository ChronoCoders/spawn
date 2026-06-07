//! Collider shapes and descriptors (2D).

use spawn_core::{Transform2D, Vec2};

use crate::shared::CollisionGroups;

/// A collider's geometric shape.
///
/// `#[non_exhaustive]`: future phases add variants without a breaking change.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Shape {
    Ball {
        radius: f32,
    },
    Cuboid {
        half_extents: Vec2,
    },
    /// Capsule aligned with the local Y axis.
    Capsule {
        half_height: f32,
        radius: f32,
    },
    /// Cooked to a convex hull at insertion; degenerate point sets are rejected.
    ConvexHull {
        points: Vec<Vec2>,
    },
}

/// Authoring description of a collider.
#[derive(Debug, Clone)]
pub struct ColliderDesc {
    pub shape: Shape,
    /// Offset relative to the parent body (or to world space if standalone).
    pub local_transform: Transform2D,
    pub friction: f32,
    pub restitution: f32,
    pub density: f32,
    pub is_sensor: bool,
    pub groups: CollisionGroups,
}

impl ColliderDesc {
    /// Constructs a descriptor with the spec defaults (`Shape` has no default).
    pub fn new(shape: Shape) -> Self {
        Self {
            shape,
            local_transform: Transform2D::IDENTITY,
            friction: 0.5,
            restitution: 0.0,
            density: 1.0,
            is_sensor: false,
            groups: CollisionGroups::ALL,
        }
    }

    pub fn with_local_transform(mut self, transform: Transform2D) -> Self {
        self.local_transform = transform;
        self
    }

    pub fn with_friction(mut self, friction: f32) -> Self {
        self.friction = friction;
        self
    }

    pub fn with_restitution(mut self, restitution: f32) -> Self {
        self.restitution = restitution;
        self
    }

    pub fn with_density(mut self, density: f32) -> Self {
        self.density = density;
        self
    }

    pub fn as_sensor(mut self, sensor: bool) -> Self {
        self.is_sensor = sensor;
        self
    }

    pub fn with_groups(mut self, groups: CollisionGroups) -> Self {
        self.groups = groups;
        self
    }
}
