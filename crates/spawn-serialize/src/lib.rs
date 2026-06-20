#![deny(warnings)]

//! Structured bit-level serialization codec for the Spawn engine.
//!
//! The bridge between structured game state and the byte payloads `spawn-net`
//! carries: a bit-precise [`BitWriter`]/[`BitReader`], a direction-agnostic
//! [`Stream`] trait so each type has one read/write-symmetric [`Serialize`]
//! function, and helpers for integers (incl. zig-zag and bounded ranges), quantized
//! floats, positions, and smallest-three unit quaternions.
//!
//! It is a pure codec: it has no opinion about ECS components, replication, schemas,
//! or transport. The Phase 2d replication layer composes these primitives to encode
//! component state into `spawn-net` channel payloads (and fragments). Dependency
//! surface is `spawn-core` + `std`, zero `unsafe`.

pub mod bits;
pub mod error;
pub mod geom;
pub mod pack;
pub mod stream;
mod transform;

pub use bits::{BitReader, BitWriter};
pub use error::{SerializeError, SerializeResult};
pub use geom::{serialize_position, serialize_unit_quat, PositionBounds};
pub use pack::{serialize_bounded, serialize_int, serialize_quantized_f32, serialize_uint};
pub use stream::{Serialize, Stream};
