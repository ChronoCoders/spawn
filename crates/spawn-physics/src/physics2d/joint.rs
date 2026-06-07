//! Joint descriptors (2D). Phase 1 supports fixed and revolute joints only.

use spawn_core::Vec2;

/// Rigidly locks all three degrees of freedom between two bodies. The 2D frame
/// orientation is a scalar angle (radians).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FixedJoint {
    pub local_anchor_a: Vec2,
    pub local_anchor_b: Vec2,
    pub local_frame_a: f32,
    pub local_frame_b: f32,
}

impl Default for FixedJoint {
    fn default() -> Self {
        Self {
            local_anchor_a: Vec2::ZERO,
            local_anchor_b: Vec2::ZERO,
            local_frame_a: 0.0,
            local_frame_b: 0.0,
        }
    }
}

/// Hinge constraint between two bodies. The axis is implicitly Z in 2D, so no
/// axis field exists.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RevoluteJoint {
    pub local_anchor_a: Vec2,
    pub local_anchor_b: Vec2,
    /// `(min, max)` angle limits in radians, or `None` for a free hinge.
    pub limits: Option<(f32, f32)>,
}

impl Default for RevoluteJoint {
    fn default() -> Self {
        Self {
            local_anchor_a: Vec2::ZERO,
            local_anchor_b: Vec2::ZERO,
            limits: None,
        }
    }
}
