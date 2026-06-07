//! Dimension-agnostic types shared by the 2D and 3D physics modules.

use crate::handles::{ColliderHandle, RigidBodyHandle};

/// How a rigid body participates in simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyType {
    /// Full simulation: affected by forces, gravity, and collisions.
    Dynamic,
    /// Velocity-driven with infinite mass: pushes dynamic bodies but is not
    /// affected by forces or collisions.
    Kinematic,
    /// Immovable static geometry.
    Fixed,
}

/// Collision-group bitmask pair. Two colliders interact iff
/// `(a.memberships & b.filter) != 0 && (b.memberships & a.filter) != 0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollisionGroups {
    pub memberships: u32,
    pub filter: u32,
}

impl CollisionGroups {
    /// Member of every group and interacting with every group.
    pub const ALL: Self = Self {
        memberships: 0xFFFF_FFFF,
        filter: 0xFFFF_FFFF,
    };

    pub const fn new(memberships: u32, filter: u32) -> Self {
        Self {
            memberships,
            filter,
        }
    }
}

impl Default for CollisionGroups {
    fn default() -> Self {
        Self::ALL
    }
}

/// Filtering applied to ray-cast and shape-overlap queries.
#[derive(Debug, Clone, Copy)]
pub struct QueryFilter {
    pub groups: CollisionGroups,
    pub exclude_body: Option<RigidBodyHandle>,
    pub exclude_collider: Option<ColliderHandle>,
    pub include_sensors: bool,
}

impl Default for QueryFilter {
    fn default() -> Self {
        Self {
            groups: CollisionGroups::ALL,
            exclude_body: None,
            exclude_collider: None,
            include_sensors: false,
        }
    }
}

/// A contact- or intersection-pair lifecycle event drained once per step.
///
/// Within a pair the lower handle (by `(index, generation)`) is always first, so
/// equality comparisons are order-stable across runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollisionEvent {
    Started(ColliderHandle, ColliderHandle),
    Stopped(ColliderHandle, ColliderHandle),
}

/// Orders a collider pair so the lower `(index, generation)` is first.
pub(crate) fn order_pair(a: ColliderHandle, b: ColliderHandle) -> (ColliderHandle, ColliderHandle) {
    if (a.index, a.generation) <= (b.index, b.generation) {
        (a, b)
    } else {
        (b, a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collision_groups_defaults_to_all() {
        assert_eq!(CollisionGroups::default(), CollisionGroups::ALL);
        assert_eq!(CollisionGroups::new(1, 2).memberships, 1);
        assert_eq!(CollisionGroups::new(1, 2).filter, 2);
    }

    #[test]
    fn query_filter_default() {
        let f = QueryFilter::default();
        assert_eq!(f.groups, CollisionGroups::ALL);
        assert!(f.exclude_body.is_none());
        assert!(f.exclude_collider.is_none());
        assert!(!f.include_sensors);
    }

    #[test]
    fn order_pair_stable() {
        let lo = ColliderHandle {
            index: 1,
            generation: 0,
        };
        let hi = ColliderHandle {
            index: 5,
            generation: 0,
        };
        assert_eq!(order_pair(hi, lo), (lo, hi));
        assert_eq!(order_pair(lo, hi), (lo, hi));
    }
}
