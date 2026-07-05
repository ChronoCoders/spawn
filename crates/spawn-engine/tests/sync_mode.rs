//! Sync-mode latency on the inline executor: it renders the buffer extracted this
//! frame in both modes, so frames-in-flight is `0`. The `Pipelined` one-frame lag
//! is a property of the render thread and is covered by the threaded executor.

use std::sync::{Arc, Mutex};

use spawn_asset::AssetId;
use spawn_core::Mat4;
use spawn_engine::{
    App, EngineConfig, EngineResult, RenderBackend, RenderProxies, RenderProxy, SurfaceSize,
    SyncMode, Time,
};

/// Records the draw count submitted each frame. `Arc<Mutex<_>>` because the
/// backend is `Send` (it may run on the render thread).
struct Recording {
    counts: Arc<Mutex<Vec<usize>>>,
}

impl RenderBackend for Recording {
    fn submit(
        &mut self,
        proxies: &RenderProxies,
        _ui: Option<&mut spawn_engine::UiTree>,
    ) -> EngineResult<()> {
        self.counts
            .lock()
            .expect("counts lock")
            .push(proxies.draws.len());
        Ok(())
    }

    fn resize(&mut self, _size: SurfaceSize) -> EngineResult<()> {
        Ok(())
    }
}

/// Runs five frames where frame N extracts N draw proxies, returning what the
/// backend rendered each frame and the frames-in-flight after each tick.
fn run(mode: SyncMode) -> (Vec<usize>, Vec<u32>) {
    let counts = Arc::new(Mutex::new(Vec::new()));
    let mut app = App::new();
    app.set_config(EngineConfig {
        sync_mode: mode,
        ..Default::default()
    });
    app.add_extract(|world: &spawn_ecs::World, proxies: &mut RenderProxies| {
        let frame = world.get_resource::<Time>().map(|t| t.frame()).unwrap_or(0);
        for _ in 0..frame {
            proxies.draws.push(RenderProxy {
                model: Mat4::IDENTITY,
                mesh: AssetId::from_canonical_path("mesh"),
                material: AssetId::from_canonical_path("material"),
            });
        }
    });

    let backend = Box::new(Recording {
        counts: Arc::clone(&counts),
    });
    let mut engine = app.build_headless_with(1.0 / 60.0, backend).unwrap();

    let mut flights = Vec::new();
    for _ in 0..5 {
        engine.tick().unwrap();
        flights.push(engine.frames_in_flight());
    }
    // Drop the engine (and the backend's Arc clone) before reclaiming the counts.
    drop(engine);
    let counts = Arc::try_unwrap(counts)
        .expect("sole owner")
        .into_inner()
        .expect("counts lock");
    (counts, flights)
}

#[test]
fn immediate_renders_current_frame_zero_in_flight() {
    let (counts, flights) = run(SyncMode::Immediate);
    assert_eq!(counts, vec![1, 2, 3, 4, 5], "renders this frame's draws");
    assert!(flights.iter().all(|&f| f == 0), "zero frames in flight");
}

#[test]
fn last_render_report_tracks_frame_and_draw_count() {
    let mut app = App::new();
    app.add_extract(|world: &spawn_ecs::World, proxies: &mut RenderProxies| {
        let frame = world.get_resource::<Time>().map(|t| t.frame()).unwrap_or(0);
        for _ in 0..frame {
            proxies.draws.push(RenderProxy {
                model: Mat4::IDENTITY,
                mesh: AssetId::from_canonical_path("mesh"),
                material: AssetId::from_canonical_path("material"),
            });
        }
    });
    let mut engine = app.build_headless().unwrap();
    engine.tick().unwrap();
    engine.tick().unwrap();

    let report = engine.last_render_report();
    assert_eq!(
        report.draw_count, 2,
        "report reflects this frame's draw count"
    );
    assert_eq!(report.frame, 1, "frame index advances once per submit");
    assert!(report.error.is_none(), "no error on a clean frame");
}

#[test]
fn pipelined_inline_collapses_to_immediate() {
    // On the inline executor there is no render thread to lag, so `Pipelined`
    // renders this frame's draws with zero frames in flight, same as `Immediate`.
    let (counts, flights) = run(SyncMode::Pipelined);
    assert_eq!(counts, vec![1, 2, 3, 4, 5], "renders this frame's draws");
    assert!(flights.iter().all(|&f| f == 0), "zero frames in flight");
}
