//! Per-frame asset resources surfaced into the world by the frame loop's asset
//! pump: the applied-load report and the in-place reload events for the frame.

use spawn_asset::{AppliedReport, ReloadEvent};
use spawn_ecs::Resource;

/// The most recent [`AssetServer::apply_loaded`](spawn_asset::AssetServer::apply_loaded)
/// outcome, refreshed each frame before the schedules run so systems observe this
/// frame's loads, reloads, and failures.
#[derive(Debug, Default, Clone, Copy)]
pub struct FrameAssets {
    /// Loads, reloads, and failures applied this frame.
    pub applied: AppliedReport,
}

impl Resource for FrameAssets {}

/// Assets swapped in place by hot-reload this frame. Empty when hot-reload is
/// disabled or idle. Cleared (retaining capacity) each frame, so a steady state
/// with no reloads is allocation-free.
#[derive(Debug, Default)]
pub struct ReloadEvents {
    events: Vec<ReloadEvent>,
}

impl Resource for ReloadEvents {}

impl ReloadEvents {
    /// The assets reloaded in place this frame.
    pub fn events(&self) -> &[ReloadEvent] {
        &self.events
    }

    /// Whether any asset reloaded this frame.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Number of assets reloaded this frame.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Replaces this frame's events with `incoming`, reusing the existing buffer
    /// (cleared-not-freed) so an idle frame allocates nothing.
    pub(crate) fn refresh(&mut self, incoming: Vec<ReloadEvent>) {
        self.events.clear();
        self.events.extend(incoming);
    }
}
