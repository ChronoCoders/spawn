//! Verifies the hot path allocates nothing after warm-up: a second
//! `Schedule::run` performs zero heap allocations.

use spawn_ecs::{Commands, Component, Query, Schedule, Stage, World};
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
struct Position(i64);
#[derive(Debug, Clone, Copy)]
struct Velocity(i64);

impl Component for Position {}
impl Component for Velocity {}

#[test]
fn second_run_is_allocation_free() {
    let mut world = World::new();
    for i in 0..256 {
        world.spawn_with((Position(i), Velocity(1)));
    }

    let mut schedule = Schedule::new();
    let mut stage = Stage::new("update");
    // Pure read systems iterating columns; structural changes deferred but none
    // emitted here, so the steady-state run touches no allocating path.
    stage.add_system(
        |q: Query<'_, (&Position, &Velocity), ()>, _c: &mut Commands<'_>| {
            let mut acc = 0i64;
            for (p, v) in q.iter() {
                acc = acc.wrapping_add(p.0 + v.0);
            }
            std::hint::black_box(acc);
            Ok(())
        },
    );
    schedule.add_stage(stage);
    schedule.build(&world).unwrap();

    // Warm-up run sizes all buffers/masks.
    schedule.run(&mut world).unwrap();

    ALLOCS.store(0, Ordering::Relaxed);
    ARMED.store(true, Ordering::Relaxed);
    schedule.run(&mut world).unwrap();
    ARMED.store(false, Ordering::Relaxed);

    assert_eq!(
        ALLOCS.load(Ordering::Relaxed),
        0,
        "second Schedule::run must not allocate"
    );
}
