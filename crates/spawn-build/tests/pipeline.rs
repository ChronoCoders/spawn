//! End-to-end pipeline integration tests.

use std::path::{Path, PathBuf};

use spawn_build::{AssetId, BuildConfig, BuildManifest, BuildPipeline};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "spawn-build-it-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
    fn file(&self, rel: &str, bytes: &[u8]) {
        let p = self.path.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn manifest(root: &Path, out: &Path) -> BuildManifest {
    let text = format!(
        "source_root = {}\noutput_dir = {}\ninclude = **\n",
        root.display(),
        out.display()
    );
    BuildManifest::parse_str(&text, Path::new("/")).unwrap()
}

#[test]
fn incremental_skip_and_recompile() {
    let tree = TempDir::new("incr");
    tree.file("src/a.txt", b"aaa");
    tree.file("src/b.txt", b"bbb");
    tree.file("src/c.txt", b"ccc");
    let root = tree.path.join("src");
    let out = tree.path.join("out");
    let m = manifest(&root, &out);

    let pipeline = BuildPipeline::new(m.clone()).unwrap();
    let r1 = pipeline.build().unwrap();
    assert_eq!(r1.compiled, 3);
    assert_eq!(r1.skipped, 0);
    assert!(r1.is_success());

    let r2 = pipeline.build().unwrap();
    assert_eq!(r2.compiled, 0);
    assert_eq!(r2.skipped, 3);

    std::fs::write(root.join("b.txt"), b"BBBB").unwrap();
    let r3 = pipeline.build().unwrap();
    assert_eq!(r3.compiled, 1);
    assert_eq!(r3.skipped, 2);

    // delete one output -> forces recompile despite cache hash match
    let id_a = AssetId::from_canonical_path("a.txt");
    std::fs::remove_file(out.join("data").join(format!("{:016x}", id_a.raw()))).unwrap();
    let r4 = pipeline.build().unwrap();
    assert_eq!(r4.compiled, 1);
    assert_eq!(r4.skipped, 2);
}

#[test]
fn byte_identical_index_across_runs() {
    let tree_a = TempDir::new("detA");
    let tree_b = TempDir::new("detB");
    for tree in [&tree_a, &tree_b] {
        tree.file("src/x.png", b"image-bytes-here");
        tree.file("src/sub/y.txt", b"text content");
        tree.file("src/sub/deep/z.bin", &[0u8, 1, 2, 3, 4, 5]);
    }
    let build = |tree: &TempDir| {
        let root = tree.path.join("src");
        let out = tree.path.join("out");
        let p = BuildPipeline::new(manifest(&root, &out)).unwrap();
        p.build().unwrap();
        out
    };
    let out_a = build(&tree_a);
    let out_b = build(&tree_b);

    let idx_a = std::fs::read(out_a.join("index.spawnpack")).unwrap();
    let idx_b = std::fs::read(out_b.join("index.spawnpack")).unwrap();
    assert_eq!(idx_a, idx_b, "index.spawnpack must be byte-identical");

    for name in ["x.png", "sub/y.txt", "sub/deep/z.bin"] {
        let id = AssetId::from_canonical_path(name);
        let hex = format!("{:016x}", id.raw());
        let da = std::fs::read(out_a.join("data").join(&hex)).unwrap();
        let db = std::fs::read(out_b.join("data").join(&hex)).unwrap();
        assert_eq!(da, db, "data output for {name} must be byte-identical");
    }
}

#[test]
fn per_asset_failure_isolation() {
    let tree = TempDir::new("fail");
    tree.file("src/good1.txt", b"g1");
    tree.file("src/good2.txt", b"g2");
    tree.file("src/bad.txt", b"bad");
    let root = tree.path.join("src");
    let out = tree.path.join("out");
    let pipeline = BuildPipeline::new(manifest(&root, &out)).unwrap();

    // Force exactly one asset's compile to fail: pre-create its atomic temp
    // output path (`data/<id>.tmp`) as a directory so `File::create` fails.
    let data = out.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let bad_id = AssetId::from_canonical_path("bad.txt");
    std::fs::create_dir(data.join(format!("{:016x}.tmp", bad_id.raw()))).unwrap();

    let report = pipeline.build().unwrap();
    assert_eq!(report.compiled, 2);
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].0, bad_id);
    assert!(!report.is_success());

    let idx = spawn_build::PackIndex::read(&out.join("index.spawnpack")).unwrap();
    assert_eq!(idx.entries.len(), 2);
    assert!(!idx.entries.iter().any(|e| e.id == bad_id));
}

#[test]
fn clean_removes_outputs() {
    let tree = TempDir::new("clean");
    tree.file("src/a.txt", b"a");
    let root = tree.path.join("src");
    let out = tree.path.join("out");
    let pipeline = BuildPipeline::new(manifest(&root, &out)).unwrap();
    pipeline.build().unwrap();
    assert!(out.join("data").is_dir());
    assert!(out.join("index.spawnpack").is_file());
    assert!(out.join("build.cache").is_file());

    pipeline.clean().unwrap();
    assert!(!out.join("data").exists());
    assert!(!out.join("index.spawnpack").exists());
    assert!(!out.join("build.cache").exists());
    assert!(root.join("a.txt").is_file());

    pipeline.clean().unwrap();
}

#[test]
fn parallel_matches_single_worker() {
    let tree_p = TempDir::new("par");
    let tree_s = TempDir::new("seq");
    for tree in [&tree_p, &tree_s] {
        for i in 0..300u32 {
            tree.file(&format!("src/dir{}/f{i}.bin", i % 8), &i.to_le_bytes());
        }
    }
    let build = |tree: &TempDir, workers: usize| {
        let root = tree.path.join("src");
        let out = tree.path.join("out");
        let cfg = BuildConfig {
            workers,
            cache_name: "build.cache".to_string(),
        };
        let p = BuildPipeline::with_config(manifest(&root, &out), cfg).unwrap();
        let report = p.build().unwrap();
        assert_eq!(report.compiled, 300);
        (out, report)
    };
    let (out_p, _) = build(&tree_p, 8);
    let (out_s, _) = build(&tree_s, 1);

    let idx_p = std::fs::read(out_p.join("index.spawnpack")).unwrap();
    let idx_s = std::fs::read(out_s.join("index.spawnpack")).unwrap();
    assert_eq!(idx_p, idx_s, "parallel build must match single-worker");
}

/// §11 "Parallel compile correctness": the build must complete under a hard
/// deadline (no deadlock). The build runs on a child thread and the parent
/// fails the test if it has not finished within the deadline.
#[test]
fn parallel_build_completes_under_hard_deadline() {
    use std::sync::mpsc;
    use std::time::Duration;

    let tree = TempDir::new("deadline");
    for i in 0..300u32 {
        tree.file(&format!("src/dir{}/f{i}.bin", i % 8), &i.to_le_bytes());
    }
    let root = tree.path.join("src");
    let out = tree.path.join("out");
    let cfg = BuildConfig {
        workers: 8,
        cache_name: "build.cache".to_string(),
    };
    let pipeline = BuildPipeline::with_config(manifest(&root, &out), cfg).unwrap();

    let (tx, rx) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        let report = pipeline.build();
        let _ = tx.send(report.map(|r| r.compiled));
    });

    match rx.recv_timeout(Duration::from_secs(60)) {
        Ok(Ok(compiled)) => {
            assert_eq!(compiled, 300);
            handle.join().unwrap();
        }
        Ok(Err(err)) => panic!("build failed: {err}"),
        Err(_) => {
            panic!("parallel build did not complete within the 60s hard deadline (deadlock?)")
        }
    }
}
