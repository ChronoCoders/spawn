//! Backend abstraction. All kira usage is confined to [`kira`] (this module's
//! private `kira` submodule); no kira type appears in any signature exposed
//! above this module.
//!
//! Threading: a backend's command methods are called only from the engine on
//! the main thread, once per frame from [`crate::AudioEngine::update`] (or
//! synchronously from control calls). They must not block on the audio render
//! thread; kira's own command queue absorbs the work.

mod null;

#[path = "kira.rs"]
mod kira_backend;

use crate::bus::BusId;
use crate::error::AudioResult;

pub(crate) use kira_backend::{decode_source, BackendSound, DeviceBackend};
pub(crate) use null::NullBackend;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Device,
    Null,
}

/// A started backend voice. The engine owns the lifecycle; the backend only
/// applies parameter changes and reports completion. All gains passed in are
/// final linear amplitudes already folded through voice/bus/master and
/// spatialization by the engine.
pub(crate) trait Voice: Send {
    fn set_gains(&mut self, left: f32, right: f32);
    fn set_pitch(&mut self, ratio: f32);
    fn pause(&mut self);
    fn resume(&mut self);
    fn stop(&mut self);
    /// `true` once the underlying sound has finished and the voice may be reaped.
    fn finished(&self) -> bool;
}

pub(crate) trait Backend {
    fn kind(&self) -> BackendKind;
    fn create_bus(&mut self, bus: BusId, initial_volume: f32) -> AudioResult<()>;
    fn set_bus_volume(&mut self, bus: BusId, amplitude: f32) -> AudioResult<()>;
    fn set_master_volume(&mut self, amplitude: f32) -> AudioResult<()>;
    fn suspend(&mut self) -> AudioResult<()>;
    fn resume(&mut self) -> AudioResult<()>;
    /// Starts a voice on `bus` from `sound`. `left`/`right` are the initial
    /// per-channel linear gains; `pitch` is the playback-rate multiplier;
    /// `looping` selects the loop region.
    fn play(
        &mut self,
        sound: &BackendSound,
        bus: BusId,
        left: f32,
        right: f32,
        pitch: f32,
        looping: bool,
    ) -> AudioResult<Box<dyn Voice>>;
}
