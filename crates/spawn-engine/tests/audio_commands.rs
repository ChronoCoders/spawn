use spawn_asset::{AssetServer, Handle};
use spawn_audio::{AudioSource, PlaybackParams};
use spawn_ecs::{Commands, Res, ResMut, Resource};
use spawn_engine::{App, AudioCommands, ScheduleLabel};

struct Clip(Handle<AudioSource>);
impl Resource for Clip {}

#[test]
fn audio_commands_are_drained_each_tick() {
    let mut app = App::new();
    app.add_audio_setup(|assets: &mut AssetServer, world| {
        let _ = spawn_audio::register(assets);
        let handle = assets.load::<AudioSource>("does-not-exist.wav");
        world.insert_resource(Clip(handle));
        Ok(())
    });
    app.add_system(
        ScheduleLabel::Update,
        |clip: Res<'_, Clip>, mut audio: ResMut<'_, AudioCommands>, _c: &mut Commands<'_>| {
            audio.play(clip.0.clone(), PlaybackParams::default());
            Ok(())
        },
    );

    let mut engine = app.build_headless().unwrap();
    assert!(engine.world().get_resource::<AudioCommands>().is_some());
    engine.tick().unwrap();
    assert!(
        engine
            .world()
            .get_resource::<AudioCommands>()
            .unwrap()
            .is_empty(),
        "the engine drained the queue during the tick"
    );
}
