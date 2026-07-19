//! Packet fragmentation and reassembly for application messages larger than a
//! single datagram (the initial baseline snapshot, large reliable RPCs).
//!
//! A fragmented message is split into [`FRAGMENT_PAYLOAD`]-sized pieces, each sent as
//! a `PacketType::Fragment` datagram whose body is a 6-byte [`FragmentHeader`] plus
//! the piece. The normal sub-MTU path is untouched, only `send`/`broadcast` payloads
//! above a channel's single-datagram limit fragment. Reliability reuses the
//! connection's existing per-packet acks (see `connection.rs`); the unreliable path is
//! best-effort with a reassembly timeout.
//!
//! All buffers are preallocated; steady-state send and reassembly allocate nothing.

use std::time::{Duration, Instant};

use crate::channel::ChannelId;
use crate::error::{NetError, NetResult};
use crate::protocol::MAX_PAYLOAD_SIZE;
use crate::sequence::sequence_greater_than;

/// Fragment sub-header length: `fragment_id: u16`, `index: u16`, `count: u16` (LE).
pub const FRAGMENT_HEADER_SIZE: usize = 6;
/// Application bytes carried per fragment datagram (after the sub-header).
pub const FRAGMENT_PAYLOAD: usize = MAX_PAYLOAD_SIZE - FRAGMENT_HEADER_SIZE;
/// Hard cap on fragments per message; bounds reassembly buffers and the index space.
pub const MAX_FRAGMENTS: usize = 256;
/// Largest application message acceptable to `send` once fragmentation is available.
pub const MAX_FRAGMENTED_PAYLOAD: usize = FRAGMENT_PAYLOAD * MAX_FRAGMENTS;
/// Concurrent inbound messages reassembled per connection (best-effort path).
pub const MAX_REASSEMBLY: usize = 4;
/// A partial inbound message untouched for this long is abandoned.
pub const REASSEMBLY_TIMEOUT: Duration = Duration::from_millis(2000);
/// Maximum fragment datagrams emitted per `flush`. Bounds use of the connection's
/// 256-entry packet send-time ring so a flush's fragment sequences cannot alias a
/// still-unacked slot, and lightly paces a large transfer (not congestion control).
pub const FRAGMENT_BURST: usize = 32;

/// The 6-byte fragment sub-header carried at the start of a `Fragment` datagram body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FragmentHeader {
    /// Groups the fragments of one logical message (per connection, per direction).
    pub fragment_id: u16,
    /// This fragment's position, `0..count`.
    pub index: u16,
    /// Total fragments in the message, `1..=MAX_FRAGMENTS`.
    pub count: u16,
}

impl FragmentHeader {
    /// Serialize into `out` (little-endian). `BufferTooSmall` if `out` is shorter than
    /// [`FRAGMENT_HEADER_SIZE`].
    pub fn encode(self, out: &mut [u8]) -> NetResult<()> {
        if out.len() < FRAGMENT_HEADER_SIZE {
            return Err(NetError::BufferTooSmall);
        }
        out[0..2].copy_from_slice(&self.fragment_id.to_le_bytes());
        out[2..4].copy_from_slice(&self.index.to_le_bytes());
        out[4..6].copy_from_slice(&self.count.to_le_bytes());
        Ok(())
    }

    /// Parse from the front of `src`. `MalformedPacket` if `src` is too short or the
    /// `(index, count)` pair is invalid (`count` in `1..=MAX_FRAGMENTS`, `index < count`).
    pub fn decode(src: &[u8]) -> NetResult<Self> {
        if src.len() < FRAGMENT_HEADER_SIZE {
            return Err(NetError::MalformedPacket);
        }
        let fragment_id = u16::from_le_bytes([src[0], src[1]]);
        let index = u16::from_le_bytes([src[2], src[3]]);
        let count = u16::from_le_bytes([src[4], src[5]]);
        let count_usize = usize::from(count);
        if count_usize == 0 || count_usize > MAX_FRAGMENTS || index >= count {
            return Err(NetError::MalformedPacket);
        }
        Ok(Self {
            fragment_id,
            index,
            count,
        })
    }
}

/// Number of fragments a `len`-byte message splits into.
pub fn fragment_count(len: usize) -> usize {
    len.div_ceil(FRAGMENT_PAYLOAD).max(1)
}

/// One outbound large message in flight. For a reliable channel it lives until every
/// fragment is acked; for a best-effort channel it is sent once and dropped.
pub(crate) struct OutboundFragmented {
    data: Vec<u8>,
    count: usize,
    fragment_id: u16,
    channel: ChannelId,
    reliable: bool,
    acked: Vec<bool>,
    last_sent: Vec<Option<Instant>>,
}

