//! Joint descriptors (3D). Phase 1 supports fixed and revolute joints only.

use spawn_core::{Quat, Vec3};

/// Rigidly locks all six degrees of freedom between two bodies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FixedJoint {
    pub local_anchor_a: Vec3,
    pub local_anchor_b: Vec3,
    pub local_frame_a: Quat,
    pub local_frame_b: Quat,
}

impl Default for FixedJoint {
    fn default() -> Self {
        Self {
            local_anchor_a: Vec3::ZERO,
            local_anchor_b: Vec3::ZERO,
            local_frame_a: Quat::IDENTITY,
            local_frame_b: Quat::IDENTITY,
        }
    }
}

/// Single-axis hinge constraint between two bodies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RevoluteJoint {
    pub local_anchor_a: Vec3,
    pub local_anchor_b: Vec3,
    /// Hinge axis; normalized internally.
    pub axis: Vec3,
    /// `(min, max)` angle limits in radians, or `None` for a free hinge.
    pub limits: Option<(f32, f32)>,
}

impl Default for RevoluteJoint {
    fn default() -> Self {
        Self {
            local_anchor_a: Vec3::ZERO,
            local_anchor_b: Vec3::ZERO,
            axis: Vec3::Y,
            limits: None,
        }
    }
}
