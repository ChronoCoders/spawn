#![deny(warnings)]

//! Server-authoritative replication for the Spawn engine.
//!
//! Sits above the `spawn-net` byte transport: it decides what state each client
//! should hold and keeps clients converged on the server's authoritative `World`
//! through interest-filtered, delta-compressed snapshots, while routing client input
//! and ownership-gated RPCs back to the server. It owns *mechanism*; the game owns
//! *policy* (which components replicate, RPC semantics + validation, ownership
//! assignment, relevancy parameters). Driven once per fixed tick — no threads, no
//! async, no internal clock.
//!
//! This first module set establishes the identity and classification layer: the dense
//! [`ReplId`] space, the relevancy [`markers`], and the derived [`NetRole`] triad.

pub mod id;
pub mod interest;
pub mod markers;
pub mod role;

pub use id::{ReplId, ReplIdMap};
pub use interest::{ReplicationVisibility, VisibilityConfig};
pub use markers::{AlwaysRelevant, OwnerOnly, Replicated, StaticRelevant};
pub use role::NetRole;
