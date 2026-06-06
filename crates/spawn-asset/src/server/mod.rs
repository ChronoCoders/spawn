//! The [`AssetServer`]: non-blocking loading, lock-light access, and the
//! single main-thread [`AssetServer::apply_loaded`] synchronization point.
//!
//! Threading contract: `load`, `get`, `load_state`, `error`,
//! `reborrow_or_load`, and `drain_reload_events` are cheap and may be called
//! between frames; none of them mutate asset state visible to `get`. All such
//! mutation — committing loaded/failed payloads, applying hot-reload
//! replacements, and freeing unloaded slots — happens only inside
//! `apply_loaded`, which is expected to run once per frame on the main thread.

mod pool;
mod registry;

use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use crate::error::{AssetError, AssetResult};
use crate::handle::Asset;
use crate::handle::Handle;
use crate::id::{canonicalize, AssetId};
use crate::loader::{AssetLoader, ErasedLoader, LoaderShim};
use crate::watch::{HotReload, ReloadEvent, ReloadOutcome};

use pool::{IoPool, JobResult, LoadJob, LoaderTable};
use registry::Registry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadState {
    NotLoaded,
    Loading,
    Loaded,
    Failed,
}

pub struct AssetServerConfig {
    pub root: PathBuf,
    pub io_threads: usize,
    pub queue_capacity: usize,
    pub hot_reload: bool,
    pub debounce: Duration,
}

