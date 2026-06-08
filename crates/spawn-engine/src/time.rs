//! The frame-clock resource. spawn-core defines no time type, so the engine owns
//! it; systems read it via `Res<Time>` and the engine is its only writer.

use spawn_ecs::Resource;

/// Per-frame timing, updated by the engine at the top of each tick before any
/// schedule runs. Systems access it as `Res<Time>` (read-only). `Copy` so the
/// engine can snapshot it to satisfy a fixed hook's `&Time` while the world holds
/// the authoritative resource.
#[derive(Debug, Clone, Copy)]
pub struct Time {
    delta: f32,
    elapsed: f64,
    fixed_delta: f32,
    fixed_elapsed: f64,
    frame: u64,
    fixed_tick: u64,
    interpolation_alpha: f32,
}

impl Resource for Time {}

impl Time {
    pub(crate) fn new(fixed_delta: f32) -> Self {
        Self {
            delta: 0.0,
            elapsed: 0.0,
            fixed_delta,
            fixed_elapsed: 0.0,
            frame: 0,
            fixed_tick: 0,
            interpolation_alpha: 0.0,
        }
    }

    /// Real seconds since the previous frame, clamped to the engine's frame-delta
    /// bound.
    pub fn delta(&self) -> f32 {
        self.delta
    }

    /// Total real seconds since the engine started.
    pub fn elapsed(&self) -> f64 {
        self.elapsed
    }

    /// Seconds per fixed simulation tick.
    pub fn fixed_delta(&self) -> f32 {
        self.fixed_delta
    }

    /// Total simulated seconds (`fixed_delta * fixed_tick`).
    pub fn fixed_elapsed(&self) -> f64 {
        self.fixed_elapsed
    }

    /// Render-frame counter.
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// Total fixed ticks run.
    pub fn fixed_tick(&self) -> u64 {
        self.fixed_tick
    }

    /// Accumulator remainder over `fixed_delta`, in `[0, 1)`; for render-side
    /// interpolation between fixed ticks. The engine surfaces it but does no
    /// smoothing itself.
    pub fn alpha(&self) -> f32 {
        self.interpolation_alpha
    }

    /// Advances the frame clock by one render frame of `delta` real seconds.
    pub(crate) fn advance_frame(&mut self, delta: f32) {
        self.delta = delta;
        self.elapsed += f64::from(delta);
        self.frame += 1;
    }

    /// Records one fixed tick.
    pub(crate) fn advance_fixed_tick(&mut self) {
        self.fixed_tick += 1;
        self.fixed_elapsed += f64::from(self.fixed_delta);
    }

    /// Sets the interpolation alpha from the leftover accumulator.
    pub(crate) fn set_alpha(&mut self, accumulator: f32) {
        self.interpolation_alpha = if self.fixed_delta > 0.0 {
            accumulator / self.fixed_delta
        } else {
            0.0
        };
    }
}
