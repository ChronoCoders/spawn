//! Startup observability: the engine's default-logger install and the two
//! one-time records (GPU adapter, audio backend) emitted during assembly.
//!
//! Logging here is idempotent and infallible by contract: an app-owned logger is
//! never clobbered, and an install failure is dropped so logging can never fail a
//! frame. Message formatting is factored into pure helpers so the record contents
//! are unit-testable without touching the process-global logger.

use spawn_audio::BackendKind;
use spawn_debug::{LogConfig, Logger};
use spawn_render::AdapterInfo;

/// `spawn-debug` target for the GPU adapter startup line.
const ADAPTER_TARGET: &str = "spawn_engine::render";
/// `spawn-debug` target for the audio backend startup line.
const AUDIO_TARGET: &str = "spawn_engine::audio";

/// Installs the engine's default logger (a stderr sink at `Info`) unless a logger
/// is already initialized. A pre-existing app logger is kept, and any install
/// error is dropped so logging never fails a frame.
pub(crate) fn install_default_logging() {
    if Logger::is_initialized() {
        return;
    }
    let _ = Logger::init(LogConfig::default());
}

/// The GPU adapter line: the selected adapter's name and backend, or
/// `none (headless)` when the backend has no GPU.
fn adapter_line(adapter: Option<&AdapterInfo>) -> String {
    match adapter {
        Some(info) => format!("GPU adapter: {} ({})", info.name, info.backend),
        None => "GPU adapter: none (headless)".to_string(),
    }
}

/// The audio backend line: `Device` on hardware, `Null` when the engine fell back
/// to the silent backend.
fn audio_line(kind: BackendKind) -> String {
    format!("audio backend: {kind:?}")
}

/// Emits the two one-time startup records, adapter first then audio, through the
/// global logger. Called once during assembly after the default logger is
/// installed.
pub(crate) fn log_startup(adapter: Option<AdapterInfo>, audio: BackendKind) {
    spawn_debug::spawn_info!(target: ADAPTER_TARGET, "{}", adapter_line(adapter.as_ref()));
    spawn_debug::spawn_info!(target: AUDIO_TARGET, "{}", audio_line(audio));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_line_reads_name_and_backend() {
        let info = AdapterInfo {
            name: "NVIDIA A16".to_string(),
            backend: "Vulkan",
        };
        assert_eq!(
            adapter_line(Some(&info)),
            "GPU adapter: NVIDIA A16 (Vulkan)"
        );
    }

    #[test]
    fn adapter_line_headless_reads_none() {
        assert_eq!(adapter_line(None), "GPU adapter: none (headless)");
    }

    #[test]
    fn audio_line_reports_backend_kind() {
        assert_eq!(audio_line(BackendKind::Device), "audio backend: Device");
        assert_eq!(audio_line(BackendKind::Null), "audio backend: Null");
    }
}
