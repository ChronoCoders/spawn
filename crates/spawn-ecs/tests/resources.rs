//! Resource storage as observed through the public API: scheduled access via
//! `Res`/`ResMut`, deferred resource commands, determinism, the no-allocation
//! hot path, and the build-time error for an unregistered resource.

use spawn_ecs::{Commands, Component, Query, Res, ResMut, Resource, Schedule, Stage, World};
use std::alloc::{GlobalAlloc, Layout, System as SystemAlloc};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct CountingAlloc;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static ARMED: AtomicBool = AtomicBool::new(false);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ARMED.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        SystemAlloc.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        SystemAlloc.dealloc(ptr, layout);
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

#[derive(Debug, Clone, Copy)]
struct Value(i64);
impl Component for Value {}

#[derive(Debug, Default, PartialEq)]
struct Sum(i64);
impl Resource for Sum {}

#[derive(Debug, Default)]
struct Ticks(u32);
impl Resource for Ticks {}

#[test]
fn direct_world_access_roundtrip() {
    let mut world = World::new();
    assert!(world.get_resource::<Sum>().is_none());
    world.insert_resource(Sum(3));
    assert!(world.contains_resource::<Sum>());
    assert_eq!(world.get_resource::<Sum>().unwrap().0, 3);
    world.get_resource_mut::<Sum>().unwrap().0 = 10;
    assert_eq!(world.get_resource::<Sum>().unwrap().0, 10);
    assert_eq!(world.remove_resource::<Sum>().unwrap().0, 10);
    assert!(!world.contains_resource::<Sum>());
}

#[test]
fn scheduled_resmut_mutation_is_correct_and_deterministic() {
    let reference = run_accumulate();
    for _ in 0..200 {
        assert_eq!(run_accumulate(), reference);
    }
    // 0 + 1 + ... + 63 = 2016.
    assert_eq!(reference, 2016);
}

fn run_accumulate() -> i64 {
    let mut world = World::new();
    for i in 0..64 {
        world.spawn_with(Value(i));
    }
    world.insert_resource(Sum::default());
    world.insert_resource(Ticks::default());

    let mut schedule = Schedule::new();
    let mut stage = Stage::new("update");
    // Writer of Sum (reads Value); a parallel pure reader of Value shares the
    // batch (read-read on Value, and it does not touch Sum).
    stage.add_system(
        |q: Query<'_, &Value, ()>, mut sum: ResMut<'_, Sum>, _c: &mut Commands<'_>| {
            for v in q.iter() {
                sum.0 += v.0;
            }
            Ok(())
        },
    );
    stage.add_system(|q: Query<'_, &Value, ()>, _c: &mut Commands<'_>| {
        let _total: i64 = q.iter().map(|v| v.0).sum();
        Ok(())
    });
    // A reader of Sum conflicts with the writer above, so it serializes after it
    // and observes the final value via Ticks.
    stage.add_system(
        |_q: Query<'_, &Value, ()>,
         sum: Res<'_, Sum>,
         mut ticks: ResMut<'_, Ticks>,
         _c: &mut Commands<'_>| {
            ticks.0 = sum.0 as u32;
            Ok(())
        },
    );
    schedule.add_stage(stage);
    schedule.build(&world).unwrap();
    schedule.run(&mut world).unwrap();

    let sum = world.get_resource::<Sum>().unwrap().0;
    assert_eq!(world.get_resource::<Ticks>().unwrap().0, sum as u32);
    sum
}

#[test]
fn deferred_resource_ops_apply_at_stage_boundary() {
    let mut world = World::new();
    world.register::<Value>();
    // Pre-register Sum so stage 2's reader resolves at build time; the deferred
    // command then overwrites it at the stage-1 boundary.
    world.insert_resource(Sum(0));

    let mut schedule = Schedule::new();
    let mut s1 = Stage::new("insert");
    s1.add_system(|_q: Query<'_, &Value, ()>, c: &mut Commands<'_>| {
        c.insert_resource(Sum(42));
        Ok(())
    });
    let mut s2 = Stage::new("observe");
    s2.add_system(
        |_q: Query<'_, &Value, ()>, sum: Res<'_, Sum>, _c: &mut Commands<'_>| {
            // Stage 1's deferred insert is visible here, after the stage boundary.
            assert_eq!(sum.0, 42);
            Ok(())
        },
    );
    schedule.add_stage(s1);
    schedule.add_stage(s2);
    schedule.build(&world).unwrap();
    schedule.run(&mut world).unwrap();
    assert_eq!(world.get_resource::<Sum>().unwrap().0, 42);

    // Deferred remove via the world-level command buffer.
    world.commands().remove_resource::<Sum>();
    world.apply_commands();
    assert!(!world.contains_resource::<Sum>());
}

