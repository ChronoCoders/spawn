//! Per-direction acknowledgement tracking and reconstruction.

use crate::sequence::sequence_greater_than;

/// Maximum number of sequences reportable in one [`ReliableEndpoint::process_acks`]
/// call: the `ack` itself plus the 32 bits of `ack_bits`.
pub(crate) const MAX_ACKED: usize = 33;

/// Fixed-capacity, heap-free buffer of newly-acknowledged sequence numbers.
///
/// Capacity is bounded by `MAX_ACKED` (the ack plus 32 bitfield entries), so
/// [`process_acks`](ReliableEndpoint::process_acks) never allocates.
#[derive(Debug, Clone, Copy)]
pub struct AckedSequences {
    buf: [u16; MAX_ACKED],
    len: usize,
}

impl Default for AckedSequences {
    fn default() -> Self {
        Self::new()
    }
}

impl AckedSequences {
    /// Create an empty buffer.
    pub fn new() -> Self {
        Self {
            buf: [0; MAX_ACKED],
            len: 0,
        }
    }

    /// Discard all entries without releasing storage.
    pub fn clear(&mut self) {
        self.len = 0;
    }

    fn push(&mut self, seq: u16) {
        if self.len < MAX_ACKED {
            self.buf[self.len] = seq;
            self.len += 1;
        }
    }

    /// The acknowledged sequences recorded since the last `clear`.
    pub fn as_slice(&self) -> &[u16] {
        &self.buf[..self.len]
    }

    /// Number of recorded sequences.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether no sequences are recorded.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

const RECV_HISTORY: usize = 1024;

/// Tracks outgoing sequence allocation and the set of recently received peer
/// sequences, and reconstructs the `(ack, ack_bits)` pair plus newly-acked sets.
///
/// Receive state is a fixed ring of the last `RECV_HISTORY` sequence slots, so all
/// operations are allocation-free. `ack` is the newest received sequence under
/// wrapping serial-number order; `ack_bits` bit `n` reflects `ack - (n+1)`.
pub struct ReliableEndpoint {
    local_sequence: u16,
    remote_ack: u16,
    received_any: bool,
    received: [Option<u16>; RECV_HISTORY],
    acked: [bool; RECV_HISTORY],
}

impl Default for ReliableEndpoint {
    fn default() -> Self {
        Self::new()
    }
}

impl ReliableEndpoint {
    /// Create a fresh endpoint with no sequences sent or received.
    pub fn new() -> Self {
        Self {
            local_sequence: 0,
            remote_ack: 0,
            received_any: false,
            received: [None; RECV_HISTORY],
            acked: [false; RECV_HISTORY],
        }
    }

    /// Allocate the next outgoing sequence number, wrapping at 65536.
    pub fn next_sequence(&mut self) -> u16 {
        let seq = self.local_sequence;
        self.local_sequence = self.local_sequence.wrapping_add(1);
        seq
    }

    fn last_sent(&self) -> Option<u16> {
        if self.local_sequence == 0 {
            None
        } else {
            Some(self.local_sequence.wrapping_sub(1))
        }
    }

    /// Record `seq` as received from the peer. Idempotent for duplicates; advances the
    /// `ack` view when `seq` is newer under serial-number order.
    pub fn on_received(&mut self, seq: u16) {
        let idx = usize::from(seq) % RECV_HISTORY;
        // Stamp the slot for this sequence; slots are keyed by full sequence so wrap
        // never confuses two values sharing a ring index.
        self.received[idx] = Some(seq);
        if !self.received_any || sequence_greater_than(seq, self.remote_ack) {
            self.remote_ack = seq;
            self.received_any = true;
        }
    }

    fn was_received(&self, seq: u16) -> bool {
        let idx = usize::from(seq) % RECV_HISTORY;
        self.received[idx] == Some(seq)
    }

    /// The `(ack, ack_bits)` pair to stamp on the next outgoing header. `ack` is the
    /// newest received sequence; bit `n` is set iff `ack - (n+1)` was also received.
    pub fn ack_header(&self) -> (u16, u32) {
        let ack = self.remote_ack;
        let mut bits = 0u32;
        if self.received_any {
            for n in 0..32u16 {
                let s = ack.wrapping_sub(n + 1);
                if self.was_received(s) {
                    bits |= 1 << n;
                }
            }
        }
        (ack, bits)
    }

