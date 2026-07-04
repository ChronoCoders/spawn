//! A headless assemble emits exactly two startup info records: the GPU adapter
//! line (`none (headless)` with no GPU) and the audio backend line (the selected
//! `BackendKind`). Kept in its own test binary so it owns the process-global
//! logger and no sibling test's engine build installs a logger first.

use std::sync::Arc;

use spawn_debug::{LogConfig, LogLevel, LogRecord, LogSink, Logger, RingBufferSink};
use spawn_engine::App;

/// Shares one ring buffer between the installed global logger and the assertions.
struct SharedRing(Arc<RingBufferSink>);

impl LogSink for SharedRing {
    fn write(&self, record: &LogRecord<'_>) {
        self.0.write(record);
    }
}

#[test]
fn headless_assemble_logs_adapter_and_audio() {
    let ring = Arc::new(RingBufferSink::with_capacity(16));
    Logger::init(LogConfig {
        max_level: LogLevel::Info,
        sinks: vec![Box::new(SharedRing(Arc::clone(&ring)))],
        target_filters: Vec::new(),
    })
    .expect("this test owns the process-global logger");

    // The engine's own `init_default_logging` must see the test logger and leave
    // it in place, so both startup records land in the ring.
    let _engine = App::new().build_headless().expect("headless build");

    let records = ring.snapshot();
    assert_eq!(records.len(), 2, "exactly two startup records");

    let adapter = &records[0];
    assert_eq!(adapter.level, LogLevel::Info);
    assert_eq!(adapter.target, "spawn_engine::render");
    assert_eq!(adapter.message, "GPU adapter: none (headless)");

    let audio = &records[1];
    assert_eq!(audio.level, LogLevel::Info);
    assert_eq!(audio.target, "spawn_engine::audio");
    assert!(
        audio.message == "audio backend: Device" || audio.message == "audio backend: Null",
        "audio line reports the selected backend kind, got {:?}",
        audio.message
    );
}
