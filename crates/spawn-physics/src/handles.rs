//! Opaque, generational handles shared by the 2D and 3D modules.
//!
//! Each handle wraps Rapier's generational index. The index/generation pair
//! guarantees a handle to a removed object never aliases a later object: stale
//! handles resolve to `None`/`Err(InvalidHandle)`.

/// Handle to a rigid body in a [`PhysicsWorld`](crate::PhysicsWorld).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RigidBodyHandle {
    pub(crate) index: u32,
    pub(crate) generation: u32,
}

/// Handle to a collider in a [`PhysicsWorld`](crate::PhysicsWorld).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ColliderHandle {
    pub(crate) index: u32,
    pub(crate) generation: u32,
}

/// Handle to a joint in a [`PhysicsWorld`](crate::PhysicsWorld).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct JointHandle {
    pub(crate) index: u32,
    pub(crate) generation: u32,
}

const _: () = assert!(std::mem::size_of::<RigidBodyHandle>() == 8);
const _: () = assert!(std::mem::size_of::<ColliderHandle>() == 8);
const _: () = assert!(std::mem::size_of::<JointHandle>() == 8);
