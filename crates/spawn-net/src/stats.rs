//! Per-connection statistics: RTT, packet loss, byte/packet counters.

use std::time::Duration;

/// A snapshot of one connection's transport statistics.
///
/// `rtt` is a smoothed round-trip estimate; `packet_loss` is a `[0,1]` EWMA fraction;
/// byte counters include header bytes and count UDP payload only (not IP/UDP overhead).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConnectionStats {
    pub rtt: Duration,
    pub packet_loss: f32,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub packets_sent: u64,
    pub packets_received: u64,
}

const SMOOTHING: f32 = 0.1;

/// Mutable accumulator behind [`ConnectionStats`]. RTT and loss use EWMA with a `0.1`
/// smoothing factor; RTT starts at zero until the first ack sample arrives.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StatsTracker {
    rtt: Duration,
    have_rtt: bool,
    packet_loss: f32,
    bytes_sent: u64,
    bytes_received: u64,
    packets_sent: u64,
    packets_received: u64,
}

impl StatsTracker {
    pub fn new() -> Self {
        Self {
            rtt: Duration::ZERO,
            have_rtt: false,
            packet_loss: 0.0,
            bytes_sent: 0,
            bytes_received: 0,
            packets_sent: 0,
            packets_received: 0,
        }
    }

    pub fn on_sent(&mut self, bytes: usize) {
        self.bytes_sent += bytes as u64;
        self.packets_sent += 1;
    }

    pub fn on_received(&mut self, bytes: usize) {
        self.bytes_received += bytes as u64;
        self.packets_received += 1;
    }

    pub fn on_rtt_sample(&mut self, sample: Duration) {
        if !self.have_rtt {
            self.rtt = sample;
            self.have_rtt = true;
        } else {
            let cur = self.rtt.as_secs_f32();
            let next = cur + SMOOTHING * (sample.as_secs_f32() - cur);
            self.rtt = Duration::from_secs_f32(next.max(0.0));
        }
    }

    /// Feed one delivery outcome (`true` = acked, `false` = considered lost) into the
    /// packet-loss EWMA.
    pub fn on_delivery(&mut self, lost: bool) {
        let sample = if lost { 1.0 } else { 0.0 };
        self.packet_loss += SMOOTHING * (sample - self.packet_loss);
    }

    pub fn snapshot(&self) -> ConnectionStats {
        ConnectionStats {
            rtt: self.rtt,
            packet_loss: self.packet_loss.clamp(0.0, 1.0),
            bytes_sent: self.bytes_sent,
            bytes_received: self.bytes_received,
            packets_sent: self.packets_sent,
            packets_received: self.packets_received,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtt_starts_zero_then_ewma() {
        let mut s = StatsTracker::new();
        assert_eq!(s.snapshot().rtt, Duration::ZERO);
        s.on_rtt_sample(Duration::from_millis(100));
        assert_eq!(s.snapshot().rtt, Duration::from_millis(100));
        s.on_rtt_sample(Duration::from_millis(200));
        let rtt = s.snapshot().rtt.as_millis();
        assert!((105..=115).contains(&rtt), "got {rtt}");
    }

    #[test]
    fn loss_ewma_bounded() {
        let mut s = StatsTracker::new();
        for _ in 0..100 {
            s.on_delivery(true);
        }
        let loss = s.snapshot().packet_loss;
        assert!((0.0..=1.0).contains(&loss));
        assert!(loss > 0.9);
    }

    #[test]
    fn loss_ewma_tracks_mixed_outcomes() {
        // Known send/ack pattern: 1 of every 4 packets lost. The EWMA must converge to a
        // nonzero estimate near the true 0.25 loss fraction, never pinned at 0.
        let mut s = StatsTracker::new();
        for _ in 0..400 {
            s.on_delivery(false);
            s.on_delivery(false);
            s.on_delivery(false);
            s.on_delivery(true);
        }
        let loss = s.snapshot().packet_loss;
        assert!(loss > 0.0, "loss must be nonzero under drops, got {loss}");
        assert!((0.15..=0.35).contains(&loss), "got {loss}");
    }

    #[test]
    fn loss_decays_to_zero_when_all_delivered() {
        let mut s = StatsTracker::new();
        for _ in 0..50 {
            s.on_delivery(true);
        }
        for _ in 0..200 {
            s.on_delivery(false);
        }
        assert!(s.snapshot().packet_loss < 0.01);
    }

    #[test]
    fn counters_accumulate() {
        let mut s = StatsTracker::new();
        s.on_sent(50);
        s.on_sent(20);
        s.on_received(14);
        let snap = s.snapshot();
        assert_eq!(snap.bytes_sent, 70);
        assert_eq!(snap.packets_sent, 2);
        assert_eq!(snap.bytes_received, 14);
        assert_eq!(snap.packets_received, 1);
    }
}