#[test]
fn deferred_insert_last_writer_wins_by_registration_order() {
    let mut world = World::new();
    world.register::<Value>();
    let mut schedule = Schedule::new();
    let mut stage = Stage::new("s");
    stage.add_system(|_q: Query<'_, &Value, ()>, c: &mut Commands<'_>| {
        c.insert_resource(Sum(1));
        Ok(())
    });
    stage.add_system(|_q: Query<'_, &Value, ()>, c: &mut Commands<'_>| {
        c.insert_resource(Sum(2));
        Ok(())
    });
    schedule.add_stage(stage);
    schedule.build(&world).unwrap();
    schedule.run(&mut world).unwrap();
    // Second-registered system's insert applies last.
    assert_eq!(world.get_resource::<Sum>().unwrap().0, 2);
}

#[test]
fn existing_query_only_systems_still_compile_and_run() {
    // Backward compatibility: the Phase 1 system shapes are unchanged.
    let mut world = World::new();
    world.spawn_with(Value(5));
    let mut schedule = Schedule::new();
    let mut stage = Stage::new("s");
    stage.add_system(|q: Query<'_, &Value, ()>, _c: &mut Commands<'_>| {
        assert_eq!(q.count(), 1);
        Ok(())
    });
    stage.add_system(|_c: &mut Commands<'_>| Ok(()));
    schedule.add_stage(stage);
    schedule.build(&world).unwrap();
    schedule.run(&mut world).unwrap();
}

#[test]
fn unregistered_resource_build_errors() {
    let mut world = World::new();
    world.register::<Value>();
    let mut schedule = Schedule::new();
    let mut stage = Stage::new("s");
    stage.add_system(|_q: Query<'_, &Value, ()>, _sum: Res<'_, Sum>, _c: &mut Commands<'_>| Ok(()));
    schedule.add_stage(stage);
    assert!(matches!(
        schedule.build(&world),
        Err(spawn_ecs::EcsError::ResourceNotRegistered { .. })
    ));
}

#[test]
fn second_run_with_resources_is_allocation_free() {
    let mut world = World::new();
    for i in 0..128 {
        world.spawn_with(Value(i));
    }
    world.insert_resource(Sum::default());

    world.insert_resource(Ticks::default());
    let mut schedule = Schedule::new();
    let mut stage = Stage::new("update");
    // Writer of Sum (also reads Value).
    stage.add_system(
        |q: Query<'_, &Value, ()>, mut sum: ResMut<'_, Sum>, _c: &mut Commands<'_>| {
            let mut acc = 0i64;
            for v in q.iter() {
                acc = acc.wrapping_add(v.0);
            }
            sum.0 = acc;
            Ok(())
        },
    );
    // Reader of Sum (conflicts with the writer → separate batch); writes Ticks.
    stage.add_system(
        |sum: Res<'_, Sum>, mut ticks: ResMut<'_, Ticks>, _c: &mut Commands<'_>| {
            ticks.0 = sum.0 as u32;
            Ok(())
        },
    );
    schedule.add_stage(stage);
    schedule.build(&world).unwrap();
    schedule.run(&mut world).unwrap();

    ALLOCS.store(0, Ordering::Relaxed);
    ARMED.store(true, Ordering::Relaxed);
    schedule.run(&mut world).unwrap();
    ARMED.store(false, Ordering::Relaxed);

    assert_eq!(
        ALLOCS.load(Ordering::Relaxed),
        0,
        "second Schedule::run with Res/ResMut must not allocate"
    );
}
