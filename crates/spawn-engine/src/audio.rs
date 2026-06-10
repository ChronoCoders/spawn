use spawn_asset::{AssetServer, Handle};
use spawn_audio::{AudioSource, PlaybackParams};
use spawn_ecs::{Resource, World};

use crate::error::EngineResult;

pub(crate) type AudioSetup = Box<dyn FnOnce(&mut AssetServer, &mut World) -> EngineResult<()>>;

pub struct AudioCommand {
    pub source: Handle<AudioSource>,
    pub params: PlaybackParams,
}

#[derive(Default)]
pub struct AudioCommands {
    queue: Vec<AudioCommand>,
}

impl Resource for AudioCommands {}

impl AudioCommands {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn play(&mut self, source: Handle<AudioSource>, params: PlaybackParams) {
        self.queue.push(AudioCommand { source, params });
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub(crate) fn drain(&mut self) -> std::vec::Drain<'_, AudioCommand> {
        self.queue.drain(..)
    }
}
