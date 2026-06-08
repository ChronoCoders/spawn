//! Events as observed through the public API: scheduled writers/readers,
//! per-frame double-buffer lifetime and swap cadence, deterministic delivery
//! order, and the build-time error for an uninitialized event type. The
//! allocation-counting test lives in `events_no_alloc.rs` (its own binary).

use spawn_ecs::{Commands, Component, EventReader, EventWriter, Schedule, Stage, World};

#[derive(Debug, Clone, Copy, PartialEq)]
struct Hit(u32);
impl spawn_ecs::Event for Hit {}

#[derive(Debug, Clone, Copy)]
struct Tick;
impl Component for Tick {}

#[test]
fn writer_then_reader_across_stages_same_frame() {
    // Single end-of-frame swap: an event sent in an early stage is visible to a
    // reader in a later stage of the same frame.
    let mut world = World::new();
    world.init_event::<Hit>();

    let mut schedule = Schedule::new();
    let mut s1 = Stage::new("emit");
    s1.add_system(|mut w: EventWriter<'_, Hit>, _c: &mut Commands<'_>| {
        w.send(Hit(1));
        w.send(Hit(2));
        Ok(())
    });
    let mut s2 = Stage::new("consume");
    s2.add_system(|mut r: EventReader<'_, '_, Hit>, _c: &mut Commands<'_>| {
        let got: Vec<u32> = r.read().map(|h| h.0).collect();
        assert_eq!(got, vec![1, 2]);
        Ok(())
    });
    schedule.add_stage(s1);
    schedule.add_stage(s2);
    schedule.build(&world).unwrap();
    schedule.run(&mut world).unwrap();
}

#[test]
fn reader_consumes_each_event_once_across_frames() {
    let mut world = World::new();
    world.init_event::<Hit>();

    // A resource counting how many events the reader saw, summed over frames.
    #[derive(Default)]
    struct Seen(u32);
    impl spawn_ecs::Resource for Seen {}
    world.insert_resource(Seen::default());

    let mut schedule = Schedule::new();
    let mut stage = Stage::new("frame");
    // Emit exactly one event per frame.
    stage.add_system(|mut w: EventWriter<'_, Hit>, _c: &mut Commands<'_>| {
        w.send(Hit(9));
        Ok(())
    });
    // Reader tallies what it sees; conflicts with the writer (write vs read), so
    // it serializes after it and sees this frame's event.
    stage.add_system(
        |mut r: EventReader<'_, '_, Hit>,
         mut seen: spawn_ecs::ResMut<'_, Seen>,
         _c: &mut Commands<'_>| {
            seen.0 += r.read().count() as u32;
            Ok(())
        },
    );
    schedule.add_stage(stage);
    schedule.build(&world).unwrap();

    for _ in 0..5 {
        schedule.run(&mut world).unwrap();
    }
    // One event per frame, each seen exactly once: 5 frames -> 5.
    assert_eq!(world.get_resource::<Seen>().unwrap().0, 5);
}

#[test]
fn deterministic_serialized_writer_order() {
    let reference = run_two_writers();
    for _ in 0..200 {
        assert_eq!(run_two_writers(), reference);
    }
    assert_eq!(reference, vec![10, 20]);
}

#[derive(Default)]
struct Collected(Vec<u32>);
impl spawn_ecs::Resource for Collected {}

fn run_two_writers() -> Vec<u32> {
    let mut world = World::new();
    world.init_event::<Hit>();
    world.insert_resource(Collected::default());
    let mut schedule = Schedule::new();
    let mut stage = Stage::new("emit");
    // Two writers of the same event type conflict and serialize in registration
    // order, so the appended order is deterministic.
    stage.add_system(|mut w: EventWriter<'_, Hit>, _c: &mut Commands<'_>| {
        w.send(Hit(10));
        Ok(())
    });
    stage.add_system(|mut w: EventWriter<'_, Hit>, _c: &mut Commands<'_>| {
        w.send(Hit(20));
        Ok(())
    });
    // Reader conflicts with both writers (read vs write Events<Hit>), so it
    // serializes last and observes this frame's events in send order.
    stage.add_system(
        |mut r: EventReader<'_, '_, Hit>,
         mut out: spawn_ecs::ResMut<'_, Collected>,
         _c: &mut Commands<'_>| {
            out.0.clear();
            out.0.extend(r.read().map(|h| h.0));
            Ok(())
        },
    );
    schedule.add_stage(stage);
    schedule.build(&world).unwrap();
    schedule.run(&mut world).unwrap();

    let out = world.get_resource::<Collected>().unwrap().0.clone();
    out
}

#[test]
fn uninitialized_event_build_errors() {
    let mut world = World::new();
    world.spawn_with(Tick);
    let mut schedule = Schedule::new();
    let mut stage = Stage::new("s");
    stage.add_system(|mut _w: EventWriter<'_, Hit>, _c: &mut Commands<'_>| Ok(()));
    schedule.add_stage(stage);
    assert!(matches!(
        schedule.build(&world),
        Err(spawn_ecs::EcsError::EventsNotInitialized { .. })
    ));
}

#[test]
fn init_event_is_idempotent() {
    let mut world = World::new();
    world.init_event::<Hit>();
    {
        let mut events = world.get_resource_mut::<spawn_ecs::Events<Hit>>().unwrap();
        events.send(Hit(1));
    }
    // A second init must not reset the existing buffer.
    world.init_event::<Hit>();
    assert_eq!(
        world
            .get_resource::<spawn_ecs::Events<Hit>>()
            .unwrap()
            .len(),
        1
    );
}
