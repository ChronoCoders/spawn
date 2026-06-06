//! Global-logger integration tests. The process-global `Logger` is once-only, so
//! all init-dependent assertions live in a single test function to stay
//! deterministic under test parallelism (each integration test binary has its
//! own process, but within it only one `init` may succeed).

use spawn_debug::log::{LogLevel, LogRecord, LogSink, Logger};
use spawn_debug::{spawn_debug, spawn_error, spawn_info, spawn_trace, spawn_warn, LogConfig};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Default)]
struct CountingSink {
    total: AtomicUsize,
    saw_debug: AtomicUsize,
    saw_trace: AtomicUsize,
    saw_target_render: AtomicUsize,
}

impl LogSink for CountingSink {
    fn write(&self, record: &LogRecord<'_>) {
        self.total.fetch_add(1, Ordering::Relaxed);
        match record.level {
            LogLevel::Debug => {
                self.saw_debug.fetch_add(1, Ordering::Relaxed);
            }
            LogLevel::Trace => {
                self.saw_trace.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
        if record.target == "render" {
            self.saw_target_render.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[test]
fn global_logger_lifecycle() {
    let sink = Arc::new(CountingSink::default());

    struct Forward(Arc<CountingSink>);
    impl LogSink for Forward {
        fn write(&self, record: &LogRecord<'_>) {
            self.0.write(record);
        }
    }

    assert!(!Logger::is_initialized());
    assert_eq!(Logger::elapsed(), core::time::Duration::ZERO);

    let config = LogConfig {
        max_level: LogLevel::Info,
        sinks: vec![Box::new(Forward(Arc::clone(&sink)))],
        target_filters: vec![("render", LogLevel::Trace)],
    };
    Logger::init(config).expect("first init succeeds");
    assert!(Logger::is_initialized());

    // Second init returns AlreadyInitialized.
    let again = Logger::init(LogConfig::default());
    assert!(matches!(
        again,
        Err(spawn_debug::DebugError::AlreadyInitialized)
    ));

    // At Info floor: error/warn/info pass; debug/trace dropped (default target).
    // Each level also requires surviving the compile-time floor feature.
    use spawn_debug::log::{COMPILE_MAX_LEVEL, COMPILE_OFF};
    let compiled = |level: LogLevel| !COMPILE_OFF && level as u8 <= COMPILE_MAX_LEVEL as u8;

    spawn_error!("e {}", 1);
    spawn_warn!("w");
    spawn_info!("i");
    spawn_debug!("d should be filtered at runtime");
    spawn_trace!("t should be filtered at runtime");

    let expected_total = [LogLevel::Error, LogLevel::Warn, LogLevel::Info]
        .into_iter()
        .filter(|&l| compiled(l))
        .count();
    assert_eq!(sink.saw_debug.load(Ordering::Relaxed), 0);
    assert_eq!(sink.saw_trace.load(Ordering::Relaxed), 0);
    assert_eq!(sink.total.load(Ordering::Relaxed), expected_total);

    // Per-target filter override: "render" floor is Trace, so trace passes —
    // but only when Trace is not stripped by the compile-time floor.
    let trace_compiled = compiled(LogLevel::Trace);
    spawn_trace!(target: "render", "render trace");
    let expected_trace = trace_compiled as usize;
    assert_eq!(
        sink.saw_target_render.load(Ordering::Relaxed),
        expected_trace
    );
    assert_eq!(sink.saw_trace.load(Ordering::Relaxed), expected_trace);

    // set_max_level takes effect: lower to Error, only error passes now.
    let before = sink.total.load(Ordering::Relaxed);
    Logger::set_max_level(LogLevel::Error);
    assert_eq!(Logger::max_level(), LogLevel::Error);
    spawn_warn!("w2 dropped");
    spawn_error!("e2 kept");
    let expected_kept = compiled(LogLevel::Error) as usize;
    assert_eq!(sink.total.load(Ordering::Relaxed), before + expected_kept);

    assert!(Logger::elapsed() > core::time::Duration::ZERO);
    Logger::flush();
}
