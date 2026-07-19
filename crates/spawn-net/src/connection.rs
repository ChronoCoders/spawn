//! Connection lifecycle: salt handshake, keep-alive, timeout, disconnect reasons.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use crate::ack::{AckedSequences, ReliableEndpoint};
use crate::channel::RELIABLE_RESEND_TIMEOUT;
use crate::channel::{
    ChannelId, ReliableReceiver, ReliableSender, SequencedReceiver, SequencedSender,
    MAX_RELIABLE_PAYLOAD, MAX_SEQUENCED_PAYLOAD, RELIABLE_PREFIX, SEQUENCED_PREFIX,
};
use crate::error::{NetError, NetResult};
use crate::event::ClientId;
use crate::frag::{
    FragmentHeader, OutboundFragmented, Reassembled, Reassembler, FRAGMENT_BURST,
    FRAGMENT_HEADER_SIZE, MAX_FRAGMENTED_PAYLOAD,
};
use crate::protocol::{
    control_layout, PacketHeader, PacketType, HEADER_SIZE, MAX_PACKET_SIZE, MAX_PAYLOAD_SIZE,
    PROTOCOL_ID,
};
use crate::stats::{ConnectionStats, StatsTracker};

/// If no other packet was sent to a peer within this interval, a `KeepAlive` is emitted.
pub const KEEP_ALIVE_INTERVAL: Duration = Duration::from_millis(250);
/// If no packet is received from a peer within this deadline, the connection is dropped.
pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);
/// A pending handshake that does not complete within this deadline is discarded.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// Number of redundant `Disconnect` packets sent before freeing a slot locally.
pub const DISCONNECT_REDUNDANCY: u32 = 10;

/// Why a connection ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisconnectReason {
    /// Peer sent a `Disconnect` packet.
    Disconnected,
    /// No packet arrived within `CONNECTION_TIMEOUT`.
    TimedOut,
    /// A pending handshake failed to complete within `HANDSHAKE_TIMEOUT`.
    HandshakeTimeout,
    /// Server refused the connection (client side only).
    Denied(DenyReason),
    /// The local server stopped.
    ServerShutdown,
}

/// Why a server refused a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyReason {
    /// No free client slot.
    ServerFull,
    /// `ChallengeResponse` carried the wrong `connect_salt`.
    InvalidResponse,
    /// `protocol_id` did not match.
    ProtocolMismatch,
}

impl DenyReason {
    pub(crate) fn to_u8(self) -> u8 {
        match self {
            Self::ServerFull => 0,
            Self::InvalidResponse => 1,
            Self::ProtocolMismatch => 2,
        }
    }

    pub(crate) fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::ServerFull),
            1 => Some(Self::InvalidResponse),
            2 => Some(Self::ProtocolMismatch),
            _ => None,
        }
    }
}

/// Deterministic, dependency-free PRNG (SplitMix64) for salt generation. Salts only need
/// to be unpredictable to an off-path attacker (§10); this is seeded from runtime entropy
/// (clock, address, a per-process counter) and is not a cryptographic guarantee.
pub(crate) struct SaltRng {
    state: u64,
}

impl SaltRng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Derive an initial PRNG seed from the wall clock and an address discriminant.
pub(crate) fn seed_from_env(disc: u64) -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mono = Instant::now().elapsed().as_nanos() as u64;
    nanos ^ mono.rotate_left(17) ^ disc.wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// What a received payload datagram resolved to after channel processing.
pub(crate) enum Incoming {
    /// One delivered application message (payload offset/len within the shared arena).
    Message {
        channel: ChannelId,
        offset: usize,
        len: usize,
    },
    /// Nothing to surface (ack-only, duplicate, stale, or reliable buffered for later).
    None,
}

