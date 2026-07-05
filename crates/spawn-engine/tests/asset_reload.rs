//! The frame loop surfaces the asset pump: `FrameAssets` reports each frame's
//! `apply_loaded` outcome, and `ReloadEvents` surfaces in-place hot-reloads and is
//! cleared the following frame. Reproducible resource wiring is asserted without a
//! filesystem; the reload path is driven through the real watcher (as spawn-asset's
//! own integration test does) under a bounded deadline.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use spawn_asset::{register_builtin_loaders, Handle, ReloadOutcome, TextAsset};
use spawn_ecs::Resource;
use spawn_engine::{App, AppliedReport, Engine, EngineConfig, FrameAssets, ReloadEvents};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Keeps the loaded asset's strong handle alive for the engine's lifetime, so a
/// file change hot-reloads the live slot instead of unloading a dropped handle.
struct HeldAsset {
    #[allow(dead_code)]
    handle: Handle<TextAsset>,
}

impl Resource for HeldAsset {}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "spawn_engine_reload_{}_{}",
            std::process::id(),
            unique
        ));
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    fn write(&self, name: &str, contents: &[u8]) {
        std::fs::write(self.path.join(name), contents).expect("write file");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Ticks until `done` holds or `deadline` elapses; returns whether `done` held.
/// Sleeps between ticks so the wall-clock reload watcher can observe file changes.
fn tick_until(
    engine: &mut Engine,
    deadline: Duration,
    mut done: impl FnMut(&Engine) -> bool,
) -> bool {
    let start = Instant::now();
    loop {
        engine.tick().expect("tick");
        if done(engine) {
            return true;
        }
        if start.elapsed() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn resources_present_and_empty_without_hot_reload() {
    let mut engine = App::new().build_headless().expect("headless build");
    engine.tick().expect("tick");
    let world = engine.world();

    let applied = world.get_resource::<FrameAssets>().map(|f| f.applied);
    assert_eq!(
        applied,
        Some(AppliedReport::default()),
        "no loads or reloads without assets"
    );

    let empty = world.get_resource::<ReloadEvents>().map(|r| r.is_empty());
    assert_eq!(
        empty,
        Some(true),
        "ReloadEvents is present and empty when hot-reload is off"
    );
}

#[test]
fn hot_reload_surfaces_and_clears_reload_events() {
    let dir = TempDir::new();
    dir.write("r.txt", b"v1");

    let mut app = App::new();
    app.set_config(EngineConfig {
        asset_root: dir.path.clone(),
        hot_reload: true,
        ..Default::default()
    });
    app.add_audio_setup(|assets, world| {
        // The asset-setup hook is the app's access point to the server; used here
        // to register the text loader and start the watched load. The handle is
        // stashed in a resource so the strong reference outlives assembly.
        register_builtin_loaders(assets)?;
        let handle = assets.load::<TextAsset>("r.txt");
        world.insert_resource(HeldAsset { handle });
        Ok(())
    });
    let mut engine = app.build_headless().expect("headless build");

    let loaded = tick_until(&mut engine, Duration::from_secs(3), |e| {
        e.world()
            .get_resource::<FrameAssets>()
            .map(|f| f.applied.loaded > 0)
            .unwrap_or(false)
    });
    assert!(loaded, "initial load should surface in FrameAssets");

    std::thread::sleep(Duration::from_millis(60));
    dir.write("r.txt", b"v2-updated");

    let reloaded = tick_until(&mut engine, Duration::from_secs(4), |e| {
        e.world()
            .get_resource::<ReloadEvents>()
            .map(|r| !r.is_empty())
            .unwrap_or(false)
    });
    assert!(reloaded, "in-place reload should surface in ReloadEvents");

    let replaced = engine
        .world()
        .get_resource::<ReloadEvents>()
        .map(|r| {
            r.events()
                .iter()
                .any(|ev| ev.outcome == ReloadOutcome::Replaced)
        })
        .unwrap_or(false);
    assert!(replaced, "the reload event should be a Replaced outcome");

    engine.tick().expect("tick");
    let cleared = engine
        .world()
        .get_resource::<ReloadEvents>()
        .map(|r| r.is_empty())
        .unwrap_or(false);
    assert!(cleared, "ReloadEvents is cleared the frame after a reload");
}
