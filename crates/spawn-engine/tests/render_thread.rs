//! The `ThreadedExecutor` runs rendering on a dedicated OS thread. Driven here
//! with a `HeadlessBackend` (no display) so the threading, ownership-passing,
//! frames-in-flight bound, per-frame report, error surfacing, and clean join are
//! all exercised headlessly. The real windowed backend is validated on hardware.

use spawn_asset::AssetId;
use spawn_core::Mat4;
use spawn_engine::{
    App, Engine, EngineConfig, EngineError, EngineResult, HeadlessBackend, RenderBackend,
    RenderProxies, RenderProxy, SurfaceSize, SyncMode, Time,
};

/// A threaded headless engine that extracts `frame` draw proxies each tick.
fn threaded_app(mode: SyncMode, backend: Box<dyn RenderBackend>) -> Engine {
    let mut app = App::new();
    app.set_config(EngineConfig {
        sync_mode: mode,
        render_thread: true,
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
    app.build_headless_with(1.0 / 60.0, backend)
        .expect("threaded headless build")
}

#[test]
fn threaded_immediate_zero_in_flight_with_deterministic_reports() {
    let mut engine = threaded_app(SyncMode::Immediate, Box::new(HeadlessBackend::new()));
    let mut draws = Vec::new();
    for _ in 0..5 {
        engine.tick().expect("tick");
        assert_eq!(
            engine.frames_in_flight(),
            0,
            "immediate: zero frames in flight"
        );
        draws.push(engine.last_render_report().draw_count);
    }
    assert_eq!(
        draws,
        vec![1, 2, 3, 4, 5],
        "report draw_count matches the extracted counts, in order"
    );
    engine.shutdown().expect("clean render-thread join");
}

#[test]
fn threaded_pipelined_bounded_in_flight() {
    let mut engine = threaded_app(SyncMode::Pipelined, Box::new(HeadlessBackend::new()));
    for _ in 0..6 {
        engine.tick().expect("tick");
        assert!(
            engine.frames_in_flight() <= 1,
            "pipelined: at most one frame in flight"
        );
    }
    engine.shutdown().expect("clean render-thread join");
}

/// Panics on submit after `ok_frames` successful frames, killing the render
/// thread. `#[non_exhaustive]` `EngineError` cannot be constructed here, so a
/// panic (thread death) is how a test drives the `RenderThread` surface.
struct PanicBackend {
    ok_frames: u64,
    frame: u64,
}

impl RenderBackend for PanicBackend {
    fn submit(
        &mut self,
        _proxies: &RenderProxies,
        _ui: Option<&mut spawn_engine::UiTree>,
    ) -> EngineResult<()> {
        self.frame += 1;
        assert!(
            self.frame <= self.ok_frames,
            "backend deliberately failing on frame {}",
            self.frame
        );
        Ok(())
    }

    fn resize(&mut self, _size: SurfaceSize) -> EngineResult<()> {
        Ok(())
    }
}

#[test]
fn threaded_render_thread_death_surfaces_render_thread_error() {
    let mut engine = threaded_app(
        SyncMode::Immediate,
        Box::new(PanicBackend {
            ok_frames: 1,
            frame: 0,
        }),
    );
    engine.tick().expect("first frame renders");
    let err = engine
        .tick()
        .expect_err("a dead render thread surfaces from tick");
    assert!(
        matches!(err, EngineError::RenderThread { .. }),
        "expected a render-thread error, got {err:?}"
    );
    // Shutdown after the thread died returns a join error; the loop is exiting.
    let _ = engine.shutdown();
}