/// A live (or pending) peer connection: reliability endpoint, channels, timers, salts.
///
/// Drives the ack-stamped header on every outgoing packet, maps acks to RTT samples,
/// and enforces per-channel framing. Allocation-free in steady state: all buffers are
/// preallocated at construction.
pub(crate) struct Connection {
    pub addr: SocketAddr,
    pub client_id: ClientId,
    pub connect_salt: u64,
    endpoint: ReliableEndpoint,
    rel_send: ReliableSender,
    rel_recv: ReliableReceiver,
    seq_send: SequencedSender,
    seq_recv: SequencedReceiver,
    stats: StatsTracker,
    last_recv: Instant,
    last_send: Instant,
    send_times: [Option<Instant>; 256],
    /// Maps a packet sequence (mod 256) to the reliable message id it carried, so an ack
    /// frees the right window slot. 256 outstanding sequences far exceeds the in-flight
    /// reliable window under the 100 ms resend timeout, so a slot is always reclaimed
    /// (and any stale entry counted lost) before its sequence aliases a live packet.
    packet_to_msg: [Option<u16>; 256],
    /// Maps a packet sequence (mod 256) to the fragment index it carried, so a peer ack
    /// marks that fragment of the in-flight reliable fragmented message delivered.
    /// Disjoint from `packet_to_msg`: a given sequence carries either a reliable channel
    /// message or a fragment, never both.
    packet_to_frag: [Option<u16>; 256],
    acked_scratch: AckedSequences,
    unreliable: UnreliableQueue,
    sequenced: UnreliableQueue,
    /// The single outbound large message being fragmented (one at a time per
    /// connection); `None` when idle. Reliable: held until all fragments are acked.
    /// Best-effort: sent once on the next flush, then cleared.
    out_frag: Option<OutboundFragmented>,
    next_fragment_id: u16,
    reassembler: Reassembler,
}

/// Fixed-capacity ring of best-effort outgoing messages. Best-effort channels drop the
/// oldest pending frame on overflow (allowed: `Unreliable`/`UnreliableSequenced` make no
/// delivery guarantee); reliable traffic uses the separate backpressured window.
struct UnreliableQueue {
    slots: Vec<([u8; MAX_PAYLOAD_SIZE], usize)>,
    head: usize,
    len: usize,
}

const UNRELIABLE_QUEUE_CAP: usize = 64;

impl UnreliableQueue {
    fn new() -> Self {
        Self {
            slots: vec![([0u8; MAX_PAYLOAD_SIZE], 0); UNRELIABLE_QUEUE_CAP],
            head: 0,
            len: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        let idx = if self.len == UNRELIABLE_QUEUE_CAP {
            let old = self.head;
            self.head = (self.head + 1) % UNRELIABLE_QUEUE_CAP;
            old
        } else {
            let i = (self.head + self.len) % UNRELIABLE_QUEUE_CAP;
            self.len += 1;
            i
        };
        let (buf, l) = &mut self.slots[idx];
        buf[..bytes.len()].copy_from_slice(bytes);
        *l = bytes.len();
    }

    fn pop(&mut self) -> Option<(usize, usize)> {
        if self.len == 0 {
            return None;
        }
        let idx = self.head;
        self.head = (self.head + 1) % UNRELIABLE_QUEUE_CAP;
        self.len -= 1;
        Some((idx, self.slots[idx].1))
    }
}

impl Connection {
    pub fn new(addr: SocketAddr, client_id: ClientId, connect_salt: u64, now: Instant) -> Self {
        Self {
            addr,
            client_id,
            connect_salt,
            endpoint: ReliableEndpoint::new(),
            rel_send: ReliableSender::new(),
            rel_recv: ReliableReceiver::new(),
            seq_send: SequencedSender::new(),
            seq_recv: SequencedReceiver::new(),
            stats: StatsTracker::new(),
            last_recv: now,
            last_send: now,
            send_times: [None; 256],
            packet_to_msg: [None; 256],
            packet_to_frag: [None; 256],
            acked_scratch: AckedSequences::new(),
            unreliable: UnreliableQueue::new(),
            sequenced: UnreliableQueue::new(),
            out_frag: None,
            next_fragment_id: 0,
            reassembler: Reassembler::new(now),
        }
    }

    pub fn stats(&self) -> ConnectionStats {
        self.stats.snapshot()
    }

    pub fn timed_out(&self, now: Instant) -> bool {
        now.duration_since(self.last_recv) >= CONNECTION_TIMEOUT
    }

    /// Whether a keep-alive is due (no packet sent within `KEEP_ALIVE_INTERVAL`).
    pub fn keep_alive_due(&self, now: Instant) -> bool {
        now.duration_since(self.last_send) >= KEEP_ALIVE_INTERVAL
    }

