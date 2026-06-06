//! Scheduler determinism, command deferral, and conflict-batching tests.

use spawn_ecs::{Commands, Component, Query, Schedule, Stage, World};

#[derive(Debug, Clone, Copy, PartialEq)]
struct A(u64);
#[derive(Debug, Clone, Copy, PartialEq)]
struct B(u64);
#[derive(Debug, Clone, Copy, PartialEq)]
struct Counter(u64);

impl Component for A {}
impl Component for B {}
impl Component for Counter {}

fn world_with_entities(n: u64) -> World {
    let mut world = World::new();
    for i in 0..n {
        world.spawn_with((A(i), B(0), Counter(0)));
    }
    world
}

#[test]
fn command_changes_invisible_until_stage_boundary() {
    let mut world = World::new();
    world.register::<A>();

    let mut schedule = Schedule::new();
    let mut stage = Stage::new("spawn_stage");
    stage.add_system(|q: Query<'_, &A, ()>, commands: &mut Commands<'_>| {
        // No A entities exist yet at run time.
        assert_eq!(q.count(), 0);
        commands.spawn_with(A(7));
        Ok(())
    });
    schedule.add_stage(stage);
    schedule.build(&world).unwrap();
    schedule.run(&mut world).unwrap();

    // After the stage boundary, the spawned entity is live.
    assert_eq!(world.query::<&A>().count(), 1);
    assert_eq!(world.query::<&A>().iter().next().unwrap(), &A(7));
}

#[test]
fn deferred_ops_on_dead_entities_are_dropped() {
    let mut world = World::new();
    world.register::<A>();
    let dead = world.spawn();
    world.despawn(dead).unwrap();

    let mut cmds = world.commands();
    cmds.insert(dead, A(1));
    cmds.despawn(dead);
    world.apply_commands();
    // No panic, no error, nothing created.
    assert_eq!(world.entity_count(), 0);
}

#[test]
fn reserved_ids_never_collide_across_systems() {
    let mut world = World::new();
    world.register::<A>();

    let mut schedule = Schedule::new();
    let mut stage = Stage::new("s");
    // Two non-conflicting systems each spawn; reserved ids must be unique.
    for _ in 0..2 {
        stage.add_system(|_q: Query<'_, &A, ()>, commands: &mut Commands<'_>| {
            commands.spawn_with(A(1));
            Ok(())
        });
    }
    schedule.add_stage(stage);
    schedule.build(&world).unwrap();
    schedule.run(&mut world).unwrap();
    assert_eq!(world.query::<&A>().count(), 2);
}

#[test]
fn command_application_is_registration_then_insertion_order() {
    let mut world = World::new();
    world.register::<Counter>();
    let e = world.spawn_with(Counter(0));

    let mut schedule = Schedule::new();
    let mut stage = Stage::new("s");
    // Conflicting writers serialize into ordered batches; both remove+insert
    // Counter, last writer wins deterministically by registration order.
    stage.add_system(
        move |_q: Query<'_, &mut Counter, ()>, commands: &mut Commands<'_>| {
            commands.insert(e, Counter(10));
            Ok(())
        },
    );
    stage.add_system(
        move |_q: Query<'_, &mut Counter, ()>, commands: &mut Commands<'_>| {
            commands.insert(e, Counter(20));
            Ok(())
        },
    );
    schedule.add_stage(stage);
    schedule.build(&world).unwrap();
    schedule.run(&mut world).unwrap();
    // Second-registered system's insert applies last.
    assert_eq!(*world.get::<Counter>(e).unwrap(), Counter(20));
}

#[test]
fn parallel_determinism_stress() {
    // Many non-conflicting readers + deferred writers; result must be identical
    // across repeated runs regardless of thread scheduling.
    let reference = run_once();
    for _ in 0..200 {
        assert_eq!(run_once(), reference);
    }
}

fn run_once() -> Vec<u64> {
    let mut world = world_with_entities(64);
    world.register::<A>();
    world.register::<B>();

    let mut schedule = Schedule::new();
    let mut stage = Stage::new("update");
    // Reader of A, writer-via-commands of B-bearing new entities. Two readers of
    // A share a batch; they only read, so order is irrelevant.
    stage.add_system(|q: Query<'_, &A, ()>, commands: &mut Commands<'_>| {
        for a in q.iter() {
            if a.0 % 2 == 0 {
                commands.spawn_with(B(a.0));
            }
        }
        Ok(())
    });
    stage.add_system(|q: Query<'_, &A, ()>, _commands: &mut Commands<'_>| {
        // Pure reader; shares the batch with the system above only if
        // non-conflicting (both read A, defer writes) — they do share.
        let _sum: u64 = q.iter().map(|a| a.0).sum();
        Ok(())
    });
    schedule.add_stage(stage);
    schedule.build(&world).unwrap();
    schedule.run(&mut world).unwrap();

    let mut result: Vec<u64> = world.query::<&B>().iter().map(|b| b.0).collect();
    result.sort_unstable();
    result
}

#[test]
fn conflicting_writers_serialize_readers_share() {
    // Build a stage; verify batch layout indirectly: two writers of the same
    // component cannot run concurrently, but the schedule still produces a
    // correct, deterministic result.
    let mut world = world_with_entities(8);
    world.register::<A>();

    let mut schedule = Schedule::new();
    let mut stage = Stage::new("s");
    stage.add_system(|q: Query<'_, &mut A, ()>, _c: &mut Commands<'_>| {
        let _ = q.count();
        Ok(())
    });
    stage.add_system(|q: Query<'_, &mut A, ()>, _c: &mut Commands<'_>| {
        let _ = q.count();
        Ok(())
    });
    stage.add_system(|q: Query<'_, &A, ()>, _c: &mut Commands<'_>| {
        let _ = q.count();
        Ok(())
    });
    schedule.add_stage(stage);
    schedule.build(&world).unwrap();
    // Runs without deadlock or data race (validated under --test-threads too).
    schedule.run(&mut world).unwrap();
    assert_eq!(world.query::<&A>().count(), 8);
}

#[test]
fn multi_stage_observes_previous_stage_changes() {
    let mut world = World::new();
    world.register::<A>();

    let mut schedule = Schedule::new();
    let mut s1 = Stage::new("spawn");
    s1.add_system(|_q: Query<'_, &A, ()>, c: &mut Commands<'_>| {
        c.spawn_with(A(1));
        Ok(())
    });
    let mut s2 = Stage::new("observe");
    s2.add_system(|q: Query<'_, &A, ()>, _c: &mut Commands<'_>| {
        // Stage 1's spawn is visible here.
        assert_eq!(q.count(), 1);
        Ok(())
    });
    schedule.add_stage(s1);
    schedule.add_stage(s2);
    schedule.build(&world).unwrap();
    schedule.run(&mut world).unwrap();
    assert_eq!(world.query::<&A>().count(), 1);
}
