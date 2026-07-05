//! Sync-mode latency on the inline executor: it renders the buffer extracted this
//! frame in both modes, so frames-in-flight is `0`. The `Pipelined` one-frame lag
//! is a property of the render thread and is covered by the threaded executor.

use std::cell::RefCell;
use std::rc::Rc;

use spawn_asset::AssetId;
use spawn_core::Mat4;
use spawn_engine::{
    App, EngineConfig, EngineResult, RenderBackend, RenderProxies, RenderProxy, SurfaceSize,
    SyncMode, Time,
};

/// Records the draw count submitted each frame.
struct Recording {
    counts: Rc<RefCell<Vec<usize>>>,
}

impl RenderBackend for Recording {
    fn submit(
        &mut self,
        proxies: &RenderProxies,
        _ui: Option<&mut spawn_engine::UiTree>,
    ) -> EngineResult<()> {
        self.counts.borrow_mut().push(proxies.draws.len());
        Ok(())
    }

    fn resize(&mut self, _size: SurfaceSize) -> EngineResult<()> {
        Ok(())
    }
}

/// Runs five frames where frame N extracts N draw proxies, returning what the
/// backend rendered each frame and the frames-in-flight after each tick.
fn run(mode: SyncMode) -> (Vec<usize>, Vec<u32>) {
    let counts = Rc::new(RefCell::new(Vec::new()));
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
        counts: Rc::clone(&counts),
    });
    let mut engine = app.build_headless_with(1.0 / 60.0, backend).unwrap();

    let mut flights = Vec::new();
    for _ in 0..5 {
        engine.tick().unwrap();
        flights.push(engine.frames_in_flight());
    }
    // Drop the engine (and the backend's Rc clone) before reclaiming the counts.
    drop(engine);
    let counts = Rc::try_unwrap(counts).unwrap().into_inner();
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
