//! Rolling per-scope statistics over a fixed sample window.

/// Fixed-size sample ring of `stats_window` durations with avg/min/max/p99.
/// On overflow the oldest sample is evicted. No per-`record` allocation: the
/// percentile uses a bounded scratch buffer owned by the stats object.
pub struct RollingStats {
    samples: Vec<core::time::Duration>,
    scratch: core::cell::RefCell<Vec<core::time::Duration>>,
    head: usize,
    len: usize,
    window: usize,
}

impl RollingStats {
    pub(crate) fn new(window: usize) -> Self {
        let window = window.max(1);
        Self {
            samples: Vec::with_capacity(window),
            scratch: core::cell::RefCell::new(Vec::with_capacity(window)),
            head: 0,
            len: 0,
            window,
        }
    }

    /// Push one frame's value for a scope, evicting the oldest on overflow.
    pub fn record(&mut self, sample: core::time::Duration) {
        if self.len < self.window {
            self.samples.push(sample);
            self.len += 1;
        } else {
            self.samples[self.head] = sample;
            self.head = (self.head + 1) % self.window;
        }
    }

    /// Mean over the current window; `ZERO` when empty.
    pub fn avg(&self) -> core::time::Duration {
        if self.len == 0 {
            return core::time::Duration::ZERO;
        }
        let sum: u128 = self.samples.iter().map(|d| d.as_nanos()).sum();
        let nanos = sum / self.len as u128;
        core::time::Duration::from_nanos(nanos as u64)
    }

    /// Minimum over the current window; `ZERO` when empty.
    pub fn min(&self) -> core::time::Duration {
        self.samples
            .iter()
            .copied()
            .min()
            .unwrap_or(core::time::Duration::ZERO)
    }

    /// Maximum over the current window; `ZERO` when empty.
    pub fn max(&self) -> core::time::Duration {
        self.samples
            .iter()
            .copied()
            .max()
            .unwrap_or(core::time::Duration::ZERO)
    }

    /// 99th percentile (nearest-rank) over the current window; `ZERO` when empty.
    pub fn p99(&self) -> core::time::Duration {
        if self.len == 0 {
            return core::time::Duration::ZERO;
        }
        // Bounded owned scratch (capacity == window); sorted copy of samples.
        let mut scratch = self.scratch.borrow_mut();
        scratch.clear();
        scratch.extend(self.samples.iter().copied());
        scratch.sort_unstable();
        // Nearest-rank: ceil(p/100 * n), 1-based.
        let rank = (99 * self.len).div_ceil(100);
        let idx = rank.saturating_sub(1).min(self.len - 1);
        scratch[idx]
    }

    /// Number of samples currently in the window.
    pub fn count(&self) -> usize {
        self.len
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::time::Duration;

    #[test]
    fn avg_min_max_known_window() {
        let mut s = RollingStats::new(8);
        for ms in [10u64, 20, 30, 40] {
            s.record(Duration::from_millis(ms));
        }
        assert_eq!(s.count(), 4);
        assert_eq!(s.min(), Duration::from_millis(10));
        assert_eq!(s.max(), Duration::from_millis(40));
        assert_eq!(s.avg(), Duration::from_millis(25));
    }

    #[test]
    fn window_eviction() {
        let mut s = RollingStats::new(3);
        for ms in [10u64, 20, 30, 40, 50] {
            s.record(Duration::from_millis(ms));
        }
        // Window holds last 3: 30,40,50.
        assert_eq!(s.count(), 3);
        assert_eq!(s.min(), Duration::from_millis(30));
        assert_eq!(s.max(), Duration::from_millis(50));
        assert_eq!(s.avg(), Duration::from_millis(40));
    }

    #[test]
    fn p99_nearest_rank() {
        let mut s = RollingStats::new(100);
        for i in 1..=100u64 {
            s.record(Duration::from_millis(i));
        }
        // nearest-rank ceil(0.99*100)=99 => 1-based 99th value = 99ms.
        assert_eq!(s.p99(), Duration::from_millis(99));
    }

    #[test]
    fn empty_is_zero() {
        let s = RollingStats::new(4);
        assert_eq!(s.avg(), Duration::ZERO);
        assert_eq!(s.min(), Duration::ZERO);
        assert_eq!(s.max(), Duration::ZERO);
        assert_eq!(s.p99(), Duration::ZERO);
        assert_eq!(s.count(), 0);
    }
}
