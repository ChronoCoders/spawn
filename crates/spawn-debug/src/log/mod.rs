//! Engine-owned logging: levels, records, sinks, and a once-only global logger.
//!
//! Conventions: timestamps are monotonic `Duration` since `Logger::init`, never
//! wall-clock. Logging is callable from any thread; sinks are `Send + Sync` and
//! serialize concurrent writes internally. Logging is infallible at call sites:
//! sink write failures are counted (`dropped_records`), never propagated.

pub mod macros;
pub mod sinks;

use crate::error::{DebugError, DebugResult};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

pub use sinks::{FileSink, FileSinkConfig, OwnedRecord, RingBufferSink, StderrSink};

/// Severity level. Ordering is `Error < Warn < Info < Debug < Trace`; a level is
/// enabled when `level <= active_max`.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LogLevel {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
    Trace = 4,
}

impl LogLevel {
    /// Uppercase name (`"ERROR"`..`"TRACE"`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
            Self::Trace => "TRACE",
        }
    }

    /// Parse case-insensitively; `None` if unrecognized.
    // Spec §1.1 mandates this exact inherent signature returning `Option<Self>`;
    // `FromStr` (Result-returning) is a different contract, so the lint is moot.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        if s.eq_ignore_ascii_case("error") {
            Some(Self::Error)
        } else if s.eq_ignore_ascii_case("warn") {
            Some(Self::Warn)
        } else if s.eq_ignore_ascii_case("info") {
            Some(Self::Info)
        } else if s.eq_ignore_ascii_case("debug") {
            Some(Self::Debug)
        } else if s.eq_ignore_ascii_case("trace") {
            Some(Self::Trace)
        } else {
            None
        }
    }

    const fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Error,
            1 => Self::Warn,
            2 => Self::Info,
            3 => Self::Debug,
            _ => Self::Trace,
        }
    }
}

/// Highest level retained by the compile-time floor. Macros for levels above this
/// expand to nothing, so stripped levels cost zero in the final binary. Exactly
/// one `max_level_*` feature resolves this floor.
pub const COMPILE_MAX_LEVEL: LogLevel = compile_max_level();

const fn compile_max_level() -> LogLevel {
    // Highest-verbosity selected feature wins. Exactly one arm compiles; the
    // `max_level_off` (no-feature) arm maps to `Error` and `COMPILE_OFF` gates
    // every macro to nothing regardless.
    #[cfg(feature = "max_level_trace")]
    let level = LogLevel::Trace;
    #[cfg(all(feature = "max_level_debug", not(feature = "max_level_trace")))]
    let level = LogLevel::Debug;
    #[cfg(all(
        feature = "max_level_info",
        not(feature = "max_level_trace"),
        not(feature = "max_level_debug")
    ))]
    let level = LogLevel::Info;
    #[cfg(all(
        feature = "max_level_warn",
        not(feature = "max_level_trace"),
        not(feature = "max_level_debug"),
        not(feature = "max_level_info")
    ))]
    let level = LogLevel::Warn;
    #[cfg(all(
        not(feature = "max_level_trace"),
        not(feature = "max_level_debug"),
        not(feature = "max_level_info"),
        not(feature = "max_level_warn")
    ))]
    let level = LogLevel::Error;
    level
}

/// True when the compile-time floor is `off` (every macro stripped). Macro
/// plumbing: read by the expansion of the level macros, not a stable contract.
#[doc(hidden)]
pub const COMPILE_OFF: bool = compile_off();

const fn compile_off() -> bool {
    #[cfg(all(
        not(feature = "max_level_trace"),
        not(feature = "max_level_debug"),
        not(feature = "max_level_info"),
        not(feature = "max_level_warn"),
        not(feature = "max_level_error")
    ))]
    let off = true;
    #[cfg(any(
        feature = "max_level_trace",
        feature = "max_level_debug",
        feature = "max_level_info",
        feature = "max_level_warn",
        feature = "max_level_error"
    ))]
    let off = false;
    off
}

// Coherent single floor: at least one feature must be present (default provides
// `max_level_trace`). `max_level_off` is the no-feature state and is valid.
const _: () = {
    assert!(COMPILE_MAX_LEVEL as u8 <= LogLevel::Trace as u8);
};

/// A stable per-thread id captured at log time, assigned on a thread's first log
/// via a process-global atomic counter. Cheaper to compare/store than the OS id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ThreadTag(pub u64);

static NEXT_THREAD_TAG: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static THREAD_TAG: ThreadTag = ThreadTag(NEXT_THREAD_TAG.fetch_add(1, Ordering::Relaxed));
}

/// The calling thread's `ThreadTag` (assigned on first call from that thread).
/// Macro plumbing: invoked by the level macros when building a `LogRecord`.
#[doc(hidden)]
pub fn thread_tag() -> ThreadTag {
    THREAD_TAG.with(|t| *t)
}

/// A single log event. Non-`'static` by design: `message` borrows `format_args!`
/// state so sinks format on demand. Sinks that retain records materialize an
/// owned [`OwnedRecord`].
pub struct LogRecord<'a> {
    pub level: LogLevel,
    pub target: &'static str,
    pub message: core::fmt::Arguments<'a>,
    pub timestamp: core::time::Duration,
    pub thread: ThreadTag,
}

/// A destination for log records. `write` must not panic; failures are recorded
/// internally (per-sink dropped counter), never propagated.
pub trait LogSink: Send + Sync {
    fn write(&self, record: &LogRecord<'_>);
    fn flush(&self) {}
}

/// Configuration for the global logger.
pub struct LogConfig {
    pub max_level: LogLevel,
    pub sinks: Vec<Box<dyn LogSink>>,
    pub target_filters: Vec<(&'static str, LogLevel)>,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            max_level: LogLevel::Info,
            sinks: vec![Box::new(StderrSink::new())],
            target_filters: Vec::new(),
        }
    }
}

