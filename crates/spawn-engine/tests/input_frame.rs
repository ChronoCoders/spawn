use spawn_ecs::{Commands, Res, ResMut, Resource};
use spawn_engine::{App, InputFrame, KeyCode, MouseButton, ScheduleLabel};

#[derive(Default)]
struct Seen(bool);
impl Resource for Seen {}

#[test]
fn input_frame_is_published_and_readable_by_systems() {
    let mut app = App::new();
    app.insert_resource(Seen::default());
    app.add_system(
        ScheduleLabel::Update,
        |input: Res<'_, InputFrame>, mut seen: ResMut<'_, Seen>, _c: &mut Commands<'_>| {
            assert!(!input.keyboard().is_pressed(KeyCode::A));
            assert!(!input.mouse().is_pressed(MouseButton::Left));
            seen.0 = true;
            Ok(())
        },
    );

    let mut engine = app.build_headless().unwrap();
    // Present before the first tick (inserted at assembly).
    assert!(engine.world().get_resource::<InputFrame>().is_some());
    engine.tick().unwrap();
    assert!(
        engine.world().get_resource::<Seen>().unwrap().0,
        "a system read Res<InputFrame> during the tick"
    );
}
