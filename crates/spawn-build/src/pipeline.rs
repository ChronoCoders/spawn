//! Build orchestration: discovery, incremental cache, parallel compile, packing.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use spawn_asset::AssetId;

use crate::cache::{BuildCache, CacheRecord};
use crate::compile::compile_asset;
use crate::discover::{discover, AssetEntry};
use crate::error::{BuildError, BuildResult};
use crate::manifest::BuildManifest;
use crate::pack::{PackEntry, PackIndex, PACK_FLAG_EXTERNAL};

/// Build configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildConfig {
    pub workers: usize,
    pub cache_name: String,
}

impl Default for BuildConfig {
    fn default() -> Self {
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .max(1);
        Self {
            workers,
            cache_name: "build.cache".to_string(),
        }
    }
}

/// Outcome of a build.
///
/// `duration` is the only non-deterministic field and is never written to any
/// on-disk output; it exists purely for caller reporting. `failed` is sorted by
/// `AssetId` so the report itself is order-stable.
#[derive(Debug)]
pub struct BuildReport {
    pub compiled: usize,
    pub skipped: usize,
    pub failed: Vec<(AssetId, BuildError)>,
    pub total_bytes: u64,
    pub duration: Duration,
}

impl BuildReport {
    pub fn is_success(&self) -> bool {
        self.failed.is_empty()
    }
}

/// The build orchestrator.
pub struct BuildPipeline {
    manifest: BuildManifest,
    config: BuildConfig,
}

/// Per-asset outcome produced by a worker thread.
enum CompileResult {
    Compiled {
        id: AssetId,
        record: CacheRecord,
        output_len: u64,
    },
    Failed {
        id: AssetId,
        error: BuildError,
    },
}

/// Test-only observability: the exact number of worker threads spawned by the
/// most recent [`BuildPipeline::compile_parallel`] call, and the peak number
/// observed running concurrently. These let the parallel-compile test assert
/// the worker pool is actually exercised without making production code
/// observable.
#[cfg(test)]
static SPAWNED_WORKERS: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
static PEAK_CONCURRENCY: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
#[derive(Default)]
struct ConcurrencyGauge {
    current: AtomicUsize,
    peak: AtomicUsize,
}

#[cfg(test)]
struct ActiveGuard<'a>(&'a ConcurrencyGauge);

#[cfg(test)]
impl ConcurrencyGauge {
    fn enter(&self) -> ActiveGuard<'_> {
        let now = self.current.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        ActiveGuard(self)
    }
    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        self.0.current.fetch_sub(1, Ordering::SeqCst);
    }
}

impl BuildPipeline {
    /// Creates a pipeline with [`BuildConfig::default`].
    ///
    /// # Errors
    /// Returns [`BuildError::SourceRootMissing`] if `manifest.source_root` does
    /// not exist or is not a directory. No other filesystem state is touched
    /// (`output_dir` need not yet exist).
    pub fn new(manifest: BuildManifest) -> BuildResult<Self> {
        Self::with_config(manifest, BuildConfig::default())
    }

    /// Creates a pipeline with an explicit [`BuildConfig`]. `config.workers` is
    /// clamped to at least 1.
    ///
    /// # Errors
    /// Returns [`BuildError::SourceRootMissing`] if `manifest.source_root` does
    /// not exist or is not a directory. No other filesystem state is touched
    /// (`output_dir` need not yet exist).
    pub fn with_config(manifest: BuildManifest, config: BuildConfig) -> BuildResult<Self> {
        if !manifest.source_root.is_dir() {
            return Err(BuildError::SourceRootMissing {
                path: manifest.source_root.clone(),
            });
        }
        let config = BuildConfig {
            workers: config.workers.max(1),
            ..config
        };
        Ok(Self { manifest, config })
    }

