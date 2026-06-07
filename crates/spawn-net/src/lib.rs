#![deny(warnings)]

//! UDP transport and reliability layer for the Spawn engine.
//!
//! Phase 1 scope: a byte-precise packet protocol, RFC 1982 sequence arithmetic, a
//! salt-based connection handshake with keep-alive/timeout/disconnect, poll-driven
//! [`Server`] and [`Client`], three delivery channels ([`ChannelId`]), and per-connection
//! statistics. No threads, no async, no external crates — callers drive progress via
//! `poll`. See the crate spec for the normative wire format and security limits.

pub mod ack;
pub mod channel;
pub mod client;
pub mod connection;
pub mod error;
pub mod event;
pub mod protocol;
pub mod sequence;
pub mod server;
pub mod stats;

pub use ack::{AckedSequences, ReliableEndpoint};
pub use channel::{ChannelId, RELIABLE_RECV_WINDOW, RELIABLE_RESEND_TIMEOUT, RELIABLE_SEND_WINDOW};
pub use client::{Client, ClientState};
pub use connection::{
    DenyReason, DisconnectReason, CONNECTION_TIMEOUT, DISCONNECT_REDUNDANCY, HANDSHAKE_TIMEOUT,
    KEEP_ALIVE_INTERVAL,
};
pub use error::{NetError, NetResult};
pub use event::{ClientId, NetEvent, NetEventIter};
pub use protocol::{
    PacketHeader, PacketType, ACK_BITS, HEADER_SIZE, MAX_PACKET_SIZE, MAX_PAYLOAD_SIZE, PROTOCOL_ID,
};
pub use sequence::{sequence_diff, sequence_greater_than, sequence_less_than};
pub use server::{Server, ServerConfig};
pub use stats::ConnectionStats;
