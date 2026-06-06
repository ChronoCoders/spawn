//! The [`NullBackend`]: a fully state-tracking no-op device used when the OS
//! audio device cannot be opened. Every operation is accepted and succeeds;
//! playback produces no sound. Voices never auto-finish — they end only via an
//! explicit stop — so voice accounting matches the device backend's
//! caller-driven lifecycle for game logic that depends on handle state.

use crate::bus::BusId;
use crate::error::AudioResult;

use super::{Backend, BackendKind, BackendSound, Voice};

pub(crate) struct NullBackend;

impl NullBackend {
    pub(crate) fn new() -> Self {
        Self
    }
}

struct NullVoice;

impl Voice for NullVoice {
    fn set_gains(&mut self, _left: f32, _right: f32) {}
    fn set_pitch(&mut self, _ratio: f32) {}
    fn pause(&mut self) {}
    fn resume(&mut self) {}
    fn stop(&mut self) {}
    fn finished(&self) -> bool {
        false
    }
}

impl Backend for NullBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Null
    }

    fn create_bus(&mut self, _bus: BusId, _initial_volume: f32) -> AudioResult<()> {
        Ok(())
    }

    fn set_bus_volume(&mut self, _bus: BusId, _amplitude: f32) -> AudioResult<()> {
        Ok(())
    }

    fn set_master_volume(&mut self, _amplitude: f32) -> AudioResult<()> {
        Ok(())
    }

    fn suspend(&mut self) -> AudioResult<()> {
        Ok(())
    }

    fn resume(&mut self) -> AudioResult<()> {
        Ok(())
    }

    fn play(
        &mut self,
        _sound: &BackendSound,
        _bus: BusId,
        _left: f32,
        _right: f32,
        _pitch: f32,
        _looping: bool,
    ) -> AudioResult<Box<dyn Voice>> {
        Ok(Box::new(NullVoice))
    }
}