    pub fn mark_recv(&mut self, now: Instant, bytes: usize) {
        self.last_recv = now;
        self.stats.on_received(bytes);
    }

    /// Queue an application message on `channel`. A message within the channel's
    /// single-datagram limit takes the normal path (reliable framing/backpressure
    /// applies). A larger message (up to `MAX_FRAGMENTED_PAYLOAD`) is staged for
    /// fragmentation; `ChannelFull` if another fragmented message is already in flight,
    /// or if the reliable window is saturated. `PayloadTooLarge` above the fragmented
    /// ceiling.
    pub fn enqueue(&mut self, channel: ChannelId, bytes: &[u8]) -> NetResult<()> {
        let single_max = match channel {
            ChannelId::ReliableOrdered => MAX_RELIABLE_PAYLOAD,
            ChannelId::UnreliableSequenced => MAX_SEQUENCED_PAYLOAD,
            ChannelId::Unreliable => MAX_PAYLOAD_SIZE,
        };
        if bytes.len() <= single_max {
            return match channel {
                ChannelId::ReliableOrdered => self.rel_send.queue(bytes).map(|_| ()),
                ChannelId::UnreliableSequenced => {
                    self.sequenced.push(bytes);
                    Ok(())
                }
                ChannelId::Unreliable => {
                    self.unreliable.push(bytes);
                    Ok(())
                }
            };
        }

        // Larger than one datagram → fragment.
        if bytes.len() > MAX_FRAGMENTED_PAYLOAD {
            return Err(NetError::PayloadTooLarge {
                size: bytes.len(),
                max: MAX_FRAGMENTED_PAYLOAD,
            });
        }
        if self.out_frag.is_some() {
            return Err(NetError::ChannelFull);
        }
        let reliable = matches!(channel, ChannelId::ReliableOrdered);
        let fragment_id = self.next_fragment_id;
        self.next_fragment_id = self.next_fragment_id.wrapping_add(1);
        self.out_frag = Some(OutboundFragmented::new(
            bytes,
            fragment_id,
            channel,
            reliable,
        ));
        Ok(())
    }

    /// Counts the unacked previous carrier of reliable message `id` as lost and
    /// clears its ring bookkeeping. Cold path (one 256-entry scan per retransmit).
    fn count_lost_carrier(&mut self, id: u16) {
        for ring in 0..256 {
            if self.packet_to_msg[ring] == Some(id) && self.send_times[ring].is_some() {
                self.stats.on_delivery(true);
                self.send_times[ring] = None;
                self.packet_to_msg[ring] = None;
            }
        }
    }

    fn stamp_send_time(&mut self, seq: u16, now: Instant) {
        let ring = usize::from(seq) % 256;
        // Reclaiming a slot whose prior send was never acked counts that earlier packet
        // as lost: 256 distinct sequences must pass before a slot is reused, so an entry
        // still present here outlived the resend window and will never be acked.
        if self.send_times[ring].is_some() {
            self.stats.on_delivery(true);
            self.packet_to_msg[ring] = None;
            self.packet_to_frag[ring] = None;
        }
        self.send_times[ring] = Some(now);
    }

    /// Build and transmit one payload packet whose body is `body` on `channel`.
    fn transmit_payload(
        &mut self,
        channel: ChannelId,
        body: &[u8],
        scratch: &mut [u8; MAX_PACKET_SIZE],
        now: Instant,
        socket: &mut dyn RawSend,
    ) -> NetResult<u16> {
        let seq = self.endpoint.next_sequence();
        let (ack, ack_bits) = self.endpoint.ack_header();
        let header = PacketHeader {
            protocol_id: PROTOCOL_ID,
            packet_type: PacketType::Payload,
            sequence: seq,
            ack,
            ack_bits,
            channel: channel as u8,
        };
        header.encode(scratch)?;
        scratch[HEADER_SIZE..HEADER_SIZE + body.len()].copy_from_slice(body);
        let total = HEADER_SIZE + body.len();
        socket.raw_send(&scratch[..total], self.addr)?;
        self.stamp_send_time(seq, now);
        self.last_send = now;
        self.stats.on_sent(total);
        Ok(seq)
    }

