//! The [`AudioEngine`]: a per-frame-pumped mixer over a swappable backend.
//!
//! Threading contract: every public method is main-thread only. Control calls
//! (`play`, `stop`, volume, …) enqueue commands and return synchronously; the
//! audio render thread is owned by the backend and never blocks the caller.
//! [`AudioEngine::update`] is the single point that drains the queue, applies
//! spatialization, and reaps finished voices, and is allocation-free in steady
//! state (the command queue and voice table are pre-sized and reused).

use std::collections::VecDeque;
use std::sync::Arc;

use spawn_asset::{AssetServer, Handle};
use spawn_core::Vec3;

use crate::backend::{Backend, BackendKind, DeviceBackend, NullBackend, Voice};
use crate::bus::BusId;
use crate::error::{AudioError, AudioResult};
use crate::handle::{SoundHandle, VoiceState};
use crate::params::{
    attenuation_gain, clamp_amplitude, equal_power_gains, stereo_pan, PlaybackParams, Spatial,
};
use crate::source::AudioSource;
use crate::spatial::Listener;

/// Specification for one named bus created at [`AudioEngine::new`].
#[derive(Debug, Clone)]
pub struct BusSpec {
    /// The bus identifier. Must not be [`BusId::MASTER`] (reserved → [`AudioError::ReservedBus`])
    /// and must be unique across the config (duplicates → [`AudioError::DuplicateBus`]).
    pub id: BusId,
    /// Initial linear amplitude, clamped to `0.0..=1.0` at init.
    pub initial_volume: f32,
}

/// Configuration consumed once by [`AudioEngine::new`].
#[derive(Debug, Clone)]
pub struct AudioConfig {
    /// Advisory device sample rate; the device may ignore it. `None` = device default.
    pub sample_rate_hint: Option<u32>,
    /// Named buses to create at init. The master bus ([`BusId::MASTER`]) is implicit
    /// and must not appear here. Empty is valid (master only).
    pub buses: Vec<BusSpec>,
    /// Pre-sizes the voice table; `play` beyond this returns [`AudioError::VoiceLimit`]
    /// rather than allocating.
    pub max_voices: usize,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sample_rate_hint: None,
            buses: Vec::new(),
            max_voices: 64,
        }
    }
}

struct BusEntry {
    id: BusId,
    volume: f32,
}

enum Command {
    Start {
        index: u32,
        generation: u32,
        source: Arc<AudioSource>,
        bus: BusId,
        volume: f32,
        pitch: f32,
        looping: bool,
        spatial: Option<Spatial>,
    },
    Stop {
        index: u32,
        generation: u32,
    },
    Pause {
        index: u32,
        generation: u32,
    },
    Resume {
        index: u32,
        generation: u32,
    },
    SetVolume {
        index: u32,
        generation: u32,
        amplitude: f32,
    },
    SetPitch {
        index: u32,
        generation: u32,
        ratio: f32,
    },
    SetPosition {
        index: u32,
        generation: u32,
        position: Vec3,
    },
}

struct VoiceSlot {
    occupied: bool,
    generation: u32,
    state: VoiceState,
    bus: BusId,
    volume: f32,
    pitch: f32,
    spatial: Option<Spatial>,
    voice: Option<Box<dyn Voice>>,
}

impl VoiceSlot {
    fn empty() -> Self {
        Self {
            occupied: false,
            generation: 0,
            state: VoiceState::Stopped,
            bus: BusId::MASTER,
            volume: 1.0,
            pitch: 1.0,
            spatial: None,
            voice: None,
        }
    }
}

pub struct AudioEngine {
    backend: Box<dyn Backend>,
    kind: BackendKind,
    master_volume: f32,
    buses: Vec<BusEntry>,
    voices: Vec<VoiceSlot>,
    commands: VecDeque<Command>,
    listener: Listener,
    active: usize,
}

