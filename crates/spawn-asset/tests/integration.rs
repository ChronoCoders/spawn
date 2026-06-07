//! Integration tests for the IO pool, hot-reload watcher, and full server flow.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use spawn_asset::{
    register_builtin_loaders, AppliedReport, AssetServer, AssetServerConfig, BinaryAsset,
    LoadState, ReloadOutcome, TextAsset,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "spawn_asset_it_{}_{}_{}",
            std::process::id(),
            unique,
            Instant::now().elapsed().as_nanos()
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

fn server(dir: &TempDir, hot_reload: bool) -> AssetServer {
    let mut srv = AssetServer::new(AssetServerConfig {
        root: dir.path.clone(),
        io_threads: 3,
        queue_capacity: 64,
        hot_reload,
        debounce: Duration::from_millis(40),
    })
    .expect("server");
    register_builtin_loaders(&mut srv).expect("builtin loaders");
    srv
}

fn pump_until_loaded(srv: &AssetServer, deadline: Duration) -> AppliedReport {
    let start = Instant::now();
    let mut total = AppliedReport::default();
    loop {
        let r = srv.apply_loaded();
        total.loaded += r.loaded;
        total.failed += r.failed;
        total.reloaded += r.reloaded;
        total.unloaded += r.unloaded;
        if r.loaded > 0 || r.failed > 0 || r.reloaded > 0 {
            return total;
        }
        if start.elapsed() > deadline {
            return total;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn loader_dispatch_and_state_transitions() {
    let dir = TempDir::new();
    dir.write("a.txt", b"hello");
    dir.write("b.bin", &[1, 2, 3]);
    let srv = server(&dir, false);

    let text = srv.load::<TextAsset>("a.txt");
    let bin = srv.load::<BinaryAsset>("b.bin");
    assert_eq!(srv.load_state(&text), LoadState::Loading);
    assert!(srv.get(&text).is_none());

    pump_until_loaded(&srv, Duration::from_secs(3));
    // Drain any straggler completions.
    for _ in 0..50 {
        srv.apply_loaded();
        if srv.load_state(&text) == LoadState::Loaded && srv.load_state(&bin) == LoadState::Loaded {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    assert_eq!(srv.load_state(&text), LoadState::Loaded);
    assert_eq!(
        srv.get(&text).map(|t| t.0.clone()),
        Some("hello".to_string())
    );
    assert_eq!(srv.load_state(&bin), LoadState::Loaded);
    assert_eq!(srv.get(&bin).map(|b| b.0.clone()), Some(vec![1, 2, 3]));
}

#[test]
fn case_insensitive_extension() {
    let dir = TempDir::new();
    dir.write("up.TXT", b"x");
    let srv = server(&dir, false);
    let h = srv.load::<TextAsset>("up.TXT");
    pump_until_loaded(&srv, Duration::from_secs(3));
    assert_eq!(srv.load_state(&h), LoadState::Loaded);
}

#[test]
fn unknown_extension_fails_with_no_loader() {
    let dir = TempDir::new();
    dir.write("x.unknown", b"x");
    let srv = server(&dir, false);
    let h = srv.load::<TextAsset>("x.unknown");
    assert_eq!(srv.load_state(&h), LoadState::Failed);
    assert!(matches!(
        srv.error(&h),
        Some(spawn_asset::AssetError::NoLoader { .. })
    ));
}

#[test]
fn duplicate_loader_registration_errors() {
    let dir = TempDir::new();
    let mut srv = AssetServer::new(AssetServerConfig {
        root: dir.path.clone(),
        hot_reload: false,
        ..Default::default()
    })
    .unwrap();
    register_builtin_loaders(&mut srv).unwrap();
    let err = register_builtin_loaders(&mut srv).unwrap_err();
    assert!(matches!(
        err,
        spawn_asset::AssetError::DuplicateLoader { .. }
    ));
}

#[test]
fn identity_dedup_across_equivalent_paths() {
    let dir = TempDir::new();
    std::fs::create_dir_all(dir.path.join("a")).unwrap();
    dir.write("a/b.txt", b"y");
    let srv = server(&dir, false);
    let h1 = srv.load::<TextAsset>("a/b.txt");
    let h2 = srv.load::<TextAsset>("a/./b.txt");
    assert_eq!(h1.id(), h2.id());
    assert_eq!(h1, h2);
}

#[test]
fn missing_file_fails_with_not_found() {
    let dir = TempDir::new();
    let srv = server(&dir, false);
    let h = srv.load::<TextAsset>("nope.txt");
    pump_until_loaded(&srv, Duration::from_secs(3));
    assert_eq!(srv.load_state(&h), LoadState::Failed);
    assert!(matches!(
        srv.error(&h),
        Some(spawn_asset::AssetError::NotFound { .. }) | Some(spawn_asset::AssetError::Io { .. })
    ));
}

#[test]
fn handle_lifetime_unload_and_weak() {
    let dir = TempDir::new();
    dir.write("u.txt", b"z");
    let srv = server(&dir, false);
    let weak;
    {
        let h = srv.load::<TextAsset>("u.txt");
        pump_until_loaded(&srv, Duration::from_secs(3));
        weak = h.downgrade();
        assert!(weak.upgrade().is_some());
        // h dropped here.
    }
    let mut unloaded = 0;
    for _ in 0..50 {
        unloaded += srv.apply_loaded().unloaded;
        if unloaded > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(unloaded >= 1);
    assert!(weak.upgrade().is_none());
}

#[test]
fn held_strong_handle_keeps_slot_alive() {
    let dir = TempDir::new();
    dir.write("k.txt", b"z");
    let srv = server(&dir, false);
    let h = srv.load::<TextAsset>("k.txt");
    pump_until_loaded(&srv, Duration::from_secs(3));
    let weak = h.downgrade();
    for _ in 0..5 {
        srv.apply_loaded();
    }
    assert!(weak.upgrade().is_some());
    drop(h);
}

#[test]
fn hot_reload_replaces_in_place() {
    let dir = TempDir::new();
    dir.write("r.txt", b"v1");
    let srv = server(&dir, true);
    let h = srv.load::<TextAsset>("r.txt");
    pump_until_loaded(&srv, Duration::from_secs(3));
    assert_eq!(srv.get(&h).map(|t| t.0.clone()), Some("v1".to_string()));

    std::thread::sleep(Duration::from_millis(60));
    dir.write("r.txt", b"v2-updated");

    let mut reloaded = false;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(4) {
        std::thread::sleep(Duration::from_millis(30));
        let r = srv.apply_loaded();
        if r.reloaded > 0 {
            let events = srv.drain_reload_events();
            assert!(events
                .iter()
                .any(|e| e.outcome == ReloadOutcome::Replaced && e.generation >= 1));
            reloaded = true;
            break;
        }
    }
    assert!(reloaded, "expected an in-place reload");
    assert_eq!(
        srv.get(&h).map(|t| t.0.clone()),
        Some("v2-updated".to_string())
    );
}

#[test]
fn hot_reload_invalid_utf8_marks_failed() {
    let dir = TempDir::new();
    dir.write("bad.txt", b"ok");
    let srv = server(&dir, true);
    let h = srv.load::<TextAsset>("bad.txt");
    pump_until_loaded(&srv, Duration::from_secs(3));
    assert_eq!(srv.load_state(&h), LoadState::Loaded);

    std::thread::sleep(Duration::from_millis(60));
    dir.write("bad.txt", &[0xff, 0xfe, 0xfd]);

    let mut failed = false;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(4) {
        std::thread::sleep(Duration::from_millis(30));
        let r = srv.apply_loaded();
        if r.failed > 0 {
            let events = srv.drain_reload_events();
            assert!(events.iter().any(|e| e.outcome == ReloadOutcome::Failed));
            failed = true;
            break;
        }
    }
    assert!(failed, "expected a failed reload");
    assert_eq!(srv.load_state(&h), LoadState::Failed);
    assert!(srv.get(&h).is_none());
    assert!(matches!(
        srv.error(&h),
        Some(spawn_asset::AssetError::InvalidUtf8 { .. })
    ));
}

#[test]
fn concurrent_load_stress() {
    let dir = TempDir::new();
    let count = 1200usize;
    for i in 0..count {
        dir.write(&format!("f{i}.txt"), format!("content-{i}").as_bytes());
    }
    let srv = std::sync::Arc::new(server(&dir, false));

    let mut threads = Vec::new();
    for t in 0..6 {
        let srv = std::sync::Arc::clone(&srv);
        threads.push(std::thread::spawn(move || {
            let mut handles = Vec::new();
            let mut i = t;
            while i < count {
                handles.push(srv.load::<TextAsset>(&format!("f{i}.txt")));
                i += 6;
            }
            handles
        }));
    }
    let mut all = Vec::new();
    for th in threads {
        all.extend(th.join().expect("thread join"));
    }

    let start = Instant::now();
    let mut loaded = 0;
    while loaded < all.len() && start.elapsed() < Duration::from_secs(20) {
        srv.apply_loaded();
        loaded = all
            .iter()
            .filter(|h| srv.load_state(h) == LoadState::Loaded)
            .count();
        if loaded < all.len() {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    assert_eq!(loaded, all.len(), "every handle should reach Loaded");
}

#[test]
fn full_queue_defers_and_drains_over_multiple_pumps() {
    // One worker + capacity-1 queue: a burst of loads cannot all be submitted at
    // once, so the server defers the overflow and retries it on later pumps. Big
    // files keep the single worker busy long enough that the first pump cannot
    // drain everything, exercising the defer -> retry -> drain path.
    let dir = TempDir::new();
    let n = 8usize;
    let big = vec![b'x'; 512 * 1024];
    for i in 0..n {
        dir.write(&format!("q{i}.bin"), &big);
    }
    let mut srv = AssetServer::new(AssetServerConfig {
        root: dir.path.clone(),
        io_threads: 1,
        queue_capacity: 1,
        hot_reload: false,
        debounce: Duration::from_millis(40),
    })
    .expect("server");
    register_builtin_loaders(&mut srv).expect("builtin loaders");

    let handles: Vec<_> = (0..n)
        .map(|i| srv.load::<BinaryAsset>(&format!("q{i}.bin")))
        .collect();

    // First pump retries deferred jobs and applies whatever workers finished, but
    // with capacity 1 and a single busy worker it cannot have drained all N.
    let first = srv.apply_loaded();
    let loaded_after_first = handles
        .iter()
        .filter(|h| srv.load_state(h) == LoadState::Loaded)
        .count();
    assert!(
        loaded_after_first < n,
        "first pump should not drain all {n} handles (got {loaded_after_first}, report {first:?})"
    );

    // A bounded pump loop must eventually drain every handle, proving deferred
    // jobs are retried rather than dropped.
    let start = Instant::now();
    let mut loaded = loaded_after_first;
    while loaded < n && start.elapsed() < Duration::from_secs(2) {
        srv.apply_loaded();
        loaded = handles
            .iter()
            .filter(|h| srv.load_state(h) == LoadState::Loaded)
            .count();
        if loaded < n {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    assert_eq!(loaded, n, "all deferred handles must eventually load");
}

#[test]
fn clean_shutdown_joins_threads() {
    let dir = TempDir::new();
    dir.write("s.txt", b"x");
    let srv = server(&dir, true);
    let _h = srv.load::<TextAsset>("s.txt");
    pump_until_loaded(&srv, Duration::from_secs(3));
    // Dropping the server must join all IO threads and stop the watcher; if it
    // hung this test would time out under the harness.
    drop(srv);
}