    /// Build and transmit one `Fragment` datagram (sub-header + piece) on `channel`.
    #[allow(clippy::too_many_arguments)]
    fn transmit_fragment(
        &mut self,
        fragment_id: u16,
        index: usize,
        count: usize,
        channel: ChannelId,
        body: &[u8],
        scratch: &mut [u8; MAX_PACKET_SIZE],
        now: Instant,
        socket: &mut dyn RawSend,
    ) -> NetResult<u16> {
        let seq = self.endpoint.next_sequence();
        let (ack, ack_bits) = self.endpoint.ack_header();
        let header = PacketHeader {
            protocol_id: PROTOCOL_ID,
            packet_type: PacketType::Fragment,
            sequence: seq,
            ack,
            ack_bits,
            channel: channel as u8,
        };
        header.encode(scratch)?;
        let fh = FragmentHeader {
            fragment_id,
            index: index as u16,
            count: count as u16,
        };
        fh.encode(&mut scratch[HEADER_SIZE..HEADER_SIZE + FRAGMENT_HEADER_SIZE])?;
        let body_start = HEADER_SIZE + FRAGMENT_HEADER_SIZE;
        scratch[body_start..body_start + body.len()].copy_from_slice(body);
        let total = body_start + body.len();
        socket.raw_send(&scratch[..total], self.addr)?;
        self.stamp_send_time(seq, now);
        self.last_send = now;
        self.stats.on_sent(total);
        Ok(seq)
    }

    /// Advance the single in-flight fragmented message: send up to [`FRAGMENT_BURST`]
    /// due fragments. Reliable fragments resend on timeout until acked and the message
    /// is dropped when fully acked; best-effort fragments are sent once and the message
    /// is dropped when every fragment has been sent.
    fn flush_fragments(
        &mut self,
        scratch: &mut [u8; MAX_PACKET_SIZE],
        now: Instant,
        socket: &mut dyn RawSend,
    ) -> NetResult<()> {
        let Some(mut of) = self.out_frag.take() else {
            return Ok(());
        };
        let reliable = of.is_reliable();
        let timeout = if reliable {
            RELIABLE_RESEND_TIMEOUT
        } else {
            Duration::MAX
        };
        let mut sent = 0usize;
        // A transmit error must not discard the in-flight message (the reliable
        // invariant: it completes or the connection times out). On error we stop the
        // burst, restore `out_frag`, and propagate, the message resends next flush.
        let mut result = Ok(());
        for i in 0..of.count() {
            if sent >= FRAGMENT_BURST {
                break;
            }
            if of.due(i, now, timeout) {
                match self.transmit_fragment(
                    of.fragment_id(),
                    i,
                    of.count(),
                    of.channel(),
                    of.slice(i),
                    scratch,
                    now,
                    socket,
                ) {
                    Ok(seq) => {
                        of.mark_sent(i, now);
                        if reliable {
                            self.packet_to_frag[usize::from(seq) % 256] = Some(i as u16);
                        }
                        sent += 1;
                    }
                    Err(e) => {
                        result = Err(e);
                        break;
                    }
                }
            }
        }
        let done = result.is_ok()
            && if reliable {
                of.all_acked()
            } else {
                of.all_sent()
            };
        if !done {
            self.out_frag = Some(of);
        }
        result
    }

