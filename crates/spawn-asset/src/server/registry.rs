//! Sharded lock map of `AssetId -> slot`.
//!
//! Slots are stored type-erased (`Arc<dyn ErasedSlot>`) so payloads of many
//! concrete types share one registry. `get`/`load_state` take a single shard
//! **read** lock; slot creation, commits, and unloads take a shard **write**
//! lock and happen only during setup or `apply_loaded`. Reads on one shard
//! never block reads or loads on another.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::error::AssetError;
use crate::handle::{Asset, Handle, HandleSlot};
use crate::id::AssetId;
use crate::loader::ErasedPayload;
use crate::server::LoadState;

/// Type-independent operations the pump needs on a slot through the registry.
pub(crate) trait ErasedSlot: Send + Sync {
    /// Commits an erased payload as the initial load. Returns `false` if the
    /// downcast fails (payload type mismatch) or the slot was already committed.
    fn commit_loaded_erased(&self, payload: ErasedPayload) -> bool;
    fn commit_failed(&self, error: AssetError);
    /// Swaps a reloaded erased payload in place, dropping the previous one.
    /// Returns the new generation, or `None` on downcast mismatch.
    fn reload_commit_erased(&self, payload: ErasedPayload) -> Option<u32>;
    fn reload_failed(&self, error: AssetError) -> u32;
}

impl<T: Asset> ErasedSlot for HandleSlot<T> {
    fn commit_loaded_erased(&self, payload: ErasedPayload) -> bool {
        match payload.downcast::<T>() {
            Ok(value) => self.commit_loaded(*value),
            Err(_) => false,
        }
    }
    fn commit_failed(&self, error: AssetError) {
        HandleSlot::commit_failed(self, error)
    }
    fn reload_commit_erased(&self, payload: ErasedPayload) -> Option<u32> {
        match payload.downcast::<T>() {
            Ok(value) => Some(self.reload_commit(*value)),
            Err(_) => None,
        }
    }
    fn reload_failed(&self, error: AssetError) -> u32 {
        HandleSlot::reload_failed(self, error)
    }
}

/// One registry entry: the erased slot plus the original typed `Arc` kept as
/// `Arc<dyn Any>` so handles can be re-derived on dedup via downcast.
struct Entry {
    erased: Arc<dyn ErasedSlot>,
    typed: Arc<dyn std::any::Any + Send + Sync>,
}

pub(crate) struct Registry {
    shards: Vec<RwLock<HashMap<AssetId, Entry>>>,
    mask: usize,
}

impl Registry {
    pub(crate) fn new(io_threads: usize) -> Self {
        let target = (io_threads.max(1) * 4).max(16);
        let count = target.next_power_of_two();
        let mut shards = Vec::with_capacity(count);
        for _ in 0..count {
            shards.push(RwLock::new(HashMap::new()));
        }
        Self {
            shards,
            mask: count - 1,
        }
    }

    fn shard(&self, id: AssetId) -> &RwLock<HashMap<AssetId, Entry>> {
        let index = (id.raw() as usize) & self.mask;
        &self.shards[index]
    }

    /// Returns an existing handle for `id` if one is present and its stored type
    /// matches `T`. `Err(true)` signals a present entry whose type differs
    /// (only possible on a genuine `AssetId` collision across types).
    pub(crate) fn get_existing<T: Asset>(&self, id: AssetId) -> Result<Option<Handle<T>>, ()> {
        let guard = match self.shard(id).read() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        match guard.get(&id) {
            None => Ok(None),
            Some(entry) => match Arc::downcast::<HandleSlot<T>>(Arc::clone(&entry.typed)) {
                Ok(slot) => Ok(Some(Handle::new(id, slot))),
                Err(_) => Err(()),
            },
        }
    }

    /// Inserts a freshly created slot and returns its handle. If another thread
    /// raced an insert for the same id with a matching type, the existing handle
    /// is returned. A type mismatch on an existing entry yields `Err`.
    pub(crate) fn insert<T: Asset>(&self, id: AssetId, state: LoadState) -> Result<Handle<T>, ()> {
        let mut guard = match self.shard(id).write() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(entry) = guard.get(&id) {
            return match Arc::downcast::<HandleSlot<T>>(Arc::clone(&entry.typed)) {
                Ok(slot) => Ok(Handle::new(id, slot)),
                Err(_) => Err(()),
            };
        }
        let slot = Arc::new(HandleSlot::<T>::new(id, state));
        let typed: Arc<dyn std::any::Any + Send + Sync> = slot.clone();
        let erased: Arc<dyn ErasedSlot> = slot.clone();
        guard.insert(id, Entry { erased, typed });
        Ok(Handle::new(id, slot))
    }

    /// Runs `f` against the erased slot for `id` under a read lock, if present.
    pub(crate) fn with_slot<R>(
        &self,
        id: AssetId,
        f: impl FnOnce(&Arc<dyn ErasedSlot>) -> R,
    ) -> Option<R> {
        let guard = match self.shard(id).read() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        guard.get(&id).map(|entry| f(&entry.erased))
    }

    /// Frees slots whose only remaining strong reference is the registry's own.
    /// Returns the number of slots unloaded. Takes write locks per shard.
    pub(crate) fn collect_unloaded(&self) -> usize {
        let mut freed = 0;
        for shard in &self.shards {
            let mut guard = match shard.write() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            let before = guard.len();
            // The registry itself holds two strong references per entry
            // (`typed` and `erased`, both clones of the same allocation), so a
            // strong count of exactly 2 means no external handle remains.
            guard.retain(|_, entry| Arc::strong_count(&entry.typed) > 2);
            freed += before - guard.len();
        }
        freed
    }

    /// All currently tracked asset ids. Used by the watcher pump to decide
    /// whether a filesystem change targets a known asset.
    pub(crate) fn contains(&self, id: AssetId) -> bool {
        let guard = match self.shard(id).read() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        guard.contains_key(&id)
    }
}
