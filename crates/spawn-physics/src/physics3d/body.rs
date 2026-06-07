//! Rigid-body descriptors and the values read back from the world (3D).

use spawn_core::{Transform3D, Vec3};

use crate::math::BVec3;
use crate::shared::BodyType;

/// Per-axis degree-of-freedom locks. A `true` axis is removed from the solve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LockFlags {
    pub translation: BVec3,
    pub rotation: BVec3,
}

/// Linear and angular velocity of a body.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Velocity {
    pub linear: Vec3,
    pub angular: Vec3,
}

/// How a body's mass and inertia are determined.
///
/// A collider-less dynamic body under the default `Density` has zero mass and
/// therefore does not respond to gravity or forces; give it an explicit
/// `Mass` to make it move.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MassProperties {
    /// Mass and inertia computed from collider shapes scaled by this density.
    Density(f32),
    /// Explicit total mass; inertia auto-derived from attached shapes.
    Mass(f32),
}

impl Default for MassProperties {
    fn default() -> Self {
        Self::Density(1.0)
    }
}

/// Authoring description of a rigid body.
#[derive(Debug, Clone)]
pub struct RigidBodyDesc {
    pub body_type: BodyType,
    /// Translation and rotation are used; scale is ignored (physics has none).
    pub transform: Transform3D,
    pub velocity: Velocity,
    pub mass: MassProperties,
    pub linear_damping: f32,
    pub angular_damping: f32,
    pub ccd_enabled: bool,
    pub locks: LockFlags,
    pub can_sleep: bool,
}

impl Default for RigidBodyDesc {
    fn default() -> Self {
        Self {
            body_type: BodyType::Dynamic,
            transform: Transform3D::IDENTITY,
            velocity: Velocity::default(),
            mass: MassProperties::default(),
            linear_damping: 0.0,
            angular_damping: 0.0,
            ccd_enabled: false,
            locks: LockFlags::default(),
            can_sleep: true,
        }
    }
}

impl RigidBodyDesc {
    pub fn dynamic() -> Self {
        Self {
            body_type: BodyType::Dynamic,
            ..Self::default()
        }
    }

    pub fn kinematic() -> Self {
        Self {
            body_type: BodyType::Kinematic,
            ..Self::default()
        }
    }

    pub fn fixed() -> Self {
        Self {
            body_type: BodyType::Fixed,
            ..Self::default()
        }
    }

    pub fn with_transform(mut self, transform: Transform3D) -> Self {
        self.transform = transform;
        self
    }

    pub fn with_velocity(mut self, velocity: Velocity) -> Self {
        self.velocity = velocity;
        self
    }

    pub fn with_mass(mut self, mass: MassProperties) -> Self {
        self.mass = mass;
        self
    }

    pub fn with_density(mut self, density: f32) -> Self {
        self.mass = MassProperties::Density(density);
        self
    }

    pub fn with_linear_damping(mut self, damping: f32) -> Self {
        self.linear_damping = damping;
        self
    }

    pub fn with_angular_damping(mut self, damping: f32) -> Self {
        self.angular_damping = damping;
        self
    }

    pub fn with_ccd(mut self, enabled: bool) -> Self {
        self.ccd_enabled = enabled;
        self
    }

    pub fn with_locks(mut self, locks: LockFlags) -> Self {
        self.locks = locks;
        self
    }

    pub fn with_can_sleep(mut self, can_sleep: bool) -> Self {
        self.can_sleep = can_sleep;
        self
    }
}
