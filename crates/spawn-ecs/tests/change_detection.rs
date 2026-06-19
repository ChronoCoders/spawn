use spawn_ecs::{
    Added, Changed, Commands, Component, Query, ResMut, Resource, Schedule, Stage, World,
};

#[derive(Clone, Copy)]
struct A(u32);
impl Component for A {}

#[derive(Clone, Copy)]
struct B;
impl Component for B {}

#[derive(Default)]
struct Seen(usize);
impl Resource for Seen {}

#[test]
fn added_is_seen_the_first_frame_then_not() {
    let mut world = World::new();
    world.insert_resource(Seen::default());
    world.spawn_with((A(1),));

    let mut schedule = Schedule::new();
    let mut stage = Stage::new("read");
    stage.add_system(
        |q: Query<'_, &A, Added<A>>, mut seen: ResMut<'_, Seen>, _c: &mut Commands<'_>| {
            seen.0 += q.iter().count();
            Ok(())
        },
    );
    schedule.add_stage(stage);
    schedule.build(&world).unwrap();

    schedule.run(&mut world).unwrap();
    assert_eq!(world.get_resource::<Seen>().unwrap().0, 1);

    schedule.run(&mut world).unwrap();
    assert_eq!(world.get_resource::<Seen>().unwrap().0, 1);
}

#[test]
fn changed_is_seen_after_a_mutation_in_a_later_frame() {
    let mut world = World::new();
    world.insert_resource(Seen::default());
    let e = world.spawn_with((A(1),));

    let mut schedule = Schedule::new();
    let mut stage = Stage::new("read");
    stage.add_system(
        |q: Query<'_, &A, Changed<A>>, mut seen: ResMut<'_, Seen>, _c: &mut Commands<'_>| {
            seen.0 += q.iter().count();
            Ok(())
        },
    );
    schedule.add_stage(stage);
    schedule.build(&world).unwrap();

    world.increment_change_tick();
    schedule.run_stages(&mut world).unwrap();
    assert_eq!(world.get_resource::<Seen>().unwrap().0, 1);

    world.increment_change_tick();
    schedule.run_stages(&mut world).unwrap();
    assert_eq!(world.get_resource::<Seen>().unwrap().0, 1);

    world.increment_change_tick();
    world.get_mut::<A>(e).unwrap().0 = 9;
    schedule.run_stages(&mut world).unwrap();
    assert_eq!(world.get_resource::<Seen>().unwrap().0, 2);
}

#[test]
fn added_implies_changed_via_direct_query() {
    let mut world = World::new();
    let e = world.spawn_with((A(1),));
    world.increment_change_tick();
    world.get_mut::<A>(e).unwrap().0 = 2;
    assert_eq!(world.query::<&A>().changed::<A>().iter().count(), 1);
    assert_eq!(world.query::<&A>().added::<A>().iter().count(), 1);
}

#[test]
fn direct_queries_see_every_row_as_changed_and_added() {
    let mut world = World::new();
    world.spawn_with((A(1),));
    world.spawn_with((A(2), B));
    assert_eq!(world.query::<&A>().added::<A>().iter().count(), 2);
    assert_eq!(world.query::<&A>().changed::<A>().iter().count(), 2);
}

#[test]
fn unfiltered_queries_are_unaffected() {
    let mut world = World::new();
    for i in 0..16 {
        world.spawn_with((A(i),));
    }
    assert_eq!(world.query::<&A>().iter().count(), 16);
    let sum: u32 = world.query::<&A>().iter().map(|a| a.0).sum();
    assert_eq!(sum, (0..16).sum());
}

#[test]
fn changed_reader_count_is_deterministic_across_runs() {
    let run_once = || {
        let mut world = World::new();
        world.insert_resource(Seen::default());
        for i in 0..64 {
            world.spawn_with((A(i),));
        }
        let mut schedule = Schedule::new();
        let mut stage = Stage::new("read");
        stage.add_system(
            |q: Query<'_, &A, Changed<A>>, mut seen: ResMut<'_, Seen>, _c: &mut Commands<'_>| {
                seen.0 += q.iter().count();
                Ok(())
            },
        );
        schedule.add_stage(stage);
        schedule.build(&world).unwrap();
        schedule.run(&mut world).unwrap();
        schedule.run(&mut world).unwrap();
        let total = world.get_resource::<Seen>().unwrap().0;
        total
    };
    let first = run_once();
    for _ in 0..16 {
        assert_eq!(run_once(), first);
    }
    assert_eq!(first, 64);
}

#[test]
fn separate_component_change_does_not_flag_others() {
    let mut world = World::new();
    let e = world.spawn_with((A(1), B));
    world.insert_resource(Seen::default());

    let mut schedule = Schedule::new();
    let mut stage = Stage::new("read");
    stage.add_system(
        |q: Query<'_, &A, Changed<B>>, mut seen: ResMut<'_, Seen>, _c: &mut Commands<'_>| {
            seen.0 += q.iter().count();
            Ok(())
        },
    );
    schedule.add_stage(stage);
    schedule.build(&world).unwrap();

    world.increment_change_tick();
    schedule.run_stages(&mut world).unwrap();
    let base = world.get_resource::<Seen>().unwrap().0;

    world.increment_change_tick();
    world.get_mut::<A>(e).unwrap().0 = 2;
    schedule.run_stages(&mut world).unwrap();
    assert_eq!(world.get_resource::<Seen>().unwrap().0, base);
}
