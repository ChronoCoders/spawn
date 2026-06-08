//! Isolated allocation-counting test for resources: the second `Schedule::run`
//! of systems taking `Res`/`ResMut` performs zero heap allocations. Kept in its
//! own test binary (sole test) so a parallel sibling test can never pollute the
//! process-global allocation counter.

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

#[derive(Debug, Default)]
struct Sum(i64);
impl Resource for Sum {}

#[derive(Debug, Default)]
struct Ticks(u32);
impl Resource for Ticks {}

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
    // Reader of Sum (conflicts with the writer → separate inline batch); writes Ticks.
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