impl Default for AssetServerConfig {
    fn default() -> Self {
        Self {
            root: PathBuf::from("assets"),
            io_threads: 2,
            queue_capacity: 256,
            hot_reload: true,
            debounce: Duration::from_millis(100),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AppliedReport {
    pub loaded: usize,
    pub failed: usize,
    pub reloaded: usize,
    pub unloaded: usize,
}

pub struct AssetServer {
    root: PathBuf,
    registry: Registry,
    loaders: LoaderTable,
    pool: Option<IoPool>,
    hot_reload: Option<Mutex<HotReload>>,
    deferred: Mutex<VecDeque<LoadJob>>,
    paths: Mutex<HashMap<AssetId, (PathBuf, String, String)>>,
    reload_events: Mutex<Vec<ReloadEvent>>,
}

impl AssetServer {
    /// Constructs the server: validates the asset root exists, spawns the IO
    /// pool, and (when `config.hot_reload`) starts the filesystem watcher.
    /// Returns [`AssetError::NotFound`] if the root is missing or
    /// [`AssetError::WatcherInit`] if the watcher fails to start.
    pub fn new(config: AssetServerConfig) -> AssetResult<Self> {
        if !config.root.is_dir() {
            return Err(AssetError::NotFound {
                path: config.root.to_string_lossy().into_owned(),
            });
        }
        let registry = Registry::new(config.io_threads);
        let loaders: LoaderTable = Arc::new(RwLock::new(HashMap::new()));
        let pool = IoPool::new(
            config.io_threads,
            config.queue_capacity,
            Arc::clone(&loaders),
        );
        let hot_reload = if config.hot_reload {
            Some(Mutex::new(HotReload::new(&config.root, config.debounce)?))
        } else {
            None
        };
        Ok(Self {
            root: config.root,
            registry,
            loaders,
            pool: Some(pool),
            hot_reload,
            deferred: Mutex::new(VecDeque::new()),
            paths: Mutex::new(HashMap::new()),
            reload_events: Mutex::new(Vec::new()),
        })
    }

    /// Registers a loader under each of its extensions. Main-thread setup only,
    /// before loading begins. Returns [`AssetError::DuplicateLoader`] if any
    /// extension is already claimed; on conflict no extension is registered.
    pub fn register_loader<L: AssetLoader>(&mut self, loader: L) -> AssetResult<()> {
        let extensions: Vec<String> = loader
            .extensions()
            .iter()
            .map(|e| e.to_ascii_lowercase())
            .collect();
        let mut table = match self.loaders.write() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        for ext in &extensions {
            if table.contains_key(ext) {
                return Err(AssetError::DuplicateLoader {
                    extension: ext.clone(),
                });
            }
        }
        let shim: Arc<dyn ErasedLoader> = Arc::new(LoaderShim(loader));
        for ext in extensions {
            table.insert(ext, Arc::clone(&shim));
        }
        Ok(())
    }

    fn has_loader(&self, extension: &str) -> bool {
        let table = match self.loaders.read() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        table.contains_key(extension)
    }

    fn canon(&self, path: &str) -> (AssetId, String, String, PathBuf) {
        let canonical = canonicalize(path);
        let id = AssetId::from_canonical_path(&canonical);
        let extension = Path::new(&canonical)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        let abs = self.root.join(&canonical);
        (id, canonical, extension, abs)
    }

    /// Non-blocking. Canonicalizes `path`, dedups by [`AssetId`], and returns a
    /// handle immediately. An already-known asset returns a clone of the
    /// existing handle. A path whose extension has no registered loader is
    /// returned directly in `Failed` with [`AssetError::NoLoader`]. Never
    /// blocks and never returns a `Result`; failure is observed via
    /// [`AssetServer::load_state`].
    pub fn load<T: Asset>(&self, path: &str) -> Handle<T> {
        let (id, canonical, extension, abs) = self.canon(path);

        if let Ok(Some(existing)) = self.registry.get_existing::<T>(id) {
            return existing;
        }

        if !self.has_loader(&extension) {
            let handle = self.make_slot::<T>(id, LoadState::Failed);
            self.registry.with_slot(id, |slot| {
                slot.commit_failed(AssetError::NoLoader {
                    extension: extension.clone(),
                });
            });
            return handle;
        }

        let handle = self.make_slot::<T>(id, LoadState::Loading);
        if let Ok(mut paths) = self.paths.lock() {
            paths.insert(id, (abs.clone(), canonical.clone(), extension.clone()));
        }
        self.submit_or_defer(LoadJob {
            id,
            abs_path: abs,
            canonical_path: canonical,
            extension,
            is_reload: false,
        });
        handle
    }

    fn make_slot<T: Asset>(&self, id: AssetId, state: LoadState) -> Handle<T> {
        // A type mismatch on an existing entry implies an `AssetId` collision
        // across distinct canonical paths/types. Surface it as a failed handle
        // rather than panicking.
        match self.registry.insert::<T>(id, state) {
            Ok(handle) => handle,
            Err(()) => self.fallback_failed_handle::<T>(id),
        }
    }

    fn fallback_failed_handle<T: Asset>(&self, id: AssetId) -> Handle<T> {
        // Constructs a detached failed handle when the registry cannot hold the
        // typed slot. The slot still reports `Failed` via its own state.
        let slot = Arc::new(crate::handle::HandleSlot::<T>::new(id, LoadState::Failed));
        slot.commit_failed(AssetError::IdCollision {
            path_a: String::new(),
            path_b: String::new(),
        });
        Handle::new(id, slot)
    }

    fn submit_or_defer(&self, job: LoadJob) {
        if let Some(pool) = &self.pool {
            if let Err(job) = pool.try_submit(job) {
                if let Ok(mut deferred) = self.deferred.lock() {
                    deferred.push_back(job);
                }
            }
        }
    }

    /// Returns a clone of the payload `Arc` only when the asset is `Loaded`;
    /// `None` for every other state. Lock-light: a single shard-free read lock on
    /// the handle's own slot, an `Arc` refcount bump, no payload copy. The
    /// returned `Arc` is a stable snapshot — a concurrent reload swap leaves it
    /// pointing at the data it observed.
    pub fn get<T: Asset>(&self, handle: &Handle<T>) -> Option<Arc<T>> {
        handle.slot().payload()
    }

    /// Cheap snapshot of the handle's current load state.
    pub fn load_state<T: Asset>(&self, handle: &Handle<T>) -> LoadState {
        handle.slot().state()
    }

    /// Returns a copy of the retained error for a `Failed` slot, else `None`.
    pub fn error<T: Asset>(&self, handle: &Handle<T>) -> Option<AssetError> {
        handle.slot().error()
    }

    /// Ergonomic alias for [`AssetServer::load`] with identical dedup semantics.
    pub fn reborrow_or_load<T: Asset>(&self, path: &str) -> Handle<T> {
        self.load(path)
    }

    /// The single synchronization point. Drains completed IO jobs and debounced
    /// reloads, commits payloads (publishing `Loaded`/`Failed`), applies
    /// in-place reload replacements, retries deferred-pending loads against the
    /// bounded queue, and frees slots whose last strong handle was dropped.
    /// Returns a per-pump [`AppliedReport`].
    pub fn apply_loaded(&self) -> AppliedReport {
        let mut report = AppliedReport::default();
        let mut events: Vec<ReloadEvent> = Vec::new();

        self.pump_reload_queue();
        self.retry_deferred();

        if let Some(pool) = &self.pool {
            for result in pool.drain_results() {
                match result {
                    JobResult::Loaded {
                        id,
                        payload,
                        is_reload,
                    } => {
                        if is_reload {
                            let gen = self
                                .registry
                                .with_slot(id, |slot| slot.reload_commit_erased(payload));
                            if let Some(Some(generation)) = gen {
                                report.reloaded += 1;
                                events.push(ReloadEvent {
                                    id,
                                    generation,
                                    outcome: ReloadOutcome::Replaced,
                                });
                            }
                        } else {
                            let ok = self
                                .registry
                                .with_slot(id, |slot| slot.commit_loaded_erased(payload));
                            if ok == Some(true) {
                                report.loaded += 1;
                            }
                        }
                    }
                    JobResult::Failed {
                        id,
                        error,
                        is_reload,
                    } => {
                        if is_reload {
                            let gen = self
                                .registry
                                .with_slot(id, |slot| slot.reload_failed(error));
                            if let Some(generation) = gen {
                                report.failed += 1;
                                events.push(ReloadEvent {
                                    id,
                                    generation,
                                    outcome: ReloadOutcome::Failed,
                                });
                            }
                        } else {
                            self.registry
                                .with_slot(id, |slot| slot.commit_failed(error));
                            report.failed += 1;
                        }
                    }
                }
            }
        }

        report.unloaded = self.registry.collect_unloaded();
        if report.unloaded > 0 {
            self.prune_paths();
        }

        if let Ok(mut sink) = self.reload_events.lock() {
            *sink = events;
        }
        report
    }

    fn pump_reload_queue(&self) {
        let Some(hot) = &self.hot_reload else {
            return;
        };
        let ready = {
            let mut guard = match hot.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            guard.collect_ready(Instant::now())
        };
        for canonical in ready {
            let id = AssetId::from_canonical_path(&canonical);
            if !self.registry.contains(id) {
                continue;
            }
            let info = self.paths.lock().ok().and_then(|p| p.get(&id).cloned());
            let Some((abs, canonical_path, extension)) = info else {
                continue;
            };
            self.submit_or_defer(LoadJob {
                id,
                abs_path: abs,
                canonical_path,
                extension,
                is_reload: true,
            });
        }
    }

    fn retry_deferred(&self) {
        let pending: Vec<LoadJob> = {
            let mut guard = match self.deferred.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            guard.drain(..).collect()
        };
        for job in pending {
            self.submit_or_defer(job);
        }
    }

    fn prune_paths(&self) {
        if let Ok(mut paths) = self.paths.lock() {
            paths.retain(|id, _| self.registry.contains(*id));
        }
    }

    /// Drains the [`ReloadEvent`]s produced by the most recent
    /// [`AssetServer::apply_loaded`]. Empty when hot-reload is disabled or no
    /// reload occurred. Owners poll this to react (e.g. rebuild GPU resources).
    pub fn drain_reload_events(&self) -> Vec<ReloadEvent> {
        match self.reload_events.lock() {
            Ok(mut guard) => std::mem::take(&mut *guard),
            Err(p) => std::mem::take(&mut *p.into_inner()),
        }
    }
}

impl Drop for AssetServer {
    fn drop(&mut self) {
        // Join IO threads (pool Drop) and stop the watcher (HotReload Drop)
        // cleanly; no detached threads survive.
        self.pool = None;
        self.hot_reload = None;
    }
}
