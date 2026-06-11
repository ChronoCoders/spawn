use spawn_asset::{AssetServer, Handle};
use spawn_audio::{AudioSource, PlaybackParams};
use spawn_ecs::{Resource, World};
use spawn_engine::{AudioCommands, EngineResult};

use crate::components::{Ball, Paddle, Wall, WallSide};
use crate::resources::Contact;

#[derive(Debug, Clone, Copy)]
pub enum SoundEffect {
    Hit,
    Break,
    Pickup,
    LevelComplete,
    GameOver,
}

pub struct GameAudio {
    hit: Handle<AudioSource>,
    brick_break: Handle<AudioSource>,
    pickup: Handle<AudioSource>,
    level_complete: Handle<AudioSource>,
    game_over: Handle<AudioSource>,
}

impl Resource for GameAudio {}

impl GameAudio {
    fn handle(&self, effect: SoundEffect) -> Handle<AudioSource> {
        match effect {
            SoundEffect::Hit => self.hit.clone(),
            SoundEffect::Break => self.brick_break.clone(),
            SoundEffect::Pickup => self.pickup.clone(),
            SoundEffect::LevelComplete => self.level_complete.clone(),
            SoundEffect::GameOver => self.game_over.clone(),
        }
    }
}

pub fn setup(assets: &mut AssetServer, world: &mut World) -> EngineResult<()> {
    let _ = spawn_audio::register(assets);
    let audio = GameAudio {
        hit: assets.load::<AudioSource>("assets/audio/hit.wav"),
        brick_break: assets.load::<AudioSource>("assets/audio/break.wav"),
        pickup: assets.load::<AudioSource>("assets/audio/pickup.wav"),
        level_complete: assets.load::<AudioSource>("assets/audio/level_complete.wav"),
        game_over: assets.load::<AudioSource>("assets/audio/game_over.wav"),
    };
    world.insert_resource(audio);
    Ok(())
}

pub fn play(world: &mut World, effect: SoundEffect) {
    let handle = world
        .get_resource::<GameAudio>()
        .map(|audio| audio.handle(effect));
    if let Some(handle) = handle {
        if let Some(mut commands) = world.get_resource_mut::<AudioCommands>() {
            commands.play(handle, PlaybackParams::default());
        }
    }
}

fn is_ball_bounce(world: &World, contact: &Contact) -> bool {
    let other = if world.get::<Ball>(contact.a).is_some() {
        contact.b
    } else if world.get::<Ball>(contact.b).is_some() {
        contact.a
    } else {
        return false;
    };
    if world.get::<Paddle>(other).is_some() {
        return true;
    }
    world
        .get::<Wall>(other)
        .map(|wall| wall.side != WallSide::Bottom)
        .unwrap_or(false)
}

pub fn hit_cues(world: &mut World, contacts: &[Contact]) {
    let bounced = contacts
        .iter()
        .any(|contact| contact.started && is_ball_bounce(world, contact));
    if bounced {
        play(world, SoundEffect::Hit);
    }
}
