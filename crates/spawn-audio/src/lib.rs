#![deny(warnings)]

//! Audio subsystem for the Spawn engine: a per-frame-pumped [`AudioEngine`]
//! over a kira-backed mixer with a master bus and named buses, WAV/OGG loading
//! through an [`AudioLoader`] registered with spawn-asset, `Copy` playback
//! handles, one listener with distance attenuation and equal-power panning, and
//! an explicit silent [`BackendKind::Null`] fallback when no audio device can be
//! opened.
//!
//! Volume is linear amplitude `0.0..=1.0` at every public boundary (clamped, not
//! rejected); decibels appear only in [`params::db_to_amplitude`] /
//! [`params::amplitude_to_db`]. No kira type appears in any public signature —
//! kira is confined to the private `backend` module.

mod backend;
pub mod bus;
pub mod engine;
pub mod error;
pub mod handle;
pub mod params;
pub mod source;
pub mod spatial;

pub use backend::BackendKind;
pub use bus::BusId;
pub use engine::{AudioConfig, AudioEngine, BusSpec};
pub use error::{AudioError, AudioResult};
pub use handle::{SoundHandle, VoiceState};
pub use params::{
    amplitude_to_db, attenuation_gain, clamp_amplitude, db_to_amplitude, equal_power_gains,
    stereo_pan, PlaybackParams, Spatial,
};
pub use source::{register, AudioLoader, AudioSource};
pub use spatial::{Attenuation, AttenuationModel, Listener};
