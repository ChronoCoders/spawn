//! Engine configuration.

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
    /// Window configuration; ignored in headless mode.
    pub window: WindowConfig,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            fixed_timestep: 1.0 / 60.0,
            max_fixed_steps_per_frame: 8,
            max_frame_delta: 0.25,
            sync_mode: SyncMode::Immediate,
            window: WindowConfig::default(),
        }
    }
}
