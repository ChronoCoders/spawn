//! Delivery channels and their per-connection buffers.
//!
//! Channel framing (inside the packet payload, after the 14-byte header):
//! - `Unreliable`: raw application bytes, no framing.
//! - `UnreliableSequenced`: `[seq: u16 LE][bytes...]`; receiver drops stale frames.
//! - `ReliableOrdered`: `[message_id: u16 LE][bytes...]`; resent until acked, delivered
//!   strictly in id order.

use std::time::{Duration, Instant};

use crate::error::{NetError, NetResult};
use crate::protocol::MAX_PAYLOAD_SIZE;
use crate::sequence::sequence_greater_than;

/// Channel discriminant carried at header offset 13.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ChannelId {
    Unreliable = 0,
    UnreliableSequenced = 1,
    ReliableOrdered = 2,
}

impl TryFrom<u8> for ChannelId {
    type Error = NetError;

    fn try_from(value: u8) -> NetResult<Self> {
        match value {
            0 => Ok(Self::Unreliable),
            1 => Ok(Self::UnreliableSequenced),
            2 => Ok(Self::ReliableOrdered),
            _ => Err(NetError::MalformedPacket),
        }
    }
}

/// Maximum unacked reliable messages buffered per connection before `send` returns
/// `ChannelFull` (backpressure, never silent drop).
pub const RELIABLE_SEND_WINDOW: usize = 256;
/// Capacity of the per-connection reliable reorder buffer.
pub const RELIABLE_RECV_WINDOW: usize = 256;
/// An unacked reliable message older than this is re-queued for transmission.
pub const RELIABLE_RESEND_TIMEOUT: Duration = Duration::from_millis(100);

/// Reliable message framing prefix length (`message_id: u16`).
pub(crate) const RELIABLE_PREFIX: usize = 2;
/// Sequenced framing prefix length (`seq: u16`).
pub(crate) const SEQUENCED_PREFIX: usize = 2;

/// Largest application payload acceptable on `ReliableOrdered` once framing overhead
/// is reserved.
pub(crate) const MAX_RELIABLE_PAYLOAD: usize = MAX_PAYLOAD_SIZE - RELIABLE_PREFIX;
/// Largest application payload acceptable on `UnreliableSequenced`.
pub(crate) const MAX_SEQUENCED_PAYLOAD: usize = MAX_PAYLOAD_SIZE - SEQUENCED_PREFIX;

struct PendingReliable {
    id: u16,
    len: usize,
    buf: [u8; MAX_PAYLOAD_SIZE],
    last_sent: Option<Instant>,
}

impl PendingReliable {
    fn empty() -> Self {
        Self {
            id: 0,
            len: 0,
            buf: [0u8; MAX_PAYLOAD_SIZE],
            last_sent: None,
        }
    }
}

/// Outgoing reliable state: a bounded ring of unacked messages plus the next id and a
/// preallocated staging area for the per-flush due set.
///
/// The window holds at most `RELIABLE_SEND_WINDOW` unacked messages. `queue` returns
/// `ChannelFull` rather than dropping when full; a slot frees only on ack.
pub(crate) struct ReliableSender {
    slots: Vec<PendingReliable>,
    occupied: Vec<bool>,
    staged: Vec<(u16, bool)>,
    next_id: u16,
    count: usize,
}

impl ReliableSender {
    pub fn new() -> Self {
        let mut slots = Vec::with_capacity(RELIABLE_SEND_WINDOW);
        let mut occupied = Vec::with_capacity(RELIABLE_SEND_WINDOW);
        for _ in 0..RELIABLE_SEND_WINDOW {
            slots.push(PendingReliable::empty());
            occupied.push(false);
        }
        Self {
            slots,
            occupied,
            staged: Vec::with_capacity(RELIABLE_SEND_WINDOW),
            next_id: 0,
            count: 0,
        }
    }

    fn slot_of(id: u16) -> usize {
        usize::from(id) % RELIABLE_SEND_WINDOW
    }

