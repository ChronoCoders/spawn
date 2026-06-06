//! Lightweight, `Copy` playback handles. Control operations live on
//! [`crate::AudioEngine`] keyed by handle, so the engine owns all mutation and
//! the command queue, and the handle stays kira-free.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SoundHandle {
    pub(crate) index: u32,
    pub(crate) generation: u32,
}

impl SoundHandle {
    pub(crate) fn new(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceState {
    Playing,
    Paused,
    Stopped,
}
