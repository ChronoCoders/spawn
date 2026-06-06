#![deny(warnings)]

//! Engine-owned debugging facilities for the Spawn engine: logging (levels,
//! global logger, macros, compile-time level floor, sinks, per-target filters),
//! a main-thread frame profiler (scoped RAII timers, hierarchical scope tree,
//! frame reports, rolling stats, history ring), atomic metrics, and the overlay
//! data model. `std`-only; no `log`/`tracing`. See `docs/specs/phase-01`.

pub mod error;
pub mod log;
pub mod metrics;
pub mod overlay;
pub mod profile;

pub use error::{DebugError, DebugResult};
pub use log::sinks::{FileSink, FileSinkConfig, OwnedRecord, RingBufferSink, StderrSink};
pub use log::{LogConfig, LogLevel, LogRecord, LogSink, Logger, ThreadTag};
pub use metrics::{global, Counter, Gauge, MetricKind, MetricSnapshot, MetricsRegistry};
pub use overlay::{DebugOverlayData, OverlayConfig, ScopeStat};
pub use profile::{FrameReport, Profiler, ProfilerConfig, RollingStats, ScopeGuard, ScopeNode};