    /// Drain all queued application traffic, resend due reliable messages, and emit a
    /// keep-alive if the interval elapsed with nothing else sent.
    pub fn flush(
        &mut self,
        scratch: &mut [u8; MAX_PACKET_SIZE],
        now: Instant,
        socket: &mut dyn RawSend,
    ) -> NetResult<()> {
        let mut body = [0u8; MAX_PAYLOAD_SIZE];

        while let Some((idx, len)) = self.unreliable.pop() {
            body[..len].copy_from_slice(&self.unreliable.slots[idx].0[..len]);
            self.transmit_payload(ChannelId::Unreliable, &body[..len], scratch, now, socket)?;
        }

        while let Some((idx, len)) = self.sequenced.pop() {
            let seq = self.seq_send.next_seq();
            body[..SEQUENCED_PREFIX].copy_from_slice(&seq.to_le_bytes());
            body[SEQUENCED_PREFIX..SEQUENCED_PREFIX + len]
                .copy_from_slice(&self.sequenced.slots[idx].0[..len]);
            self.transmit_payload(
                ChannelId::UnreliableSequenced,
                &body[..SEQUENCED_PREFIX + len],
                scratch,
                now,
                socket,
            )?;
        }

        // Reliable resend pass. `take_due` stages due frames (id + bytes) into a
        // preallocated buffer and marks them sent at `now`; we then transmit each and
        // record the carrying packet sequence so the matching ack frees the slot.
        let staged = self.rel_send.take_due(now);
        for i in 0..staged {
            let (id, len) = self.rel_send.staged_frame(i);
            // A retransmit means the previous carrying packet timed out unacked:
            // count it lost (spec §9) and release its seq bookkeeping so a late
            // ack for the old packet cannot double-free the message slot.
            if self.rel_send.staged_is_retransmit(i) {
                self.count_lost_carrier(id);
            }
            body[..RELIABLE_PREFIX].copy_from_slice(&id.to_le_bytes());
            self.rel_send
                .copy_staged_bytes(i, &mut body[RELIABLE_PREFIX..RELIABLE_PREFIX + len]);
            let seq = self.transmit_payload(
                ChannelId::ReliableOrdered,
                &body[..RELIABLE_PREFIX + len],
                scratch,
                now,
                socket,
            )?;
            self.packet_to_msg[usize::from(seq) % 256] = Some(id);
        }

        self.flush_fragments(scratch, now, socket)?;

        if self.keep_alive_due(now) {
            self.send_control(PacketType::KeepAlive, scratch, now, socket)?;
        }
        Ok(())
    }

    /// Emit a control packet (KeepAlive/Disconnect) carrying `connect_salt`.
    pub fn send_control(
        &mut self,
        ty: PacketType,
        scratch: &mut [u8; MAX_PACKET_SIZE],
        now: Instant,
        socket: &mut dyn RawSend,
    ) -> NetResult<()> {
        let seq = self.endpoint.next_sequence();
        let (ack, ack_bits) = self.endpoint.ack_header();
        let header = PacketHeader {
            protocol_id: PROTOCOL_ID,
            packet_type: ty,
            sequence: seq,
            ack,
            ack_bits,
            channel: PacketHeader::NO_CHANNEL,
        };
        header.encode(scratch)?;
        // KeepAlive and Disconnect share an identical body: `connect_salt: u64` @ SALT_OFFSET.
        let salt_at = HEADER_SIZE + control_layout::SALT_OFFSET;
        scratch[salt_at..salt_at + 8].copy_from_slice(&self.connect_salt.to_le_bytes());
        let total = HEADER_SIZE + control_layout::KEEP_ALIVE_LEN;
        socket.raw_send(&scratch[..total], self.addr)?;
        self.last_send = now;
        self.stats.on_sent(total);
        Ok(())
    }

    /// Process a peer header's `(ack, ack_bits)`: free acked reliable messages and feed
    /// RTT samples from send-to-ack timing.
    fn process_acks(&mut self, ack: u16, ack_bits: u32, now: Instant) {
        let mut scratch = std::mem::take(&mut self.acked_scratch);
        self.endpoint.process_acks(ack, ack_bits, &mut scratch);
        for &seq in scratch.as_slice() {
            let ring = usize::from(seq) % 256;
            if let Some(t) = self.send_times[ring].take() {
                self.stats.on_rtt_sample(now.duration_since(t));
                self.stats.on_delivery(false);
            }
            if let Some(msg_id) = self.packet_to_msg[ring].take() {
                self.rel_send.ack(msg_id);
            }
            if let Some(frag_index) = self.packet_to_frag[ring].take() {
                if let Some(of) = self.out_frag.as_mut() {
                    of.mark_acked(usize::from(frag_index));
                }
            }
        }
        self.acked_scratch = scratch;
    }

