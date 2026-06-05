#![deny(warnings)]

//! Foundation crate of the Spawn engine: math, primitives, error types, and shared traits.

pub mod error;
pub mod math;
pub mod primitives;
pub mod traits;

pub use error::{SpawnError, SpawnResult};
pub use math::{
    inverse_lerp, lerp, remap, wrap_angle, Mat3, Mat4, Quat, Vec2, Vec3, Vec4, EPSILON,
};
pub use primitives::{Color, Rect, Transform2D, Transform3D, AABB2, AABB3};
pub use traits::{ApproxEq, Lerp};
