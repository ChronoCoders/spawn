//! The 1-byte message-kind tag that multiplexes replication traffic over the
//! `spawn-net` channels (spec §9). The tag is the first byte of every replication
//! payload; the transport stays oblivious to replication semantics.

/// Server → client snapshot (carried on `UnreliableSequenced`).
pub(crate) const TAG_SNAPSHOT: u8 = 0;
/// Client → server input packet: the snapshot ack + the redundant input window
/// (carried on `UnreliableSequenced`).
pub(crate) const TAG_INPUT: u8 = 1;
/// Either direction: an RPC (carried on `ReliableOrdered`).
pub(crate) const TAG_RPC: u8 = 2;