    pub fn build(&self) -> BuildResult<BuildReport> {
        let start = Instant::now();
        let entries = discover(&self.manifest)?;
        let cache_path = self.manifest.output_dir.join(&self.config.cache_name);
        let cache = BuildCache::load(&cache_path)?;

        let data_dir = self.manifest.output_dir.join("data");
        std::fs::create_dir_all(&data_dir).map_err(|source| BuildError::Io {
            path: data_dir.clone(),
            source,
        })?;

        let mut skip: Vec<(&AssetEntry, CacheRecord)> = Vec::new();
        let mut to_compile: Vec<&AssetEntry> = Vec::new();
        for entry in &entries {
            let hit = cache.lookup(entry.id).filter(|rec| {
                rec.source_hash == entry.content_hash
                    && data_dir.join(format!("{:016x}", entry.id.raw())).is_file()
            });
            match hit {
                Some(record) => skip.push((entry, record)),
                None => to_compile.push(entry),
            }
        }

        let compiled_results = self.compile_parallel(&to_compile);

        let mut new_cache = BuildCache::default();
        let mut total_bytes: u64 = 0;
        let mut failed: Vec<(AssetId, BuildError)> = Vec::new();
        let mut compiled_count = 0usize;
        // output_hash by id for index assembly (compiled this run).
        let mut compiled_hashes: std::collections::HashMap<u64, u64> =
            std::collections::HashMap::new();

        for result in compiled_results {
            match result {
                CompileResult::Compiled {
                    id,
                    record,
                    output_len,
                } => {
                    new_cache.record(id, record);
                    compiled_hashes.insert(id.raw(), record.output_hash);
                    total_bytes += output_len;
                    compiled_count += 1;
                }
                CompileResult::Failed { id, error } => failed.push((id, error)),
            }
        }

        for (entry, record) in &skip {
            new_cache.record(entry.id, *record);
            total_bytes += entry.byte_len;
        }

        let failed_ids: std::collections::HashSet<u64> =
            failed.iter().map(|(id, _)| id.raw()).collect();

        let mut pack_entries: Vec<PackEntry> = Vec::new();
        for entry in &entries {
            if failed_ids.contains(&entry.id.raw()) {
                continue;
            }
            let output_hash = compiled_hashes
                .get(&entry.id.raw())
                .copied()
                .or_else(|| new_cache.lookup(entry.id).map(|r| r.output_hash));
            if let Some(content_hash) = output_hash {
                pack_entries.push(PackEntry {
                    id: entry.id,
                    offset: 0,
                    flags: PACK_FLAG_EXTERNAL,
                    rel_path: entry.source_path.clone(),
                    content_hash,
                });
            }
        }

        let index = PackIndex::new(pack_entries);
        index.write(&self.manifest.output_dir.join("index.spawnpack"))?;
        new_cache.save(&cache_path)?;

        failed.sort_by_key(|(id, _)| id.raw());

        Ok(BuildReport {
            compiled: compiled_count,
            skipped: skip.len(),
            failed,
            total_bytes,
            duration: start.elapsed(),
        })
    }

    /// Compiles `to_compile` across exactly `config.workers` threads (§8.1
    /// step 5). Each worker pulls indices from a shared atomic counter (bounded,
    /// no unbounded spawning); workers that find no remaining work exit
    /// immediately. Per-asset failures are collected, not propagated, so one
    /// failure does not poison the scope.
    fn compile_parallel(&self, to_compile: &[&AssetEntry]) -> Vec<CompileResult> {
        if to_compile.is_empty() {
            return Vec::new();
        }
        let next = AtomicUsize::new(0);
        let results: Mutex<Vec<CompileResult>> = Mutex::new(Vec::new());
        let workers = self.config.workers.max(1);
        #[cfg(test)]
        let gauge = ConcurrencyGauge::default();

        std::thread::scope(|scope| {
            for _ in 0..workers {
                let next = &next;
                let results = &results;
                let manifest = &self.manifest;
                #[cfg(test)]
                let gauge = &gauge;
                scope.spawn(move || {
                    #[cfg(test)]
                    let _active = gauge.enter();
                    loop {
                        let idx = next.fetch_add(1, Ordering::Relaxed);
                        if idx >= to_compile.len() {
                            break;
                        }
                        let entry = to_compile[idx];
                        let outcome = match compile_asset(entry, manifest) {
                            Ok(output) => CompileResult::Compiled {
                                id: entry.id,
                                record: CacheRecord {
                                    source_hash: entry.content_hash,
                                    output_hash: output.output_hash,
                                },
                                output_len: output.output_len,
                            },
                            Err(error) => CompileResult::Failed {
                                id: entry.id,
                                error,
                            },
                        };
                        if let Ok(mut guard) = results.lock() {
                            guard.push(outcome);
                        }
                    }
                });
            }
        });

        #[cfg(test)]
        SPAWNED_WORKERS.store(workers, Ordering::Relaxed);
        #[cfg(test)]
        PEAK_CONCURRENCY.store(gauge.peak(), Ordering::Relaxed);

        results.into_inner().unwrap_or_default()
    }