    /// Buffer `bytes` as a new reliable message. `Err(ChannelFull)` when the window is
    /// already at `RELIABLE_SEND_WINDOW` unacked messages — the caller must retry later.
    pub fn queue(&mut self, bytes: &[u8]) -> NetResult<u16> {
        if self.count >= RELIABLE_SEND_WINDOW {
            return Err(NetError::ChannelFull);
        }
        let id = self.next_id;
        let slot = Self::slot_of(id);
        if self.occupied[slot] {
            return Err(NetError::ChannelFull);
        }
        let entry = &mut self.slots[slot];
        entry.id = id;
        entry.len = bytes.len();
        entry.buf[..bytes.len()].copy_from_slice(bytes);
        entry.last_sent = None;
        self.occupied[slot] = true;
        self.count += 1;
        self.next_id = self.next_id.wrapping_add(1);
        Ok(id)
    }

    /// Number of currently unacked messages (test/diagnostics).
    #[cfg(test)]
    pub fn unacked(&self) -> usize {
        self.count
    }

    /// Mark the message `id` acked, freeing its slot. No-op for unknown/stale ids.
    pub fn ack(&mut self, id: u16) {
        let slot = Self::slot_of(id);
        if self.occupied[slot] && self.slots[slot].id == id {
            self.occupied[slot] = false;
            self.count -= 1;
        }
    }

    /// Stage every unacked message due at `now` (never sent, or sent before
    /// `now - RELIABLE_RESEND_TIMEOUT`), marking each as sent. Returns the staged count.
    /// Staging uses the preallocated `staged` buffer (capacity = window), so no per-flush
    /// allocation occurs in steady state. Read frames via `staged_frame`/`copy_staged_bytes`.
    pub fn take_due(&mut self, now: Instant) -> usize {
        self.staged.clear();
        for slot in 0..RELIABLE_SEND_WINDOW {
            if !self.occupied[slot] {
                continue;
            }
            let (due, retransmit) = match self.slots[slot].last_sent {
                None => (true, false),
                Some(t) => (now.duration_since(t) >= RELIABLE_RESEND_TIMEOUT, true),
            };
            if due {
                self.slots[slot].last_sent = Some(now);
                self.staged.push((self.slots[slot].id, retransmit));
            }
        }
        self.staged.len()
    }

    /// `(id, len)` of the `i`-th staged due frame from the most recent `take_due`.
    pub fn staged_frame(&self, i: usize) -> (u16, usize) {
        let (id, _) = self.staged[i];
        (id, self.slots[Self::slot_of(id)].len)
    }

    /// Whether the `i`-th staged frame is a retransmit (its previous send timed
    /// out unacked — the spec's definition of a lost packet for stats).
    pub fn staged_is_retransmit(&self, i: usize) -> bool {
        self.staged[i].1
    }

    /// Copy the bytes of the `i`-th staged frame into `dst` (length must match `len`).
    pub fn copy_staged_bytes(&self, i: usize, dst: &mut [u8]) {
        let (id, _) = self.staged[i];
        let slot = Self::slot_of(id);
        dst.copy_from_slice(&self.slots[slot].buf[..self.slots[slot].len]);
    }
}

/// Incoming reliable reordering. Buffers out-of-order messages until the contiguous
/// prefix starting at `next_deliver` is available, then yields them in id order.
///
/// A message whose id is `>= RELIABLE_RECV_WINDOW` ahead of `next_deliver` is dropped
/// as a protocol-violation guard; in-window duplicates are discarded (already acked).
pub(crate) struct ReliableReceiver {
    slots: Vec<Option<([u8; MAX_PAYLOAD_SIZE], usize)>>,
    next_deliver: u16,
}

impl ReliableReceiver {
    pub fn new() -> Self {
        let mut slots = Vec::with_capacity(RELIABLE_RECV_WINDOW);
        for _ in 0..RELIABLE_RECV_WINDOW {
            slots.push(None);
        }
        Self {
            slots,
            next_deliver: 0,
        }
    }

    fn slot_of(id: u16) -> usize {
        usize::from(id) % RELIABLE_RECV_WINDOW
    }