impl OutboundFragmented {
    /// Stage `bytes` (already validated `1..=MAX_FRAGMENTED_PAYLOAD`) as `fragment_id`.
    pub fn new(bytes: &[u8], fragment_id: u16, channel: ChannelId, reliable: bool) -> Self {
        let count = fragment_count(bytes.len());
        Self {
            data: bytes.to_vec(),
            count,
            fragment_id,
            channel,
            reliable,
            acked: vec![false; count],
            last_sent: vec![None; count],
        }
    }

    pub fn channel(&self) -> ChannelId {
        self.channel
    }

    pub fn is_reliable(&self) -> bool {
        self.reliable
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn fragment_id(&self) -> u16 {
        self.fragment_id
    }

    /// The byte slice for fragment `index` (the last one may be shorter).
    pub fn slice(&self, index: usize) -> &[u8] {
        let start = index * FRAGMENT_PAYLOAD;
        let end = (start + FRAGMENT_PAYLOAD).min(self.data.len());
        &self.data[start..end]
    }

    /// Whether fragment `index` should be (re)sent now: never sent, or sent before
    /// `now - timeout` and still unacked.
    pub fn due(&self, index: usize, now: Instant, timeout: Duration) -> bool {
        if self.acked[index] {
            return false;
        }
        match self.last_sent[index] {
            None => true,
            Some(t) => now.duration_since(t) >= timeout,
        }
    }

    pub fn mark_sent(&mut self, index: usize, now: Instant) {
        self.last_sent[index] = Some(now);
    }

    pub fn mark_acked(&mut self, index: usize) {
        if index < self.count {
            self.acked[index] = true;
        }
    }

    pub fn all_acked(&self) -> bool {
        self.acked.iter().all(|&a| a)
    }

    /// Whether every fragment has been transmitted at least once (best-effort
    /// completion: the message is dropped after one full pass).
    pub fn all_sent(&self) -> bool {
        self.last_sent.iter().all(Option::is_some)
    }
}

/// One inbound reassembly buffer.
struct ReassemblySlot {
    active: bool,
    channel: ChannelId,
    fragment_id: u16,
    count: usize,
    received: Vec<bool>,
    recv_count: usize,
    data: Vec<u8>,
    total_len: usize,
    last_activity: Instant,
}

impl ReassemblySlot {
    fn new(now: Instant) -> Self {
        Self {
            active: false,
            channel: ChannelId::Unreliable,
            fragment_id: 0,
            count: 0,
            received: vec![false; MAX_FRAGMENTS],
            recv_count: 0,
            data: vec![0u8; MAX_FRAGMENTED_PAYLOAD],
            total_len: 0,
            last_activity: now,
        }
    }

    fn begin(&mut self, channel: ChannelId, fragment_id: u16, count: usize, now: Instant) {
        self.active = true;
        self.channel = channel;
        self.fragment_id = fragment_id;
        self.count = count;
        for r in self.received[..count].iter_mut() {
            *r = false;
        }
        self.recv_count = 0;
        self.total_len = 0;
        self.last_activity = now;
    }
}

/// Fixed pool of reassembly buffers. Matches incoming fragments to an in-progress
/// message or starts a new one, evicting a free / timed-out / least-recently-active
/// slot when full.
pub(crate) struct Reassembler {
    slots: Vec<ReassemblySlot>,
    /// The most recently completed `fragment_id`. Because at most one fragmented
    /// message is in flight per connection, fragment ids arrive in order; a fragment
    /// whose id is not newer than this is a stale resend of an already-delivered
    /// message and is dropped, so a reliable message is never delivered twice.
    last_completed: Option<u16>,
}

/// A completed reassembly: which slot holds it and how many bytes are valid.
pub(crate) struct Reassembled {
    pub slot: usize,
    pub channel: ChannelId,
    pub len: usize,
}

impl Reassembler {
    pub fn new(now: Instant) -> Self {
        let mut slots = Vec::with_capacity(MAX_REASSEMBLY);
        for _ in 0..MAX_REASSEMBLY {
            slots.push(ReassemblySlot::new(now));
        }
        Self {
            slots,
            last_completed: None,
        }
    }

    /// Read-only view of a completed slot's bytes (valid until the slot is reused).
    pub fn bytes(&self, slot: usize) -> &[u8] {
        &self.slots[slot].data[..self.slots[slot].total_len]
    }

