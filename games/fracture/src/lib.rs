#![deny(warnings)]

pub mod audio;
pub mod ball;
pub mod brick;
pub mod components;
pub mod error;
pub mod field;
pub mod gameplay;
pub mod input;
pub mod paddle;
pub mod physics;
pub mod powerup;
pub mod render;
pub mod resources;

use spawn_core::Transform3D;
use spawn_ecs::World;
use spawn_engine::{App, EngineConfig, ScheduleLabel, SyncMode, WindowConfig};
use spawn_physics::ecs::{Collider, PhysicsBody, RigidBody};

use crate::error::FractureResult;
use crate::resources::{Collisions, GameRng, GameState, PaddleControl, PaddleState, SlowTimer};

pub use field::{FIELD_HEIGHT, FIELD_WIDTH};

const WINDOW_SIZE: (u32, u32) = (720, 864);

fn config() -> EngineConfig {
    EngineConfig {
        sync_mode: SyncMode::Immediate,
        window: WindowConfig {
            title: "Fracture".to_string(),
            size: WINDOW_SIZE,
            ..WindowConfig::default()
        },
        ..EngineConfig::default()
    }
}

fn register_components(world: &mut World) {
    world.register::<Transform3D>();
    world.register::<components::Renderable>();
    world.register::<components::Ball>();
    world.register::<components::Paddle>();
    world.register::<components::Brick>();
    world.register::<components::Wall>();
    world.register::<components::PowerUp>();
    world.register::<components::BackPlane>();
    world.register::<physics::LinVel>();
    world.register::<RigidBody>();
    world.register::<Collider>();
    world.register::<PhysicsBody>();
}

pub fn build() -> FractureResult<App> {
    let mut app = App::new();
    app.set_config(config());
    register_components(app.world_mut());
    app.insert_resource(GameState::default());
    app.insert_resource(Collisions::default());
    app.insert_resource(PaddleControl::default());
    app.insert_resource(PaddleState::default());
    app.insert_resource(SlowTimer::default());
    app.insert_resource(GameRng::seeded(0));
    app.add_startup_system(physics::spawn_field);
    app.add_startup_system(render::spawn_back_plane);
    app.add_render_setup(render::setup);
    app.add_extract(render::extract);
    app.add_audio_setup(audio::setup);
    app.add_system(ScheduleLabel::Update, input::sample_input);
    app.add_fixed_hook(|world, _time| {
        gameplay::gameplay(world);
        Ok(())
    });
    app.add_fixed_hook(physics::fixed_hook()?);
    Ok(app)
}

pub fn run() -> FractureResult<()> {
    build()?.run()?;
    Ok(())
}
