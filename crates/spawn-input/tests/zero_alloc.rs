//! Asserts the per-frame hot path performs no heap allocation after setup.
//!
//! A counting global allocator records allocations, but only while a
//! thread-local guard is armed on the current thread. The guard is armed
//! strictly around the `begin_frame`/`process`/`update`/query path, so neither
//! the test harness nor other threads contribute to the count. The gamepad
//! backend is inert in test builds (no `gilrs`), so this measures only the
//! crate's own per-frame work.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use spawn_input::{Action, ActionMap, Binding, InputState};
use spawn_platform::{ButtonState, KeyCode, KeyboardEvent, MouseButton, MouseEvent, PlatformEvent};

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
}

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

struct Counting;

// SAFETY: every method forwards verbatim to the std `System` allocator, which is a
// correct `GlobalAlloc` implementation; the only added work is an atomic counter bump
// that never touches the pointer or layout. The `unsafe` is required solely because the
// `GlobalAlloc` trait methods are themselves `unsafe`, so all memory-safety obligations
// are discharged by `System`.
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

fn key_event(key: KeyCode, pressed: bool) -> PlatformEvent {
    PlatformEvent::Keyboard(KeyboardEvent {
        key,
        state: if pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        },
        repeat: false,
    })
}

#[test]
fn per_frame_path_does_not_allocate() {
    let mut input = InputState::new().expect("init");
    let mut map: ActionMap<Action> = ActionMap::new();
    let fire = Action(0);
    map.bind(fire, Binding::Key(KeyCode::Space));
    map.bind(fire, Binding::MouseButton(MouseButton::Left));

    let events = [
        key_event(KeyCode::Space, true),
        PlatformEvent::Mouse(MouseEvent::Moved { x: 1.0, y: 2.0 }),
        PlatformEvent::Mouse(MouseEvent::Button {
            button: MouseButton::Left,
            state: ButtonState::Pressed,
        }),
    ];

    for _ in 0..4 {
        input.begin_frame();
        for ev in &events {
            input.process(ev);
        }
        map.update(&input);
        let _ = map.pressed(fire);
    }

    ARMED.with(|a| a.set(true));
    for _ in 0..1_000 {
        input.begin_frame();
        for ev in &events {
            input.process(ev);
        }
        map.update(&input);
        let _ = map.value(fire);
        let _ = map.pressed(fire);
        let _ = map.just_pressed(fire);
        let _ = input.keyboard().is_pressed(KeyCode::Space);
        let _ = input.mouse().delta();
        let _ = input.touch().count();
        let _ = input.gamepads().first();
    }
    ARMED.with(|a| a.set(false));

    let allocs = ALLOCS.load(Ordering::Relaxed);
    assert_eq!(allocs, 0, "per-frame path allocated {allocs} times");
}