impl AudioEngine {
    /// Opens the OS audio device. On device-init failure this still returns
    /// `Ok`, falling back to an explicit [`BackendKind::Null`] backend (logged
    /// via [`AudioEngine::backend_kind`], never silent, never a panic). Only
    /// configuration errors return `Err`: a [`BusSpec`] using [`BusId::MASTER`]
    /// yields [`AudioError::ReservedBus`]; two specs sharing an id yield
    /// [`AudioError::DuplicateBus`].
    pub fn new(config: AudioConfig) -> AudioResult<Self> {
        for (i, spec) in config.buses.iter().enumerate() {
            if spec.id == BusId::MASTER {
                return Err(AudioError::ReservedBus);
            }
            if config.buses[..i].iter().any(|b| b.id == spec.id) {
                return Err(AudioError::DuplicateBus);
            }
        }

        let mut backend: Box<dyn Backend> = match DeviceBackend::try_new() {
            Ok(device) => Box::new(device),
            Err(err) => {
                log_device_fallback(err);
                Box::new(NullBackend::new())
            }
        };
        let kind = backend.kind();

        let mut buses = Vec::with_capacity(config.buses.len());
        for spec in &config.buses {
            let volume = clamp_amplitude(spec.initial_volume);
            backend.create_bus(spec.id, volume)?;
            buses.push(BusEntry {
                id: spec.id,
                volume,
            });
        }

        let max_voices = config.max_voices;
        let mut voices = Vec::with_capacity(max_voices);
        for _ in 0..max_voices {
            voices.push(VoiceSlot::empty());
        }

        Ok(Self {
            backend,
            kind,
            master_volume: 1.0,
            buses,
            voices,
            commands: VecDeque::with_capacity(max_voices.max(1)),
            listener: Listener::default(),
            active: 0,
        })
    }

    /// Whether a real audio device is driving output ([`BackendKind::Device`]) or
    /// the engine fell back to a no-op [`BackendKind::Null`] device (running with
    /// audio disabled). Lets the caller log/surface that audio is unavailable.
    pub fn backend_kind(&self) -> BackendKind {
        self.kind
    }

    fn bus_index(&self, bus: BusId) -> AudioResult<Option<usize>> {
        if bus == BusId::MASTER {
            return Ok(None);
        }
        self.buses
            .iter()
            .position(|b| b.id == bus)
            .map(Some)
            .ok_or(AudioError::UnknownBus)
    }

    fn bus_volume_for(&self, bus: BusId) -> f32 {
        if bus == BusId::MASTER {
            1.0
        } else {
            self.buses
                .iter()
                .find(|b| b.id == bus)
                .map(|b| b.volume)
                .unwrap_or(1.0)
        }
    }

    /// Sets a bus's linear amplitude. `amplitude` is clamped to `0.0..=1.0`
    /// (non-finite → `0`); an unknown bus returns [`AudioError::UnknownBus`].
    pub fn set_bus_volume(&mut self, bus: BusId, amplitude: f32) -> AudioResult<()> {
        let amplitude = clamp_amplitude(amplitude);
        if bus == BusId::MASTER {
            self.master_volume = amplitude;
            return self.backend.set_master_volume(amplitude);
        }
        let idx = self.bus_index(bus)?.ok_or(AudioError::UnknownBus)?;
        self.buses[idx].volume = amplitude;
        self.backend.set_bus_volume(bus, amplitude)
    }

    pub fn bus_volume(&self, bus: BusId) -> AudioResult<f32> {
        if bus == BusId::MASTER {
            return Ok(self.master_volume);
        }
        self.buses
            .iter()
            .find(|b| b.id == bus)
            .map(|b| b.volume)
            .ok_or(AudioError::UnknownBus)
    }

    pub fn set_listener(&mut self, listener: Listener) {
        self.listener = listener;
    }

    pub fn listener(&self) -> Listener {
        self.listener
    }

    /// Pauses the whole device (e.g. when the app is backgrounded). Idempotent:
    /// suspending an already-suspended engine is a successful no-op.
    pub fn suspend(&mut self) -> AudioResult<()> {
        self.backend.suspend()
    }

    /// Resumes device output after [`suspend`](Self::suspend). Idempotent:
    /// resuming a running engine is a successful no-op.
    pub fn resume(&mut self) -> AudioResult<()> {
        self.backend.resume()
    }

    pub fn active_voice_count(&self) -> usize {
        self.active
    }

    fn alloc_slot(&mut self) -> Option<u32> {
        self.voices
            .iter()
            .position(|s| !s.occupied)
            .map(|i| i as u32)
    }

