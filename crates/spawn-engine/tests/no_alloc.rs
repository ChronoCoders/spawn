//! A warm headless tick performs no engine-owned heap allocation. Kept in its
//! own test binary (sole test) so a parallel sibling test cannot pollute the
//! process-global allocation counter.

use std::alloc::{GlobalAlloc, Layout, System as SystemAlloc};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use spawn_asset::AssetId;
use spawn_core::Mat4;
use spawn_ecs::{Commands, Query, Res, ResMut, Resource};
use spawn_engine::{App, RenderProxies, RenderProxy, ScheduleLabel, Time};

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

#[derive(Default)]
struct Accum(i64);
impl Resource for Accum {}

#[derive(Clone, Copy)]
struct Value(i64);
impl spawn_ecs::Component for Value {}

#[test]
fn warm_tick_is_allocation_free() {
    let mut app = App::new();
    app.insert_resource(Accum::default());
    for i in 0..64 {
        app.world_mut().spawn_with(Value(i));
    }
    // A read system accumulating into a resource (no structural change).
    app.add_system(
        ScheduleLabel::Update,
        |q: Query<'_, &Value, ()>,
         mut acc: ResMut<'_, Accum>,
         _t: Res<'_, Time>,
         _c: &mut Commands<'_>| {
            acc.0 = q.iter().map(|v| v.0).sum();
            Ok(())
        },
    );
    // A steady extraction (fixed draw count → buffers reach capacity).
    app.add_extract(|world: &spawn_ecs::World, proxies: &mut RenderProxies| {
        let mesh = AssetId::from_canonical_path("mesh");
        let material = AssetId::from_canonical_path("material");
        for _ in world.query::<&Value>().iter() {
            proxies.draws.push(RenderProxy {
                model: Mat4::IDENTITY,
                mesh,
                material,
            });
        }
    });

    let mut engine = app.build_headless().unwrap();
    // Warm up so both proxy buffers and all schedule buffers reach capacity.
    for _ in 0..8 {
        engine.tick().unwrap();
    }

    ALLOCS.store(0, Ordering::Relaxed);
    ARMED.store(true, Ordering::Relaxed);
    engine.tick().unwrap();
    ARMED.store(false, Ordering::Relaxed);

    assert_eq!(
        ALLOCS.load(Ordering::Relaxed),
        0,
        "a warm headless tick must not allocate"
    );
}