    fn find_active(&self, channel: ChannelId, fragment_id: u16) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| s.active && s.channel == channel && s.fragment_id == fragment_id)
    }

    /// Pick a slot to (re)use: a free one, else a timed-out one, else the least
    /// recently active.
    fn pick_slot(&self, now: Instant) -> usize {
        if let Some(i) = self.slots.iter().position(|s| !s.active) {
            return i;
        }
        if let Some(i) = self
            .slots
            .iter()
            .position(|s| now.duration_since(s.last_activity) >= REASSEMBLY_TIMEOUT)
        {
            return i;
        }
        let mut oldest = 0;
        for i in 1..self.slots.len() {
            if self.slots[i].last_activity < self.slots[oldest].last_activity {
                oldest = i;
            }
        }
        oldest
    }

    /// Accept fragment `header`/`body` on `channel`. Returns `Some` when the message is
    /// complete (the caller copies [`bytes`](Reassembler::bytes) and the slot is freed).
    /// Duplicate or invalid fragments yield `None`.
    pub fn accept(
        &mut self,
        channel: ChannelId,
        header: FragmentHeader,
        body: &[u8],
        now: Instant,
    ) -> Option<Reassembled> {
        if body.len() > FRAGMENT_PAYLOAD {
            return None;
        }
        // Drop stale resends of an already-completed (or older) message.
        if let Some(lc) = self.last_completed {
            if !sequence_greater_than(header.fragment_id, lc) {
                return None;
            }
        }
        let count = usize::from(header.count);
        let index = usize::from(header.index);

        let slot = match self.find_active(channel, header.fragment_id) {
            Some(i) => i,
            None => {
                let i = self.pick_slot(now);
                self.slots[i].begin(channel, header.fragment_id, count, now);
                i
            }
        };
        let s = &mut self.slots[slot];
        // A stale active slot with a different count for the same id: restart it.
        if s.count != count {
            s.begin(channel, header.fragment_id, count, now);
        }
        if s.received[index] {
            return None;
        }

        let start = index * FRAGMENT_PAYLOAD;
        let end = start + body.len();
        s.data[start..end].copy_from_slice(body);
        s.received[index] = true;
        s.recv_count += 1;
        s.total_len = s.total_len.max(end);
        s.last_activity = now;

        let completed = s.recv_count == s.count;
        let total = s.total_len;
        if completed {
            s.active = false;
        }
        if completed {
            self.last_completed = Some(header.fragment_id);
            Some(Reassembled {
                slot,
                channel,
                len: total,
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Instant {
        Instant::now()
    }

    #[test]
    fn header_round_trip_and_validation() {
        let h = FragmentHeader {
            fragment_id: 0xBEEF,
            index: 3,
            count: 10,
        };
        let mut buf = [0u8; FRAGMENT_HEADER_SIZE];
        h.encode(&mut buf).unwrap();
        assert_eq!(FragmentHeader::decode(&buf).unwrap(), h);

        // count 0 invalid
        let mut bad = [0u8; 6];
        bad[4] = 0;
        assert!(matches!(
            FragmentHeader::decode(&bad),
            Err(NetError::MalformedPacket)
        ));
        // index >= count invalid
        let h2 = FragmentHeader {
            fragment_id: 1,
            index: 10,
            count: 10,
        };
        h2.encode(&mut buf).unwrap();
        assert!(matches!(
            FragmentHeader::decode(&buf),
            Err(NetError::MalformedPacket)
        ));
        // short buffer
        assert!(matches!(
            FragmentHeader::decode(&[0u8; 5]),
            Err(NetError::MalformedPacket)
        ));
    }

    #[test]
    fn fragment_count_math() {
        assert_eq!(fragment_count(0), 1);
        assert_eq!(fragment_count(1), 1);
        assert_eq!(fragment_count(FRAGMENT_PAYLOAD), 1);
        assert_eq!(fragment_count(FRAGMENT_PAYLOAD + 1), 2);
        assert_eq!(fragment_count(FRAGMENT_PAYLOAD * 3), 3);
    }

    #[test]
    fn outbound_slices_cover_message() {
        let msg: Vec<u8> = (0..(FRAGMENT_PAYLOAD + 100)).map(|i| i as u8).collect();
        let of = OutboundFragmented::new(&msg, 7, ChannelId::ReliableOrdered, true);
        assert_eq!(of.count(), 2);
        assert_eq!(of.slice(0).len(), FRAGMENT_PAYLOAD);
        assert_eq!(of.slice(1).len(), 100);
        let mut joined = of.slice(0).to_vec();
        joined.extend_from_slice(of.slice(1));
        assert_eq!(joined, msg);
    }

    #[test]
    fn reassemble_in_and_out_of_order() {
        let msg: Vec<u8> = (0..(FRAGMENT_PAYLOAD * 2 + 50)).map(|i| i as u8).collect();
        let of = OutboundFragmented::new(&msg, 9, ChannelId::ReliableOrdered, true);
        let count = of.count();
        assert_eq!(count, 3);
        let mut r = Reassembler::new(now());
        // Deliver fragments out of order: 2, 0, 1.
        let order = [2usize, 0, 1];
        let mut done = None;
        for &i in &order {
            let h = FragmentHeader {
                fragment_id: 9,
                index: i as u16,
                count: count as u16,
            };
            done = r.accept(ChannelId::ReliableOrdered, h, of.slice(i), now());
        }
        let d = done.expect("message completes on last fragment");
        assert_eq!(r.bytes(d.slot), &msg[..]);
    }

    #[test]
    fn duplicate_fragment_is_ignored() {
        let msg = vec![1u8; FRAGMENT_PAYLOAD + 10];
        let of = OutboundFragmented::new(&msg, 1, ChannelId::ReliableOrdered, true);
        let mut r = Reassembler::new(now());
        let h0 = FragmentHeader {
            fragment_id: 1,
            index: 0,
            count: 2,
        };
        assert!(r
            .accept(ChannelId::ReliableOrdered, h0, of.slice(0), now())
            .is_none());
        // Duplicate of fragment 0 does not advance.
        assert!(r
            .accept(ChannelId::ReliableOrdered, h0, of.slice(0), now())
            .is_none());
        let h1 = FragmentHeader {
            fragment_id: 1,
            index: 1,
            count: 2,
        };
        assert!(r
            .accept(ChannelId::ReliableOrdered, h1, of.slice(1), now())
            .is_some());
    }

    #[test]
    fn completed_message_resend_is_dropped() {
        let msg = vec![3u8; FRAGMENT_PAYLOAD + 5];
        let of = OutboundFragmented::new(&msg, 5, ChannelId::ReliableOrdered, true);
        let mut r = Reassembler::new(now());
        let h0 = FragmentHeader {
            fragment_id: 5,
            index: 0,
            count: 2,
        };
        let h1 = FragmentHeader {
            fragment_id: 5,
            index: 1,
            count: 2,
        };
        assert!(r
            .accept(ChannelId::ReliableOrdered, h0, of.slice(0), now())
            .is_none());
        assert!(r
            .accept(ChannelId::ReliableOrdered, h1, of.slice(1), now())
            .is_some());
        // A full resend of the same id (sender still resending until acked) must NOT
        // re-deliver the message.
        assert!(r
            .accept(ChannelId::ReliableOrdered, h0, of.slice(0), now())
            .is_none());
        assert!(r
            .accept(ChannelId::ReliableOrdered, h1, of.slice(1), now())
            .is_none());
        // A newer message id is still accepted and delivered.
        let msg2 = vec![4u8; FRAGMENT_PAYLOAD + 5];
        let of2 = OutboundFragmented::new(&msg2, 6, ChannelId::ReliableOrdered, true);
        let g0 = FragmentHeader {
            fragment_id: 6,
            index: 0,
            count: 2,
        };
        let g1 = FragmentHeader {
            fragment_id: 6,
            index: 1,
            count: 2,
        };
        assert!(r
            .accept(ChannelId::ReliableOrdered, g0, of2.slice(0), now())
            .is_none());
        let done = r
            .accept(ChannelId::ReliableOrdered, g1, of2.slice(1), now())
            .expect("newer message delivers");
        assert_eq!(r.bytes(done.slot), &msg2[..]);
    }

    #[test]
    fn oversized_fragment_body_rejected() {
        let mut r = Reassembler::new(now());
        let h = FragmentHeader {
            fragment_id: 1,
            index: 0,
            count: 1,
        };
        let big = vec![0u8; FRAGMENT_PAYLOAD + 1];
        assert!(r.accept(ChannelId::Unreliable, h, &big, now()).is_none());
    }

    #[test]
    fn ack_tracking_completes() {
        let msg = vec![2u8; FRAGMENT_PAYLOAD * 3];
        let mut of = OutboundFragmented::new(&msg, 1, ChannelId::ReliableOrdered, true);
        assert!(!of.all_acked());
        for i in 0..of.count() {
            assert!(of.due(i, now(), Duration::from_millis(100)));
            of.mark_sent(i, now());
            of.mark_acked(i);
        }
        assert!(of.all_acked());
    }
}