    /// Enqueues a play command and returns a handle immediately (non-blocking).
    /// An unloaded handle yields [`AudioError::AssetNotLoaded`]; an unknown
    /// `params.bus` yields [`AudioError::UnknownBus`]; a full voice table yields
    /// [`AudioError::VoiceLimit`] (no allocation beyond the pre-sized table).
    pub fn play(
        &mut self,
        source: &Handle<AudioSource>,
        params: PlaybackParams,
        server: &AssetServer,
    ) -> AudioResult<SoundHandle> {
        let _ = self.bus_index(params.bus)?;
        let asset = server.get(source).ok_or(AudioError::AssetNotLoaded)?;

        let index = self.alloc_slot().ok_or(AudioError::VoiceLimit)?;
        let slot = &mut self.voices[index as usize];
        slot.occupied = true;
        slot.generation = slot.generation.wrapping_add(1);
        slot.state = VoiceState::Playing;
        slot.bus = params.bus;
        slot.volume = clamp_amplitude(params.volume);
        slot.pitch = clamp_pitch(params.pitch);
        slot.spatial = params.spatial;
        slot.voice = None;
        let generation = slot.generation;
        self.active += 1;

        self.commands.push_back(Command::Start {
            index,
            generation,
            source: asset,
            bus: params.bus,
            volume: slot.volume,
            pitch: slot.pitch,
            looping: params.looping,
            spatial: params.spatial,
        });
        Ok(SoundHandle::new(index, generation))
    }

    fn live_slot(&self, h: SoundHandle) -> AudioResult<usize> {
        let idx = h.index as usize;
        let slot = self.voices.get(idx).ok_or(AudioError::InvalidHandle)?;
        if slot.occupied && slot.generation == h.generation {
            Ok(idx)
        } else {
            Err(AudioError::InvalidHandle)
        }
    }

    /// Idempotent; a stale or reaped handle yields [`AudioError::InvalidHandle`].
    pub fn stop(&mut self, h: SoundHandle) -> AudioResult<()> {
        let idx = self.live_slot(h)?;
        self.voices[idx].state = VoiceState::Stopped;
        self.commands.push_back(Command::Stop {
            index: h.index,
            generation: h.generation,
        });
        Ok(())
    }

    /// A stale handle yields [`AudioError::InvalidHandle`].
    pub fn pause(&mut self, h: SoundHandle) -> AudioResult<()> {
        let idx = self.live_slot(h)?;
        if self.voices[idx].state == VoiceState::Playing {
            self.voices[idx].state = VoiceState::Paused;
        }
        self.commands.push_back(Command::Pause {
            index: h.index,
            generation: h.generation,
        });
        Ok(())
    }

    /// A stale handle yields [`AudioError::InvalidHandle`].
    pub fn resume_handle(&mut self, h: SoundHandle) -> AudioResult<()> {
        let idx = self.live_slot(h)?;
        if self.voices[idx].state == VoiceState::Paused {
            self.voices[idx].state = VoiceState::Playing;
        }
        self.commands.push_back(Command::Resume {
            index: h.index,
            generation: h.generation,
        });
        Ok(())
    }

    /// `amplitude` is clamped to `0.0..=1.0`. Stale handle →
    /// [`AudioError::InvalidHandle`].
    pub fn set_volume(&mut self, h: SoundHandle, amplitude: f32) -> AudioResult<()> {
        let idx = self.live_slot(h)?;
        let amplitude = clamp_amplitude(amplitude);
        self.voices[idx].volume = amplitude;
        self.commands.push_back(Command::SetVolume {
            index: h.index,
            generation: h.generation,
            amplitude,
        });
        Ok(())
    }

    /// Playback-rate multiplier, `1.0` = original, clamped to `0.0..=8.0`. A
    /// non-finite `ratio` yields [`AudioError::Backend`]; a stale handle yields
    /// [`AudioError::InvalidHandle`].
    pub fn set_pitch(&mut self, h: SoundHandle, ratio: f32) -> AudioResult<()> {
        if !ratio.is_finite() {
            return Err(AudioError::Backend {
                context: "pitch ratio must be finite",
            });
        }
        let idx = self.live_slot(h)?;
        let ratio = clamp_pitch(ratio);
        self.voices[idx].pitch = ratio;
        self.commands.push_back(Command::SetPitch {
            index: h.index,
            generation: h.generation,
            ratio,
        });
        Ok(())
    }

    /// Updates a spatial voice's emitter position. A no-op returning `Ok` if the
    /// voice was started non-spatial. Stale handle → [`AudioError::InvalidHandle`].
    pub fn set_position(&mut self, h: SoundHandle, position: Vec3) -> AudioResult<()> {
        let idx = self.live_slot(h)?;
        if self.voices[idx].spatial.is_none() {
            return Ok(());
        }
        self.commands.push_back(Command::SetPosition {
            index: h.index,
            generation: h.generation,
            position,
        });
        Ok(())
    }

    /// Lifecycle query for tests/logic. Stale handle → [`AudioError::InvalidHandle`].
    pub fn voice_state(&self, h: SoundHandle) -> AudioResult<VoiceState> {
        let idx = self.live_slot(h)?;
        Ok(self.voices[idx].state)
    }