    /// Accept reliable message `id`. Returns `true` if it was newly stored (caller
    /// should still send an ack for it regardless of return value).
    pub fn accept(&mut self, id: u16, bytes: &[u8]) -> bool {
        let ahead = id.wrapping_sub(self.next_deliver);
        if usize::from(ahead) >= RELIABLE_RECV_WINDOW {
            // Either far-future (guard) or already delivered (behind, wraps to large).
            return false;
        }
        let slot = Self::slot_of(id);
        if self.slots[slot].is_some() {
            return false;
        }
        let mut buf = [0u8; MAX_PAYLOAD_SIZE];
        buf[..bytes.len()].copy_from_slice(bytes);
        self.slots[slot] = Some((buf, bytes.len()));
        true
    }

    /// Pop the next in-order deliverable message, if present.
    pub fn pop_next<R>(&mut self, mut take: R) -> bool
    where
        R: FnMut(&[u8]),
    {
        let slot = Self::slot_of(self.next_deliver);
        if let Some((buf, len)) = self.slots[slot].take() {
            take(&buf[..len]);
            self.next_deliver = self.next_deliver.wrapping_add(1);
            true
        } else {
            false
        }
    }
}

/// Latest-wins filter for `UnreliableSequenced`: drops any frame not newer than the
/// last delivered sequence under serial-number order.
pub(crate) struct SequencedReceiver {
    last: Option<u16>,
}

impl SequencedReceiver {
    pub fn new() -> Self {
        Self { last: None }
    }

    /// True iff `seq` should be delivered (it is newer than the last delivered frame).
    pub fn accept(&mut self, seq: u16) -> bool {
        match self.last {
            None => {
                self.last = Some(seq);
                true
            }
            Some(prev) if sequence_greater_than(seq, prev) => {
                self.last = Some(seq);
                true
            }
            _ => false,
        }
    }
}

/// Outgoing sequence counter for `UnreliableSequenced`.
pub(crate) struct SequencedSender {
    next: u16,
}

impl SequencedSender {
    pub fn new() -> Self {
        Self { next: 0 }
    }

    pub fn next_seq(&mut self) -> u16 {
        let s = self.next;
        self.next = self.next.wrapping_add(1);
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_id_try_from() {
        assert_eq!(ChannelId::try_from(2).unwrap(), ChannelId::ReliableOrdered);
        assert!(ChannelId::try_from(3).is_err());
    }

    #[test]
    fn reliable_sender_backpressure() {
        let mut s = ReliableSender::new();
        for _ in 0..RELIABLE_SEND_WINDOW {
            s.queue(b"x").unwrap();
        }
        assert!(matches!(s.queue(b"y"), Err(NetError::ChannelFull)));
        s.ack(0);
        assert_eq!(s.unacked(), RELIABLE_SEND_WINDOW - 1);
        assert!(s.queue(b"y").is_ok());
    }

    #[test]
    fn reliable_due_staging_and_resend() {
        let mut s = ReliableSender::new();
        let id = s.queue(b"abc").unwrap();
        let n = s.take_due(Instant::now());
        assert_eq!(n, 1);
        let (sid, len) = s.staged_frame(0);
        assert_eq!(sid, id);
        let mut buf = vec![0u8; len];
        s.copy_staged_bytes(0, &mut buf);
        assert_eq!(buf, b"abc");
        assert_eq!(s.take_due(Instant::now()), 0);
    }

    #[test]
    fn reliable_receiver_in_order() {
        let mut r = ReliableReceiver::new();
        assert!(r.accept(1, b"one"));
        assert!(r.accept(0, b"zero"));
        let mut out = Vec::new();
        while r.pop_next(|b| out.push(b.to_vec())) {}
        assert_eq!(out, vec![b"zero".to_vec(), b"one".to_vec()]);
    }

    #[test]
    fn reliable_receiver_dedup_and_guard() {
        let mut r = ReliableReceiver::new();
        assert!(r.accept(0, b"a"));
        assert!(!r.accept(0, b"a"));
        assert!(!r.accept(RELIABLE_RECV_WINDOW as u16, b"far")); // out of window
    }

    #[test]
    fn sequenced_latest_wins() {
        let mut r = SequencedReceiver::new();
        assert!(r.accept(5));
        assert!(!r.accept(4));
        assert!(r.accept(6));
        assert!(!r.accept(6));
    }
}
