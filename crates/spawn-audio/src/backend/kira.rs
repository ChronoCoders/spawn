//! The one and only place kira is named. Everything kira-typed is `pub(crate)`
//! at most through opaque wrappers ([`BackendSound`], the [`super::Voice`] impl);
//! no kira symbol escapes the `backend` module.
//!
//! Decode path: [`decode_source`] is the single entry point used by the asset
//! loader; it turns raw `wav`/`ogg` bytes into a [`BackendSound`] via kira's
//! symphonia integration (codecs limited to `wav`/`vorbis` by the manifest).

use std::collections::HashMap;
use std::io::Cursor;

use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::sound::{PlaybackState, Region};
use kira::track::{TrackBuilder, TrackHandle};
use kira::{AudioManager, AudioManagerSettings, Decibels, DefaultBackend, Tween, Value};

use crate::bus::BusId;
use crate::error::{AudioError, AudioResult};
use crate::params::amplitude_to_db;

use super::{Backend, BackendKind, Voice};

const INSTANT: Tween = Tween {
    start_time: kira::StartTime::Immediate,
    duration: std::time::Duration::ZERO,
    easing: kira::Easing::Linear,
};

fn amplitude_to_value(amplitude: f32) -> Value<Decibels> {
    Value::Fixed(Decibels(amplitude_to_db(amplitude.clamp(0.0, 1.0))))
}

/// Opaque, ref-counted decoded clip. Wraps kira's static sound data; cloning is
/// cheap (kira shares the sample buffer via `Arc` internally).
pub(crate) struct BackendSound {
    data: StaticSoundData,
}

impl BackendSound {
    pub(crate) fn duration_secs(&self) -> f32 {
        self.data.duration().as_secs_f32()
    }

    pub(crate) fn sample_rate(&self) -> u32 {
        self.data.sample_rate
    }

    pub(crate) fn channels(&self) -> u16 {
        // kira decodes to interleaved frames; sample type carries channel count.
        if self.data.num_frames() == 0 {
            0
        } else {
            2
        }
    }

    fn looped(&self, looping: bool) -> StaticSoundData {
        if looping {
            self.data.clone().loop_region(Region::from(0.0..))
        } else {
            self.data.clone()
        }
    }
}

/// Decodes raw `wav`/`ogg` bytes into a [`BackendSound`]. Returns
/// [`AudioError::Decode`] on malformed input. The supported-extension gate is
/// applied by the loader before this is called.
pub(crate) fn decode_source(bytes: &[u8]) -> AudioResult<BackendSound> {
    let cursor = Cursor::new(bytes.to_vec());
    match StaticSoundData::from_cursor(cursor) {
        Ok(data) => Ok(BackendSound { data }),
        Err(_) => Err(AudioError::Decode {
            context: "failed to decode wav/ogg audio data",
        }),
    }
}

struct KiraVoice {
    handle: StaticSoundHandle,
}

impl Voice for KiraVoice {
    fn set_gains(&mut self, left: f32, right: f32) {
        let amplitude = (left * left + right * right).sqrt().clamp(0.0, 1.0);
        let panning = (right * right - left * left).clamp(-1.0, 1.0);
        self.handle
            .set_volume(amplitude_to_value(amplitude), INSTANT);
        self.handle
            .set_panning(Value::Fixed(kira::Panning(panning)), INSTANT);
    }

    fn set_pitch(&mut self, ratio: f32) {
        self.handle
            .set_playback_rate(Value::Fixed(kira::PlaybackRate(ratio as f64)), INSTANT);
    }

    fn pause(&mut self) {
        self.handle.pause(INSTANT);
    }

    fn resume(&mut self) {
        self.handle.resume(INSTANT);
    }

    fn stop(&mut self) {
        self.handle.stop(INSTANT);
    }

    fn finished(&self) -> bool {
        self.handle.state() == PlaybackState::Stopped
    }
}

pub(crate) struct DeviceBackend {
    manager: AudioManager<DefaultBackend>,
    buses: HashMap<BusId, TrackHandle>,
    master_volume: f32,
    suspended: bool,
}

impl DeviceBackend {
    /// Attempts to open the default OS audio device. Returns
    /// [`AudioError::DeviceUnavailable`] if no device can be initialized; the
    /// engine treats that as the documented [`BackendKind::Null`] fallback.
    pub(crate) fn try_new() -> AudioResult<Self> {
        let manager = AudioManager::<DefaultBackend>::new(AudioManagerSettings::default())
            .map_err(|_| AudioError::DeviceUnavailable {
                context: "no default audio device could be opened",
            })?;
        Ok(Self {
            manager,
            buses: HashMap::new(),
            master_volume: 1.0,
            suspended: false,
        })
    }
}

impl Backend for DeviceBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Device
    }

    fn create_bus(&mut self, bus: BusId, initial_volume: f32) -> AudioResult<()> {
        let builder = TrackBuilder::new().volume(amplitude_to_value(initial_volume));
        let track = self
            .manager
            .add_sub_track(builder)
            .map_err(|_| AudioError::Backend {
                context: "failed to create mixer sub-track",
            })?;
        self.buses.insert(bus, track);
        Ok(())
    }

    fn set_bus_volume(&mut self, bus: BusId, amplitude: f32) -> AudioResult<()> {
        let track = self.buses.get_mut(&bus).ok_or(AudioError::UnknownBus)?;
        track.set_volume(amplitude_to_value(amplitude), INSTANT);
        Ok(())
    }

    fn set_master_volume(&mut self, amplitude: f32) -> AudioResult<()> {
        self.master_volume = amplitude.clamp(0.0, 1.0);
        if !self.suspended {
            self.manager
                .main_track()
                .set_volume(amplitude_to_value(self.master_volume), INSTANT);
        }
        Ok(())
    }

    // kira 0.10 exposes no public whole-device pause; suspend/resume gate the
    // main track to silence and back. Idempotent. The stored master volume is
    // restored on resume so a suspend/resume pair is transparent.
    fn suspend(&mut self) -> AudioResult<()> {
        if !self.suspended {
            self.suspended = true;
            self.manager
                .main_track()
                .set_volume(amplitude_to_value(0.0), INSTANT);
        }
        Ok(())
    }

    fn resume(&mut self) -> AudioResult<()> {
        if self.suspended {
            self.suspended = false;
            self.manager
                .main_track()
                .set_volume(amplitude_to_value(self.master_volume), INSTANT);
        }
        Ok(())
    }

    fn play(
        &mut self,
        sound: &BackendSound,
        bus: BusId,
        left: f32,
        right: f32,
        pitch: f32,
        looping: bool,
    ) -> AudioResult<Box<dyn Voice>> {
        let amplitude = (left * left + right * right).sqrt().clamp(0.0, 1.0);
        let panning = (right * right - left * left).clamp(-1.0, 1.0);
        let data = sound
            .looped(looping)
            .volume(amplitude_to_value(amplitude))
            .panning(Value::Fixed(kira::Panning(panning)))
            .playback_rate(Value::Fixed(kira::PlaybackRate(pitch as f64)));

        let handle = if bus == BusId::MASTER {
            self.manager.play(data).map_err(|_| AudioError::Backend {
                context: "failed to start sound on master track",
            })?
        } else {
            let track = self.buses.get_mut(&bus).ok_or(AudioError::UnknownBus)?;
            track.play(data).map_err(|_| AudioError::Backend {
                context: "failed to start sound on bus track",
            })?
        };
        Ok(Box::new(KiraVoice { handle }))
    }
}
