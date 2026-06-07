//! Asserts the profiler's steady-state per-frame path performs no heap
//! allocation after warm-up.
//!
//! A counting global allocator records allocations, but only while a
//! thread-local guard is armed on the current thread. The guard is armed
//! strictly around the `begin_frame`/`profile_scope!`/`end_frame` path on the
//! profiler's owning thread, so neither the test harness nor other threads
//! contribute. Warm-up fills the history ring and the node pool so the measured
//! loop draws every buffer from the pool.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use spawn_debug::profile::ScopeGuard;
use spawn_debug::{profile_scope, Profiler, ProfilerConfig};

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
}

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

struct Counting;

// SAFETY: delegates every allocation to the system allocator unchanged; the only
// added behavior is a relaxed counter bump when the calling thread armed the
// guard. No pointers are fabricated and layouts are forwarded verbatim.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ARMED.try_with(|a| a.get()).unwrap_or(false) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if ARMED.try_with(|a| a.get()).unwrap_or(false) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn run_frame(profiler: &mut Profiler) {
    profiler.begin_frame();
    {
        profile_scope!("update");
        {
            profile_scope!("physics");
            {
                let _broad = ScopeGuard::enter("broadphase");
            }
            {
                let _narrow = ScopeGuard::enter("narrowphase");
            }
        }
        {
            profile_scope!("render");
            for _ in 0..3 {
                // Same-name siblings merge: exercises the recycle path.
                let _draw = ScopeGuard::enter("draw");
            }
        }
    }
    profiler.end_frame();
}

#[test]
fn steady_state_frames_do_not_allocate() {
    let mut profiler = Profiler::new(ProfilerConfig {
        history_len: 8,
        stats_window: 16,
    });

    // Warm up: fill the history ring (so eviction recycling kicks in), fill the
    // rolling-stats windows, and populate the node/buffer pool.
    for _ in 0..64 {
        run_frame(&mut profiler);
    }

    ARMED.with(|a| a.set(true));
    for _ in 0..1_000 {
        run_frame(&mut profiler);
        let _ = profiler.last_report();
        let _ = profiler.history().len();
        let _ = profiler.frame_index();
    }
    ARMED.with(|a| a.set(false));

    let allocs = ALLOCS.load(Ordering::Relaxed);
    assert_eq!(
        allocs, 0,
        "steady-state frame path allocated {allocs} times"
    );
}