    /// Removes `data/`, `index.spawnpack`, and the cache file if present. Missing
    /// outputs are not an error; nothing outside `output_dir` is touched.
    pub fn clean(&self) -> BuildResult<()> {
        let data_dir = self.manifest.output_dir.join("data");
        remove_dir_if_present(&data_dir)?;
        remove_file_if_present(&self.manifest.output_dir.join("index.spawnpack"))?;
        remove_file_if_present(&self.manifest.output_dir.join(&self.config.cache_name))?;
        Ok(())
    }
}

fn remove_file_if_present(path: &std::path::Path) -> BuildResult<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(BuildError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn remove_dir_if_present(path: &std::path::Path) -> BuildResult<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(BuildError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glob::Pattern;
    use std::path::PathBuf;

    struct TempTree {
        root: PathBuf,
    }
    impl TempTree {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "spawn-build-pipeline-{tag}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self { root }
        }
        fn file(&self, rel: &str, bytes: &[u8]) {
            let p = self.root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        }
    }
    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn manifest(root: &std::path::Path, out: &std::path::Path) -> BuildManifest {
        BuildManifest {
            source_root: root.to_path_buf(),
            output_dir: out.to_path_buf(),
            include: vec![Pattern::compile("**").unwrap()],
            exclude: vec![],
        }
    }

    #[test]
    fn compile_spawns_exactly_workers_and_runs_concurrently() {
        let tree = TempTree::new("workers");
        for i in 0..300u32 {
            tree.file(&format!("d{}/f{i}.bin", i % 8), &i.to_le_bytes());
        }
        let out = tree.root.join("out");
        let cfg = BuildConfig {
            workers: 8,
            cache_name: "build.cache".to_string(),
        };
        let pipeline = BuildPipeline::with_config(manifest(&tree.root, &out), cfg).unwrap();
        let report = pipeline.build().unwrap();
        assert_eq!(report.compiled, 300);

        assert_eq!(
            SPAWNED_WORKERS.load(Ordering::SeqCst),
            8,
            "must spawn exactly `workers` threads"
        );
        assert!(
            PEAK_CONCURRENCY.load(Ordering::SeqCst) > 1,
            "worker pool must run threads concurrently (peak {} <= 1)",
            PEAK_CONCURRENCY.load(Ordering::SeqCst)
        );
    }

    #[test]
    fn config_default_clamps_workers() {
        let cfg = BuildConfig::default();
        assert!(cfg.workers >= 1);
        assert_eq!(cfg.cache_name, "build.cache");
    }

    #[test]
    fn missing_source_root_errors() {
        let manifest = BuildManifest {
            source_root: std::env::temp_dir().join("spawn-build-nope-zzz-does-not-exist"),
            output_dir: std::env::temp_dir().join("spawn-build-out-zzz"),
            include: vec![crate::glob::Pattern::compile("**").unwrap()],
            exclude: vec![],
        };
        assert!(matches!(
            BuildPipeline::new(manifest),
            Err(BuildError::SourceRootMissing { .. })
        ));
    }
}
