//! Engine configuration.

use std::path::PathBuf;

use spawn_platform::WindowConfig;

use crate::frame::SyncMode;

/// Timestep, sync, and window configuration for an [`App`](crate::App).
///
/// `Default` yields a 60 Hz fixed step, an 8-step-per-frame spiral bound, a
/// 0.25 s frame-delta clamp, the low-latency [`SyncMode::Immediate`], and the
/// default window.
#[derive(Clone)]
pub struct EngineConfig {
    /// Seconds per fixed simulation tick. Must be `> 0`.
    pub fixed_timestep: f32,
    /// Maximum fixed ticks run in one frame; bounds catch-up under overload
    /// (spiral-of-death guard).
    pub max_fixed_steps_per_frame: u32,
    /// Real frame delta is clamped to this before accumulation (spiral guard).
    pub max_frame_delta: f32,
    /// How far the render backend may lag the frontend.
    pub sync_mode: SyncMode,
    /// Filesystem root the asset server resolves load paths against.
    pub asset_root: PathBuf,
    /// Whether the asset server watches `asset_root` and hot-reloads changed
    /// files. Defaults to `false`; the deterministic headless path keeps it off so
    /// runs stay reproducible. Enable it on the windowed (wall-clock) path.
    pub hot_reload: bool,
    /// Whether rendering runs on a dedicated render thread (the `ThreadedExecutor`)
    /// instead of inline on the frame thread. Defaults to `false` so headless runs
    /// stay single-threaded and reproducible; [`windowed`](EngineConfig::windowed)
    /// turns it on.
    pub render_thread: bool,
    /// Window configuration; ignored in headless mode.
    pub window: WindowConfig,
}

impl EngineConfig {
    /// The recommended windowed configuration: a dedicated render thread and asset
    /// hot-reload on, on top of the [`Default`] timestep and sync settings.
    pub fn windowed() -> Self {
        Self {
            render_thread: true,
            hot_reload: true,
            ..Self::default()
        }
    }
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            fixed_timestep: 1.0 / 60.0,
            max_fixed_steps_per_frame: 8,
            max_frame_delta: 0.25,
            sync_mode: SyncMode::Immediate,
            asset_root: PathBuf::from("."),
            hot_reload: false,
            render_thread: false,
            window: WindowConfig::default(),
        }
    }
}
