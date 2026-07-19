#![deny(warnings)]

//! Server-authoritative replication for the Spawn engine.
//!
//! Sits above the `spawn-net` byte transport: it decides what state each client
//! should hold and keeps clients converged on the server's authoritative `World`
//! through interest-filtered, delta-compressed snapshots, while routing client input
//! and ownership-gated RPCs back to the server. It owns *mechanism*; the game owns
//! *policy* (which components replicate, RPC semantics + validation, ownership
//! assignment, relevancy parameters). Driven once per fixed tick, no threads, no
//! async, no internal clock.
//!
//! This first module set establishes the identity and classification layer: the dense
//! [`ReplId`] space, the relevancy [`markers`], and the derived [`NetRole`] triad.

pub mod client;
pub mod config;
pub mod error;
pub mod id;
pub mod interest;
pub mod interp;
pub mod markers;
pub mod predict;
pub mod registry;
pub mod role;
pub mod rpc;
pub mod server;
pub mod snapshot;
mod wire;

#[cfg(test)]
mod testcomp;

pub use error::{ReplError, ReplResult};
pub use id::{ReplId, ReplIdMap};
pub use interest::{ReplicationVisibility, VisibilityConfig};
pub use interp::{default_interp_delay, InterpBuffer, InterpolatedTransform};
pub use markers::{AlwaysRelevant, OwnerOnly, Replicated, StaticRelevant};
pub use predict::{replay, InputBuffer, Predicted, PredictionSmoother, SMOOTH_DECAY, SNAP_EPSILON};
pub use registry::{ReplComponentId, Replicate, ReplicationRegistry};
pub use role::NetRole;
pub use rpc::{
    decode_rpc, decode_rpc_header, encode_rpc, server_rpc_authorized, Rpc, RpcHeader, RpcId,
    RpcKind, RpcRegistry,
};
pub use snapshot::{
    decode_snapshot, encode_snapshot, peek_snapshot_header, DecodeOutcome, SnapshotState,
    SEND_BUDGET_BYTES, SNAPSHOT_HISTORY, SNAPSHOT_HZ, SNAPSHOT_INTERVAL,
};

pub use client::{ClientEvents, ClientRpc, ReplicationClient};
pub use config::ReplicationConfig;
pub use server::{Replicator, ServerEvents, ServerInput, ServerRpc};