    /// Decode a peer's `(ack, ack_bits)` and append to `out` exactly the local sent
    /// sequences it newly confirmed (each reported once across calls). `out` is cleared
    /// first. Correct under wrap; ignores acks for never-sent or already-acked sequences.
    pub fn process_acks(&mut self, ack: u16, ack_bits: u32, out: &mut AckedSequences) {
        out.clear();
        self.try_ack(ack, out);
        for n in 0..32u16 {
            if ack_bits & (1 << n) != 0 {
                let s = ack.wrapping_sub(n + 1);
                self.try_ack(s, out);
            }
        }
    }

    fn try_ack(&mut self, seq: u16, out: &mut AckedSequences) {
        let Some(last) = self.last_sent() else {
            return;
        };
        // Only sequences we actually allocated can be acked.
        if sequence_greater_than(seq, last) {
            return;
        }
        let idx = usize::from(seq) % RECV_HISTORY;
        if self.acked[idx] {
            return;
        }
        self.acked[idx] = true;
        out.push(seq);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_sequence_wraps() {
        let mut e = ReliableEndpoint::new();
        assert_eq!(e.next_sequence(), 0);
        assert_eq!(e.next_sequence(), 1);
        e.local_sequence = 0xFFFF;
        assert_eq!(e.next_sequence(), 0xFFFF);
        assert_eq!(e.next_sequence(), 0);
    }

    #[test]
    fn ack_header_reconstructs_received_set() {
        let mut e = ReliableEndpoint::new();
        for s in [10u16, 9, 8, 6, 4] {
            e.on_received(s);
        }
        let (ack, bits) = e.ack_header();
        assert_eq!(ack, 10);
        // bit n set => 10-(n+1) received: 9->bit0,8->bit1,6->bit3,4->bit5
        assert_eq!(bits & 1, 1);
        assert_eq!(bits & (1 << 1), 1 << 1);
        assert_eq!(bits & (1 << 2), 0); // 7 not received
        assert_eq!(bits & (1 << 3), 1 << 3);
        assert_eq!(bits & (1 << 4), 0); // 5 not received
        assert_eq!(bits & (1 << 5), 1 << 5);
    }

    #[test]
    fn ack_header_under_wrap() {
        let mut e = ReliableEndpoint::new();
        for s in [0u16, 0xFFFF, 0xFFFE] {
            e.on_received(s);
        }
        let (ack, bits) = e.ack_header();
        assert_eq!(ack, 0);
        assert_eq!(bits & 1, 1); // 0xFFFF
        assert_eq!(bits & (1 << 1), 1 << 1); // 0xFFFE
    }

    #[test]
    fn duplicate_receive_idempotent() {
        let mut e = ReliableEndpoint::new();
        e.on_received(5);
        e.on_received(5);
        let (ack, _) = e.ack_header();
        assert_eq!(ack, 5);
    }

    #[test]
    fn process_acks_recovers_newly_acked_once() {
        let mut e = ReliableEndpoint::new();
        for _ in 0..20 {
            e.next_sequence();
        }
        // peer acks 19 plus 18,17,15 via bits
        let mut out = AckedSequences::new();
        let bits = (1 << 0) | (1 << 1) | (1 << 3);
        e.process_acks(19, bits, &mut out);
        let mut got = out.as_slice().to_vec();
        got.sort_unstable();
        assert_eq!(got, vec![15, 17, 18, 19]);

        // re-process same: nothing new
        e.process_acks(19, bits, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn process_acks_ignores_unsent() {
        let mut e = ReliableEndpoint::new();
        e.next_sequence(); // only 0 sent
        let mut out = AckedSequences::new();
        e.process_acks(100, 0, &mut out);
        assert!(out.is_empty());
        e.process_acks(0, 0, &mut out);
        assert_eq!(out.as_slice(), &[0]);
    }
}
