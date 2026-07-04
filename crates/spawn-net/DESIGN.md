# spawn-net

`spawn-net` is the UDP transport and reliability layer for the engine. It moves opaque `&[u8]` payloads between a `Server` and one or more `Client`s over a single non-blocking UDP socket per endpoint, layering a byte-precise packet header, wrapping-sequence acknowledgement, a salt-based connection handshake, and three delivery channels (`Unreliable`, `UnreliableSequenced`, `ReliableOrdered`) on top of raw datagrams. It exists because higher-level replication features: snapshots, prediction, rollback, need a transport with reliable-ordered delivery, connection lifecycle management, and per-connection statistics, but none of those higher-level concerns belong in the wire layer itself. Everything above the byte stream is deliberately someone else's problem.

## Design Decisions

The crate is single-threaded and poll-driven. No threads are spawned, no async runtime is involved, and there is no internal clock thread. Callers advance all progress: socket drain, timer expiry, handshake steps, resends, by calling `poll` once per tick. Time is read on demand through `std::time::Instant`. This keeps the transport deterministic from the caller's perspective and trivially embeddable in a fixed-tick game loop, and it sidesteps the entire class of concurrency bugs that a threaded or async transport would invite. `tokio`, `quinn`, and any async machinery are rejected outright for this reason.

The socket is non-blocking. Every receive path drains in a loop until `WouldBlock`, and no operation ever blocks the caller. `WouldBlock` is an internal control-flow signal, never an error surfaced upward.

Serialization is excluded. The transport is byte-oriented and carries `&[u8]` only; it has no opinion about message structure. Structured encoding belongs with snapshot delta encoding in a later phase, so `serde` is deliberately absent now rather than pulled in speculatively. Keeping the dependency graph at `spawn-core` plus `std` also keeps compile times and audit surface minimal.

The wire format is little-endian on every multi-byte field, encoded and decoded explicitly via `to_le_bytes` / `from_le_bytes`. The format never depends on host endianness, and the header is treated as a logical struct serialized field-by-field. `#[repr(C)]` and transmutes are not used, which is both a safety stance and a portability guarantee.

Reliability uses RFC 1982 half-range serial-number arithmetic over `u16` sequence numbers, combined with a 32-bit ack bitfield. A single `ack` plus `ack_bits` confirms up to 33 sequences per packet, so acknowledgement piggybacks on ordinary traffic with no dedicated ack packets. Raw `<` / `>` comparison on sequence numbers is forbidden across the crate; the sanctioned comparison functions are the only correct way to order wrapping sequences.

Reliable-ordered delivery applies backpressure rather than silent drops. When the unacked send window is full, `send` returns `ChannelFull` and the caller retries later. A reliable message is never discarded to make room: the guarantee would be meaningless otherwise.

The salt handshake exists to raise the cost of casual spoofing, not to provide security. An off-path attacker who never observes the `Challenge` cannot reconstruct the combined connection salt and so cannot blind-inject control packets. That is the full extent of the claim: no encryption, no defense against on-path attackers, replay, or tampering. The `PROTOCOL_ID` magic is a version guard, not a security control.

## Architecture

The crate is a flat set of single-responsibility modules under `src/`, with `lib.rs` declaring them and re-exporting all public types at the crate root (`spawn_net::Server`, `spawn_net::NetError`, etc.) while leaving the modules public for explicit paths.

- `protocol`: protocol constants (`PROTOCOL_ID`, `HEADER_SIZE = 14`, `MAX_PACKET_SIZE = 1200`, `MAX_PAYLOAD_SIZE`, `ACK_BITS`), the `PacketHeader` struct with `encode`/`decode`, the `PacketType` discriminant enum, and the named offset constants for every control-packet payload layout. All wire encoding and decoding lives here or references constants defined here.
- `sequence`: the three wrapping-comparison primitives: `sequence_greater_than`, `sequence_less_than`, `sequence_diff`. Pure functions, the only sanctioned sequence ordering in the crate.
- `ack`: `ReliableEndpoint`, which allocates outgoing sequence numbers, records received sequences, produces the `(ack, ack_bits)` pair for outgoing headers, and decodes a peer's acks into the set of newly-confirmed local sequences. `AckedSequences` is its fixed-capacity (≤ 33) stack output buffer.
- `channel`: `ChannelId` and the per-channel send/resend/reorder buffers, including the backpressure logic and window-bound enforcement for `ReliableOrdered`.
- `connection`: handshake state, keep-alive and timeout timers, redundant graceful disconnect, and the `DisconnectReason` / `DenyReason` enums. Salt validation on inbound `Disconnect` and `KeepAlive` lives here.
- `event`: `NetEvent`, the `NetEventIter` returned by `poll`, and `ClientId`. `NetEvent::Message` borrows the internal receive buffer and is valid only until the next `poll`.
- `server`: `Server` and `ServerConfig`: bind, poll, send, broadcast, per-client disconnect, stats, and slot management bounded by `max_clients`.
- `client`: `Client` and the `ClientState` machine (`Disconnected` → `Connecting` → `Connected`): new, connect, poll, send, disconnect, stats.
- `stats`: `ConnectionStats` (smoothed RTT, packet-loss estimate, byte and packet counters).
- `error`: `NetError` (`#[non_exhaustive]`), `NetResult`, and the trait impls including `From<io::Error>` and the `spawn_core::SpawnError` interop conversion.

