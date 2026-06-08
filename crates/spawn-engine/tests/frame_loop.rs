//! Frame-pipeline ordering, the `Time` clock, stage-boundary command
//! application, event once-per-frame delivery, and clean teardown.

use spawn_ecs::{Commands, Component, Event, EventReader, EventWriter, Query, ResMut, Resource};
use spawn_engine::{App, ScheduleLabel};

#[derive(Default)]
struct Order(Vec<&'static str>);
impl Resource for Order {}

#[derive(Default)]
struct Reads(u32);
impl Resource for Reads {}

struct Ping;
impl Event for Ping {}

#[derive(Clone, Copy)]
struct Marker;
impl Component for Marker {}

#[test]
fn pipeline_runs_fixed_before_variable_in_stage_order() {
    let mut app = App::new();
    app.insert_resource(Order::default());
    app.add_fixed_system(
        ScheduleLabel::First,
        |mut o: ResMut<'_, Order>, _c: &mut Commands<'_>| {
            o.0.push("fixed-first");
            Ok(())
        },
    );
    app.add_fixed_system(
        ScheduleLabel::Update,
        |mut o: ResMut<'_, Order>, _c: &mut Commands<'_>| {
            o.0.push("fixed-update");
            Ok(())
        },
    );
    app.add_system(
        ScheduleLabel::First,
        |mut o: ResMut<'_, Order>, _c: &mut Commands<'_>| {
            o.0.push("var-first");
            Ok(())
        },
    );
    app.add_system(
        ScheduleLabel::Update,
        |mut o: ResMut<'_, Order>, _c: &mut Commands<'_>| {
            o.0.push("var-update");
            Ok(())
        },
    );
    app.add_system(
        ScheduleLabel::Last,
        |mut o: ResMut<'_, Order>, _c: &mut Commands<'_>| {
            o.0.push("var-last");
            Ok(())
        },
    );

    let mut engine = app.build_headless().unwrap();
    engine.tick().unwrap();

    let order = engine.world().get_resource::<Order>().unwrap();
    assert_eq!(
        order.0,
        vec![
            "fixed-first",
            "fixed-update",
            "var-first",
            "var-update",
            "var-last"
        ]
    );
    assert_eq!(engine.time().frame(), 1);
    assert_eq!(engine.time().fixed_tick(), 1);
    assert!((engine.time().delta() - 1.0 / 60.0).abs() < 1e-6);
}

#[test]
fn commands_apply_at_boundary_and_events_do_not_leak() {
    let mut app = App::new();
    app.insert_resource(Reads::default());
    app.add_event::<Ping>();
    app.world_mut().register::<Marker>();

    // First spawns one Marker per frame (visible to later stages/frames).
    app.add_system(
        ScheduleLabel::First,
        |_q: Query<'_, &Marker, ()>, c: &mut Commands<'_>| {
            c.spawn_with(Marker);
            Ok(())
        },
    );
    // Update writes a Ping; Last reads it the same frame and tallies.
    app.add_system(
        ScheduleLabel::Update,
        |mut w: EventWriter<'_, Ping>, _c: &mut Commands<'_>| {
            w.send(Ping);
            Ok(())
        },
    );
    app.add_system(
        ScheduleLabel::Last,
        |mut r: EventReader<'_, '_, Ping>, mut reads: ResMut<'_, Reads>, _c: &mut Commands<'_>| {
            reads.0 += r.read().count() as u32;
            Ok(())
        },
    );

    let mut engine = app.build_headless().unwrap();
    for _ in 0..5 {
        engine.tick().unwrap();
    }

    assert_eq!(engine.world().query::<&Marker>().count(), 5);
    assert_eq!(
        engine.world().get_resource::<Reads>().unwrap().0,
        5,
        "each frame's event is read exactly once — no loss, no leak"
    );
}

#[test]
fn run_headless_reaches_clean_shutdown() {
    assert!(App::new().run_headless(3).is_ok());
}

#[test]
fn nonpositive_fixed_timestep_is_invalid_config() {
    use spawn_engine::{EngineConfig, EngineError};
    let mut app = App::new();
    app.set_config(EngineConfig {
        fixed_timestep: 0.0,
        ..Default::default()
    });
    assert!(matches!(
        app.run_headless(1),
        Err(EngineError::InvalidConfig { .. })
    ));
}