    /// Process an inbound payload datagram already validated to belong to this peer.
    /// `payload` is the body after the 14-byte header. Returns what to surface.
    pub fn on_payload(
        &mut self,
        header: &PacketHeader,
        channel: ChannelId,
        payload: &[u8],
        arena: &mut Vec<u8>,
        now: Instant,
    ) -> Incoming {
        self.endpoint.on_received(header.sequence);
        self.process_acks(header.ack, header.ack_bits, now);

        match channel {
            ChannelId::Unreliable => {
                let offset = arena.len();
                arena.extend_from_slice(payload);
                Incoming::Message {
                    channel,
                    offset,
                    len: payload.len(),
                }
            }
            ChannelId::UnreliableSequenced => {
                if payload.len() < SEQUENCED_PREFIX {
                    return Incoming::None;
                }
                let seq = u16::from_le_bytes([payload[0], payload[1]]);
                if !self.seq_recv.accept(seq) {
                    return Incoming::None;
                }
                let data = &payload[SEQUENCED_PREFIX..];
                let offset = arena.len();
                arena.extend_from_slice(data);
                Incoming::Message {
                    channel,
                    offset,
                    len: data.len(),
                }
            }
            ChannelId::ReliableOrdered => {
                if payload.len() < RELIABLE_PREFIX {
                    return Incoming::None;
                }
                let id = u16::from_le_bytes([payload[0], payload[1]]);
                let data = &payload[RELIABLE_PREFIX..];
                self.rel_recv.accept(id, data);
                // Delivery of in-order prefix is handled by `drain_reliable`.
                Incoming::None
            }
        }
    }

    /// Process an inbound `Fragment` datagram. `payload` is the body after the 14-byte
    /// header (a 6-byte fragment sub-header plus the piece). On the fragment that
    /// completes the message, the reassembled bytes are copied into `arena` and
    /// surfaced as a `Message` on the original channel; otherwise nothing surfaces.
    pub fn on_fragment(
        &mut self,
        header: &PacketHeader,
        payload: &[u8],
        arena: &mut Vec<u8>,
        now: Instant,
    ) -> Incoming {
        self.endpoint.on_received(header.sequence);
        self.process_acks(header.ack, header.ack_bits, now);

        let Ok(fh) = FragmentHeader::decode(payload) else {
            return Incoming::None;
        };
        let Ok(channel) = ChannelId::try_from(header.channel) else {
            return Incoming::None;
        };
        let body = &payload[FRAGMENT_HEADER_SIZE..];
        match self.reassembler.accept(channel, fh, body, now) {
            Some(Reassembled { slot, channel, len }) => {
                let offset = arena.len();
                arena.extend_from_slice(self.reassembler.bytes(slot));
                Incoming::Message {
                    channel,
                    offset,
                    len,
                }
            }
            None => Incoming::None,
        }
    }

    /// Pop the next in-order reliable message into `arena`, if available.
    pub fn drain_reliable(&mut self, arena: &mut Vec<u8>) -> Option<(usize, usize)> {
        let mut out = None;
        self.rel_recv.pop_next(|bytes| {
            let offset = arena.len();
            arena.extend_from_slice(bytes);
            out = Some((offset, bytes.len()));
        });
        out
    }

    /// Note a non-payload control packet receipt (updates ack view and liveness).
    pub fn on_control(&mut self, header: &PacketHeader, now: Instant) {
        self.endpoint.on_received(header.sequence);
        self.process_acks(header.ack, header.ack_bits, now);
    }
}

/// Abstraction over the raw datagram sink so `Connection` stays socket-agnostic and the
/// owner (server/client) controls the actual `UdpSocket`.
pub(crate) trait RawSend {
    fn raw_send(&mut self, bytes: &[u8], addr: SocketAddr) -> NetResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::MAX_PAYLOAD_SIZE;

    struct FailSink;
    impl RawSend for FailSink {
        fn raw_send(&mut self, _bytes: &[u8], _addr: SocketAddr) -> NetResult<()> {
            Err(NetError::Io(std::io::Error::other("transmit failure")))
        }
    }

    fn conn() -> Connection {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        Connection::new(addr, ClientId(1), 0x1234_5678, Instant::now())
    }

    #[test]
    fn transmit_error_preserves_in_flight_fragmented_message() {
        let mut c = conn();
        let msg = vec![7u8; MAX_PAYLOAD_SIZE * 2];
        c.enqueue(ChannelId::ReliableOrdered, &msg).unwrap();

        // A failing transmit propagates the error but must NOT discard the message.
        let mut scratch = [0u8; MAX_PACKET_SIZE];
        let mut sink = FailSink;
        assert!(c.flush(&mut scratch, Instant::now(), &mut sink).is_err());

        assert!(matches!(
            c.enqueue(ChannelId::ReliableOrdered, &msg),
            Err(NetError::ChannelFull)
        ));
    }
}