    /// Pumped exactly once per frame. Drains the command queue, re-spatializes
    /// active spatial voices against the current listener, and reaps finished
    /// voices. Allocation-free in steady state.
    pub fn update(&mut self, _dt: f32) -> AudioResult<()> {
        while let Some(cmd) = self.commands.pop_front() {
            self.apply_command(cmd);
        }
        self.spatialize_and_reap();
        Ok(())
    }

    fn apply_command(&mut self, cmd: Command) {
        match cmd {
            Command::Start {
                index,
                generation,
                source,
                bus,
                volume,
                pitch,
                looping,
                spatial,
            } => {
                let slot_idx = index as usize;
                let stale = {
                    let slot = &self.voices[slot_idx];
                    !slot.occupied || slot.generation != generation
                };
                if stale {
                    return;
                }
                let bus_volume = self.bus_volume_for(bus);
                let (left, right) = initial_gains(volume, bus_volume, spatial, self.listener);
                match self
                    .backend
                    .play(source.backend_sound(), bus, left, right, pitch, looping)
                {
                    Ok(voice) => {
                        self.voices[slot_idx].voice = Some(voice);
                    }
                    Err(_) => {
                        self.free_slot(slot_idx);
                    }
                }
            }
            Command::Stop { index, generation } => {
                if let Some(slot) = self.matching_slot(index, generation) {
                    if let Some(v) = self.voices[slot].voice.as_mut() {
                        v.stop();
                    }
                    self.free_slot(slot);
                }
            }
            Command::Pause { index, generation } => {
                if let Some(slot) = self.matching_slot(index, generation) {
                    if let Some(v) = self.voices[slot].voice.as_mut() {
                        v.pause();
                    }
                }
            }
            Command::Resume { index, generation } => {
                if let Some(slot) = self.matching_slot(index, generation) {
                    if let Some(v) = self.voices[slot].voice.as_mut() {
                        v.resume();
                    }
                }
            }
            Command::SetVolume {
                index,
                generation,
                amplitude,
            } => {
                if let Some(slot) = self.matching_slot(index, generation) {
                    self.voices[slot].volume = amplitude;
                    self.apply_gains(slot);
                }
            }
            Command::SetPitch {
                index,
                generation,
                ratio,
            } => {
                if let Some(slot) = self.matching_slot(index, generation) {
                    self.voices[slot].pitch = ratio;
                    if let Some(v) = self.voices[slot].voice.as_mut() {
                        v.set_pitch(ratio);
                    }
                }
            }
            Command::SetPosition {
                index,
                generation,
                position,
            } => {
                if let Some(slot) = self.matching_slot(index, generation) {
                    if let Some(spatial) = self.voices[slot].spatial.as_mut() {
                        spatial.position = position;
                    }
                    self.apply_gains(slot);
                }
            }
        }
    }

    fn matching_slot(&self, index: u32, generation: u32) -> Option<usize> {
        let idx = index as usize;
        let slot = self.voices.get(idx)?;
        if slot.occupied && slot.generation == generation {
            Some(idx)
        } else {
            None
        }
    }

    fn apply_gains(&mut self, slot_idx: usize) {
        let (volume, bus, spatial) = {
            let slot = &self.voices[slot_idx];
            (slot.volume, slot.bus, slot.spatial)
        };
        let bus_volume = self.bus_volume_for(bus);
        let (left, right) = initial_gains(volume, bus_volume, spatial, self.listener);
        if let Some(v) = self.voices[slot_idx].voice.as_mut() {
            v.set_gains(left, right);
        }
    }

    fn spatialize_and_reap(&mut self) {
        for i in 0..self.voices.len() {
            if !self.voices[i].occupied {
                continue;
            }
            if self.voices[i]
                .voice
                .as_ref()
                .map(|v| v.finished())
                .unwrap_or(false)
            {
                if self.voices[i].voice.is_some() {
                    self.voices[i].voice = None;
                }
                self.free_slot(i);
                continue;
            }
            if self.voices[i].spatial.is_some() {
                self.apply_gains(i);
            }
        }
    }

    fn free_slot(&mut self, slot_idx: usize) {
        let slot = &mut self.voices[slot_idx];
        if slot.occupied {
            slot.occupied = false;
            slot.state = VoiceState::Stopped;
            slot.spatial = None;
            slot.voice = None;
            self.active = self.active.saturating_sub(1);
        }
    }
}

