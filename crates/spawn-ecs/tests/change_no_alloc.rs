use spawn_ecs::{Changed, Commands, Component, Query, ResMut, Resource, Schedule, Stage, World};
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

#[derive(Clone, Copy)]
struct Position(i64);

impl Component for Position {}

#[derive(Default)]
struct Sum(i64);

impl Resource for Sum {}

#[test]
fn changed_filtered_run_is_allocation_free() {
    let mut world = World::new();
    world.insert_resource(Sum::default());
    for i in 0..256 {
        world.spawn_with((Position(i),));
    }

    let mut schedule = Schedule::new();
    let mut stage = Stage::new("update");
    stage.add_system(
        |q: Query<'_, &Position, Changed<Position>>,
         mut sum: ResMut<'_, Sum>,
         _c: &mut Commands<'_>| {
            let mut acc = 0i64;
            for p in q.iter() {
                acc = acc.wrapping_add(p.0);
            }
            sum.0 = acc;
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
        "second Schedule::run with a Changed filter must not allocate"
    );
}
