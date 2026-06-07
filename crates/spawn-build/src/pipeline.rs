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

impl BuildPipeline {
    pub fn new(manifest: BuildManifest) -> BuildResult<Self> {
        Self::with_config(manifest, BuildConfig::default())
    }

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

    /// Compiles `to_compile` across exactly `config.workers` threads. Each
    /// worker pulls indices from a shared atomic counter (bounded, no unbounded
    /// spawning). Per-asset failures are collected, not propagated, so one
    /// failure does not poison the scope.
    fn compile_parallel(&self, to_compile: &[&AssetEntry]) -> Vec<CompileResult> {
        if to_compile.is_empty() {
            return Vec::new();
        }
        let next = AtomicUsize::new(0);
        let results: Mutex<Vec<CompileResult>> = Mutex::new(Vec::new());
        let workers = self.config.workers.min(to_compile.len()).max(1);

        std::thread::scope(|scope| {
            for _ in 0..workers {
                let next = &next;
                let results = &results;
                let manifest = &self.manifest;
                scope.spawn(move || loop {
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
                });
            }
        });

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