fn clamp_pitch(ratio: f32) -> f32 {
    if ratio.is_finite() {
        ratio.clamp(0.0, 8.0)
    } else {
        1.0
    }
}

fn initial_gains(
    volume: f32,
    bus_volume: f32,
    spatial: Option<Spatial>,
    listener: Listener,
) -> (f32, f32) {
    let base = clamp_amplitude(volume) * clamp_amplitude(bus_volume);
    match spatial {
        None => (base, base),
        Some(s) => {
            let distance = listener.position.distance(s.position);
            let dist_gain = attenuation_gain(s.attenuation, distance);
            let pan = stereo_pan(listener, s.position);
            let (l, r) = equal_power_gains(pan);
            (base * dist_gain * l, base * dist_gain * r)
        }
    }
}

fn log_device_fallback(err: AudioError) {
    eprintln!("[spawn-audio] audio device init failed ({err}); falling back to silent NullBackend");
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_core::ApproxEq;

    fn engine() -> AudioEngine {
        AudioEngine::new(AudioConfig::default()).expect("engine")
    }

    #[test]
    fn new_comes_up_never_errors() {
        let e = engine();
        // `engine()` already proves `new()` did not error. The backend is
        // `Device` on a host with audio output and `Null` on a headless host
        // (CI / no device); either way the engine must come up usable.
        assert!(matches!(
            e.backend_kind(),
            BackendKind::Device | BackendKind::Null
        ));
        assert_eq!(e.active_voice_count(), 0);
    }

    #[test]
    fn reserved_master_bus_rejected() {
        let cfg = AudioConfig {
            buses: vec![BusSpec {
                id: BusId::MASTER,
                initial_volume: 1.0,
            }],
            ..Default::default()
        };
        assert!(matches!(
            AudioEngine::new(cfg),
            Err(AudioError::ReservedBus)
        ));
    }

    #[test]
    fn duplicate_bus_rejected() {
        let cfg = AudioConfig {
            buses: vec![
                BusSpec {
                    id: BusId("sfx"),
                    initial_volume: 1.0,
                },
                BusSpec {
                    id: BusId("sfx"),
                    initial_volume: 0.5,
                },
            ],
            ..Default::default()
        };
        assert!(matches!(
            AudioEngine::new(cfg),
            Err(AudioError::DuplicateBus)
        ));
    }

    #[test]
    fn bus_volume_roundtrip_and_unknown() {
        let cfg = AudioConfig {
            buses: vec![BusSpec {
                id: BusId("sfx"),
                initial_volume: 0.5,
            }],
            ..Default::default()
        };
        let mut e = AudioEngine::new(cfg).expect("engine");
        assert!(e.bus_volume(BusId("sfx")).unwrap().approx_eq(0.5, 1e-6));
        e.set_bus_volume(BusId("sfx"), 2.0).unwrap();
        assert!(e.bus_volume(BusId("sfx")).unwrap().approx_eq(1.0, 1e-6));
        assert!(matches!(
            e.bus_volume(BusId("missing")),
            Err(AudioError::UnknownBus)
        ));
        assert!(matches!(
            e.set_bus_volume(BusId("missing"), 0.5),
            Err(AudioError::UnknownBus)
        ));
        e.set_bus_volume(BusId::MASTER, 0.25).unwrap();
        assert!(e.bus_volume(BusId::MASTER).unwrap().approx_eq(0.25, 1e-6));
    }

    #[test]
    fn listener_roundtrip() {
        let mut e = engine();
        let l = Listener {
            position: Vec3::new(1.0, 2.0, 3.0),
            orientation: spawn_core::Quat::IDENTITY,
        };
        e.set_listener(l);
        assert!(e.listener().position.approx_eq_default(l.position));
    }

    #[test]
    fn suspend_resume_idempotent() {
        let mut e = engine();
        assert!(e.suspend().is_ok());
        assert!(e.suspend().is_ok());
        assert!(e.resume().is_ok());
        assert!(e.resume().is_ok());
    }

    #[test]
    fn stale_handle_is_invalid() {
        let mut e = engine();
        let bogus = SoundHandle::new(0, 999);
        assert!(matches!(e.stop(bogus), Err(AudioError::InvalidHandle)));
        assert!(matches!(
            e.voice_state(bogus),
            Err(AudioError::InvalidHandle)
        ));
        assert!(matches!(
            e.set_pitch(SoundHandle::new(0, 1), f32::NAN),
            Err(AudioError::Backend { .. })
        ));
    }
}