struct LoggerState {
    start: Instant,
    sinks: Vec<Box<dyn LogSink>>,
    target_filters: Vec<(&'static str, LogLevel)>,
    dropped: AtomicU64,
}

/// The process-global logger. Installed once via [`Logger::init`].
pub struct Logger;

static MAX_LEVEL: AtomicU8 = AtomicU8::new(LogLevel::Info as u8);
static STATE: OnceLock<LoggerState> = OnceLock::new();

impl Logger {
    /// Install the process-global logger and start the monotonic clock.
    ///
    /// Returns `Err(DebugError::AlreadyInitialized)` on a second call.
    pub fn init(config: LogConfig) -> DebugResult<()> {
        let state = LoggerState {
            start: Instant::now(),
            sinks: config.sinks,
            target_filters: config.target_filters,
            dropped: AtomicU64::new(0),
        };
        STATE
            .set(state)
            .map_err(|_| DebugError::AlreadyInitialized)?;
        MAX_LEVEL.store(config.max_level as u8, Ordering::Release);
        Ok(())
    }

    /// Whether the global logger has been installed.
    pub fn is_initialized() -> bool {
        STATE.get().is_some()
    }

    /// Change the runtime floor. Atomic; the hot-path read is lock-free.
    pub fn set_max_level(level: LogLevel) {
        MAX_LEVEL.store(level as u8, Ordering::Release);
    }

    /// The current runtime floor (global; per-record target filters refine it).
    pub fn max_level() -> LogLevel {
        LogLevel::from_u8(MAX_LEVEL.load(Ordering::Acquire))
    }

    /// Monotonic time since init; `Duration::ZERO` before init.
    pub fn elapsed() -> core::time::Duration {
        STATE
            .get()
            .map(|s| s.start.elapsed())
            .unwrap_or(core::time::Duration::ZERO)
    }

    /// Flush all installed sinks (e.g. before shutdown).
    pub fn flush() {
        if let Some(s) = STATE.get() {
            for sink in &s.sinks {
                sink.flush();
            }
        }
    }

    /// Total records that sinks failed to write.
    pub fn dropped_records() -> u64 {
        STATE
            .get()
            .map(|s| s.dropped.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Hot-path enabled check: one atomic global load + branch, with a rare
    /// linear prefix scan of target filters (longest prefix wins) only when
    /// filters are registered. Allocation-free.
    pub fn enabled(level: LogLevel, target: &str) -> bool {
        let global = MAX_LEVEL.load(Ordering::Acquire);
        let floor = match STATE.get() {
            Some(s) if !s.target_filters.is_empty() => {
                resolve_floor(global, &s.target_filters, target)
            }
            _ => global,
        };
        (level as u8) <= floor
    }

    /// Dispatch a constructed record to all sinks. Called by the log macros only
    /// after [`enabled`](Logger::enabled) returned true. Failures are counted.
    /// Macro plumbing: not a stable hand-call API.
    #[doc(hidden)]
    pub fn dispatch(record: &LogRecord<'_>) {
        if let Some(s) = STATE.get() {
            for sink in &s.sinks {
                sink.write(record);
            }
        }
    }
}

fn resolve_floor(global: u8, filters: &[(&'static str, LogLevel)], target: &str) -> u8 {
    let mut best_len = 0usize;
    let mut best = global;
    let mut found = false;
    for (prefix, level) in filters {
        if target.starts_with(prefix) && (!found || prefix.len() > best_len) {
            best_len = prefix.len();
            best = *level as u8;
            found = true;
        }
    }
    best
}

/// Internal hook letting sinks report a dropped write into the global counter.
pub(crate) fn report_dropped() {
    if let Some(s) = STATE.get() {
        s.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

/// A `Mutex`-poison-tolerant lock acquisition: on poison, recover the inner guard
/// rather than panicking, since a poisoned logging mutex must not crash callers.
pub(crate) fn lock_recover<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_ordering_and_as_str() {
        assert!(LogLevel::Error < LogLevel::Warn);
        assert!(LogLevel::Warn < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Debug);
        assert!(LogLevel::Debug < LogLevel::Trace);
        assert_eq!(LogLevel::Error.as_str(), "ERROR");
        assert_eq!(LogLevel::Trace.as_str(), "TRACE");
    }

    #[test]
    fn from_str_case_insensitive() {
        assert_eq!(LogLevel::from_str("error"), Some(LogLevel::Error));
        assert_eq!(LogLevel::from_str("WARN"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::from_str("InFo"), Some(LogLevel::Info));
        assert_eq!(LogLevel::from_str("nope"), None);
    }

    #[test]
    fn resolve_floor_longest_prefix_wins() {
        let filters = [("render", LogLevel::Warn), ("render.mesh", LogLevel::Trace)];
        let g = LogLevel::Info as u8;
        assert_eq!(
            resolve_floor(g, &filters, "render.mesh.upload"),
            LogLevel::Trace as u8
        );
        assert_eq!(
            resolve_floor(g, &filters, "render.light"),
            LogLevel::Warn as u8
        );
        assert_eq!(resolve_floor(g, &filters, "audio"), g);
    }

    #[test]
    fn default_config_has_stderr_sink() {
        let c = LogConfig::default();
        assert_eq!(c.max_level, LogLevel::Info);
        assert_eq!(c.sinks.len(), 1);
        assert!(c.target_filters.is_empty());
    }

    #[test]
    fn thread_tags_distinct_per_thread() {
        let a = thread_tag();
        let b = std::thread::spawn(thread_tag).join().expect("join");
        assert_ne!(a, b);
        assert_eq!(a, thread_tag());
    }
}
