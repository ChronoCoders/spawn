//! Asserts the steady-state layout + draw-list path performs no heap allocation.
//!
//! A counting global allocator records allocations only while a thread-local
//! guard is armed, so only the measured calls contribute. After a warm-up that
//! computes layout and builds the draw list once, an unchanged tree with the
//! same `root_size` must re-run both with zero allocations.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use spawn_core::Vec2;
use spawn_ui::{Dimension, DrawList, Size, Style, TextMeasure, UiTree};

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
}

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

struct Counting;

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

struct ZeroMeasure;
impl TextMeasure for ZeroMeasure {
    fn measure(&mut self, _t: &str, _w: Option<f32>) -> Vec2 {
        Vec2::ZERO
    }
}

fn px(w: f32, h: f32) -> Size {
    Size {
        width: Dimension::Px(w),
        height: Dimension::Px(h),
    }
}

#[test]
fn steady_state_does_not_allocate() {
    let mut tree = UiTree::new(Style {
        background: spawn_core::Color::WHITE,
        size: px(300.0, 300.0),
        ..Default::default()
    });
    let root = tree.root();
    for _ in 0..8 {
        let _ = tree
            .create_node(
                Style {
                    background: spawn_core::Color::RED,
                    size: px(20.0, 20.0),
                    align_items: spawn_ui::AlignItems::Start,
                    ..Default::default()
                },
                root,
            )
            .unwrap();
    }

    let mut dl = DrawList::default();
    let mut m = ZeroMeasure;
    let size = Vec2::new(300.0, 300.0);

    // Warm-up: first layout + draw build allocate freely.
    for _ in 0..4 {
        tree.compute_layout(size, &mut m).unwrap();
        tree.build_draw_list(&mut dl).unwrap();
    }

    ARMED.with(|a| a.set(true));
    for _ in 0..1_000 {
        tree.compute_layout(size, &mut m).unwrap();
        tree.build_draw_list(&mut dl).unwrap();
    }
    ARMED.with(|a| a.set(false));

    let allocs = ALLOCS.load(Ordering::Relaxed);
    assert_eq!(allocs, 0, "steady-state path allocated {allocs} times");
}
