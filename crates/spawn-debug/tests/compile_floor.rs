//! Compile-time level floor: macros above `COMPILE_MAX_LEVEL` expand to nothing
//! and never evaluate their arguments. This test adapts to whichever
//! `max_level_*` feature is active (CI runs it under several). Under the default
//! `max_level_trace` nothing is stripped; build with e.g.
//! `--no-default-features --features max_level_info` to exercise stripping of
//! `spawn_debug!`/`spawn_trace!`.

use spawn_debug::log::{LogLevel, COMPILE_MAX_LEVEL, COMPILE_OFF};
use spawn_debug::{spawn_debug, spawn_trace};
use std::sync::atomic::{AtomicUsize, Ordering};

static SIDE_EFFECTS: AtomicUsize = AtomicUsize::new(0);

fn tick() -> u32 {
    SIDE_EFFECTS.fetch_add(1, Ordering::Relaxed);
    0
}

#[test]
fn stripped_levels_do_not_evaluate_arguments() {
    // These macros reference `tick()` as a format argument. If the level is
    // above the compile floor, the macro expands to nothing and `tick()` is
    // never called; otherwise the runtime floor (uninitialized => Info default,
    // but here the logger may be uninitialized so `enabled` uses the atomic
    // default Info) may still gate it. To isolate compile-time stripping we only
    // assert the zero-evaluation case for levels strictly above the floor.

    SIDE_EFFECTS.store(0, Ordering::Relaxed);

    let debug_stripped = COMPILE_OFF || LogLevel::Debug as u8 > COMPILE_MAX_LEVEL as u8;
    let trace_stripped = COMPILE_OFF || LogLevel::Trace as u8 > COMPILE_MAX_LEVEL as u8;

    spawn_debug!("debug arg {}", tick());
    spawn_trace!("trace arg {}", tick());

    let mut expected_min_calls = 0;
    if !debug_stripped {
        // Not stripped at compile time; the runtime gate may still drop it, but
        // the argument IS evaluated before the enabled check only inside the
        // taken branch — our macro evaluates args lazily inside the enabled
        // block, so a runtime-disabled level also avoids evaluation. Thus a
        // non-stripped-but-runtime-disabled level contributes zero too.
        let _ = &mut expected_min_calls;
    }
    let _ = trace_stripped;

    // The strong invariant: a level stripped at compile time evaluates zero args.
    if debug_stripped && trace_stripped {
        assert_eq!(SIDE_EFFECTS.load(Ordering::Relaxed), 0);
    }

    // Always sound: total evaluations never exceed the number of non-stripped
    // macros (each evaluates at most once, and only when enabled).
    let max_possible = (!debug_stripped as usize) + (!trace_stripped as usize);
    assert!(SIDE_EFFECTS.load(Ordering::Relaxed) <= max_possible);
}
