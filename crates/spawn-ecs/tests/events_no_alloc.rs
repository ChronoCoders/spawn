//! Isolated allocation-counting test for events: the second `Schedule::run` of a
//! writer + reader at steady event volume performs zero heap allocations. Kept in
//! its own test binary (sole test) so a parallel sibling test can never pollute
//! the process-global allocation counter.

use spawn_ecs::{Commands, EventReader, EventWriter, Schedule, Stage, World};
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
struct Hit(u32);
impl spawn_ecs::Event for Hit {}

#[test]
fn second_run_with_events_is_allocation_free() {
    let mut world = World::new();
    world.init_event::<Hit>();

    let mut schedule = Schedule::new();
    let mut stage = Stage::new("frame");
    stage.add_system(|mut w: EventWriter<'_, Hit>, _c: &mut Commands<'_>| {
        for i in 0..16 {
            w.send(Hit(i));
        }
        Ok(())
    });
    stage.add_system(|mut r: EventReader<'_, '_, Hit>, _c: &mut Commands<'_>| {
        let mut acc = 0u32;
        for h in r.read() {
            acc = acc.wrapping_add(h.0);
        }
        std::hint::black_box(acc);
        Ok(())
    });
    schedule.add_stage(stage);
    schedule.build(&world).unwrap();

    // Warm up several frames so both double buffers reach steady capacity.
    for _ in 0..4 {
        schedule.run(&mut world).unwrap();
    }

    ALLOCS.store(0, Ordering::Relaxed);
    ARMED.store(true, Ordering::Relaxed);
    schedule.run(&mut world).unwrap();
    ARMED.store(false, Ordering::Relaxed);

    assert_eq!(
        ALLOCS.load(Ordering::Relaxed),
        0,
        "second Schedule::run with events must not allocate at steady volume"
    );
}
