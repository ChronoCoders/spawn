//! Overlay data model, data only, no rendering. Assembled on demand from the
//! profiler, a `RingBufferSink`, and a `MetricsRegistry`.

use crate::log::sinks::{OwnedRecord, RingBufferSink};
use crate::metrics::{MetricSnapshot, MetricsRegistry};
use crate::profile::Profiler;

/// One scope's last/avg/p99 timings for the overlay table.
pub struct ScopeStat {
    pub name: &'static str,
    pub last: core::time::Duration,
    pub avg: core::time::Duration,
    pub p99: core::time::Duration,
}

/// Tuning for [`DebugOverlayData::assemble`].
pub struct OverlayConfig {
    pub graph_len: usize,
    pub top_k_scopes: usize,
    pub log_tail_len: usize,
    pub metric_filter: Vec<&'static str>,
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            graph_len: 120,
            top_k_scopes: 10,
            log_tail_len: 32,
            metric_filter: Vec::new(),
        }
    }
}

/// Snapshot of everything the overlay draws for one frame.
pub struct DebugOverlayData {
    pub fps: f32,
    pub frame_time_ms: f32,
    pub frame_graph: Vec<f32>,
    pub hottest_scopes: Vec<ScopeStat>,
    pub metrics: Vec<MetricSnapshot>,
    pub log_tail: Vec<OwnedRecord>,
}

impl DebugOverlayData {
    /// Assemble overlay data from live sources. This is the one allocating call
    /// in the overlay path and runs at most once per frame, not a steady-state
    /// engine hot path.
    pub fn assemble(
        profiler: &Profiler,
        logs: &RingBufferSink,
        metrics: &MetricsRegistry,
        config: &OverlayConfig,
    ) -> Self {
        let (fps, frame_time_ms) = match profiler.last_report() {
            Some(r) => (r.fps(), r.total.as_secs_f32() * 1000.0),
            None => (0.0, 0.0),
        };

        let history = profiler.history();
        let start = history.len().saturating_sub(config.graph_len);
        let frame_graph: Vec<f32> = history[start..]
            .iter()
            .map(|r| r.total.as_secs_f32() * 1000.0)
            .collect();

        let hottest_scopes = match profiler.last_report() {
            Some(r) => r
                .hottest(config.top_k_scopes)
                .into_iter()
                .map(|(name, last)| {
                    let (avg, p99) = profiler
                        .scope_stats(name)
                        .map(|s| (s.avg(), s.p99()))
                        .unwrap_or((core::time::Duration::ZERO, core::time::Duration::ZERO));
                    ScopeStat {
                        name,
                        last,
                        avg,
                        p99,
                    }
                })
                .collect(),
            None => Vec::new(),
        };

        let metrics_all = metrics.snapshot();
        let metrics = if config.metric_filter.is_empty() {
            metrics_all
        } else {
            metrics_all
                .into_iter()
                .filter(|m| config.metric_filter.contains(&m.name))
                .collect()
        };

        let log_tail = logs.tail(config.log_tail_len);

        Self {
            fps,
            frame_time_ms,
            frame_graph,
            hottest_scopes,
            metrics,
            log_tail,
        }
    }

    /// Borrow the frame-time graph for direct plotting without a copy.
    pub fn frame_graph_slice(&self) -> &[f32] {
        &self.frame_graph
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{ProfilerConfig, ScopeGuard};

    fn build_profiler(frames: usize) -> Profiler {
        let mut p = Profiler::new(ProfilerConfig::default());
        for _ in 0..frames {
            p.begin_frame();
            {
                let _a = ScopeGuard::enter("alpha");
                {
                    let _b = ScopeGuard::enter("beta");
                }
            }
            p.end_frame();
        }
        p
    }

    #[test]
    fn assemble_populates_fields() {
        let p = build_profiler(5);
        let logs = RingBufferSink::with_capacity(8);
        let metrics = MetricsRegistry::new();
        metrics.counter("frames").add(5);
        metrics.gauge("entities").set(100);
        let cfg = OverlayConfig {
            graph_len: 3,
            top_k_scopes: 1,
            log_tail_len: 4,
            metric_filter: vec!["frames"],
        };
        let data = DebugOverlayData::assemble(&p, &logs, &metrics, &cfg);

        assert!(data.frame_graph.len() <= 3);
        assert_eq!(data.frame_graph.len(), 3);
        assert_eq!(data.hottest_scopes.len(), 1);
        // Metric filter keeps only "frames".
        assert_eq!(data.metrics.len(), 1);
        assert_eq!(data.metrics[0].name, "frames");
        assert_eq!(data.frame_graph_slice().len(), data.frame_graph.len());
        assert!(data.log_tail.is_empty());
    }

    #[test]
    fn assemble_empty_profiler() {
        let p = Profiler::new(ProfilerConfig::default());
        let logs = RingBufferSink::with_capacity(4);
        let metrics = MetricsRegistry::new();
        let cfg = OverlayConfig::default();
        let data = DebugOverlayData::assemble(&p, &logs, &metrics, &cfg);
        assert_eq!(data.fps, 0.0);
        assert_eq!(data.frame_time_ms, 0.0);
        assert!(data.frame_graph.is_empty());
        assert!(data.hottest_scopes.is_empty());
    }

    #[test]
    fn metric_filter_empty_returns_all() {
        let p = build_profiler(1);
        let logs = RingBufferSink::with_capacity(4);
        let metrics = MetricsRegistry::new();
        metrics.counter("a").increment();
        metrics.counter("b").increment();
        let cfg = OverlayConfig::default();
        let data = DebugOverlayData::assemble(&p, &logs, &metrics, &cfg);
        assert_eq!(data.metrics.len(), 2);
    }
}