The public surface is two driver types, `Server` and `Client`, each exposing a `poll` that returns a borrowing event iterator, a `send` that queues a payload on a channel, lifecycle calls, and read-only stats accessors. Everything else is supporting types: headers, channel ids, events, stats, and errors.

## Constraints

- **Allocation:** All packet buffers, the receive buffer, and per-channel resend and reorder buffers are preallocated at construction and reused. Steady-state `poll` and `send` allocate nothing. Connection setup and teardown may allocate, bounded by `max_clients`. No per-message or per-packet heap allocation on the hot path.
- **Safety:** Zero `unsafe`. The header is serialized field-by-field, never transmuted. No `#[repr(C)]` reliance.
- **Panics:** No `unwrap()`, `expect()`, or `panic!()` in non-test code. Every fallible operation returns `Option` or `NetResult`. `unwrap` and `assert` are permitted only inside `#[cfg(test)]`.
- **Dependencies:** `spawn-core` (for `SpawnError` interop) and `std` only. `tokio`, `quinn`, and `serde` are forbidden in Phase 1. No other external crates.
- **Wire format:** All multi-byte fields little-endian, encoded explicitly. The format is host-endianness-independent. Every packet carries the fixed 14-byte header; mismatched `PROTOCOL_ID` packets are dropped silently.
- **Sequence ordering:** Raw `<` / `>` on sequence numbers is forbidden. Only the `sequence` module's comparison functions order sequences.
- **Reliability invariant:** No `ReliableOrdered` message is ever dropped to free window space; a full window yields `ChannelFull` backpressure instead. In-window duplicates are acked and discarded; out-of-window inbound reliable messages are dropped as a protocol-violation guard.
- **Non-blocking I/O:** The socket is non-blocking; all receive paths drain to `WouldBlock`, which never surfaces as an error. One `recv_from` drain loop per `poll`.
- **Payload ceiling:** `MAX_PAYLOAD_SIZE` is a hard limit. Oversized payloads return `PayloadTooLarge` rather than fragmenting.
- **Documentation:** Public items carry `///` doc comments stating their contract.

## Phase 1 Scope

In scope: the UDP transport and reliability layer in full, the byte-precise packet header and its encode/decode, wrapping sequence arithmetic, the salt-based handshake and connection lifecycle (connect, keep-alive, timeout, graceful and redundant disconnect), the `Server` and `Client` driver types with poll-driven borrowing event streams, the three delivery channels with their ordering and backpressure semantics, per-connection statistics (RTT, packet-loss estimate, byte and packet counters), the error type with `spawn-core` interop, and unit plus loopback integration tests covering header round-trips, sequence wraparound, ack reconstruction, handshake outcomes, reliable delivery under simulated loss, sequenced ordering, backpressure, timeout, oversize rejection, and allocation discipline.

Deferred to later phases, each gated behind its own design sign-off: snapshot interpolation, client-side prediction, rollback and resimulation, delta and snapshot encoding plus any structured serialization, packet fragmentation and reassembly, congestion control and bandwidth shaping beyond the MTU cap, encryption (DTLS-equivalent), IPv6-specific tuning, NAT traversal and hole punching, and matchmaking.

The line sits exactly at the boundary between moving bytes reliably and interpreting them. Fragmentation, serialization, and congestion control all require design commitments that depend on the replication model above the transport, so pulling them forward would couple the wire layer to decisions not yet made. Encryption is deferred as a deliberate, documented gap rather than a half-measure. Drawing the boundary here yields a small, fully-tested, dependency-light transport that the replication phases build on without rework. The `#[non_exhaustive]` error enum reserves room for the variants those later features will introduce.
