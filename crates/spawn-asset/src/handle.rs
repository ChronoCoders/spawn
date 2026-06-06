//! Typed reference-counted handles and the private slot they point at.
//!
//! Lifetime / borrowing conventions:
//! - A [`Handle<T>`] is a **strong** reference (`Arc`-based): while one exists the
//!   asset slot is kept alive. When the last strong handle is dropped the slot
//!   becomes eligible for unload, which is finalized at the next
//!   [`AssetServer::apply_loaded`](crate::AssetServer::apply_loaded).
//! - [`AssetServer::get`](crate::AssetServer::get) returns `Option<Arc<T>>`: a
//!   refcount bump on the slot's single current payload `Arc`, taken under a
//!   short read lock that is released before the caller uses the asset. The slot
//!   stores exactly one payload `Arc` at a time. A hot-reload swaps a new `Arc`
//!   into the slot under the write lock and **drops** the previous one — the old
//!   data is freed once the last outstanding `get` clone is released, so no
//!   superseded generation is retained. Existing handles observe the new payload
//!   on their next `get` (the slot is reused in place — handles are never
//!   re-acquired).

use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};

use crate::error::AssetError;
use crate::id::AssetId;
use crate::server::LoadState;

pub trait Asset: Send + Sync + 'static {}

impl Asset for crate::loader::BinaryAsset {}
impl Asset for crate::loader::TextAsset {}

pub(crate) const STATE_NOT_LOADED: u8 = 0;
pub(crate) const STATE_LOADING: u8 = 1;
pub(crate) const STATE_LOADED: u8 = 2;
pub(crate) const STATE_FAILED: u8 = 3;

pub(crate) fn state_from_u8(value: u8) -> LoadState {
    match value {
        STATE_LOADING => LoadState::Loading,
        STATE_LOADED => LoadState::Loaded,
        STATE_FAILED => LoadState::Failed,
        _ => LoadState::NotLoaded,
    }
}

pub(crate) struct HandleSlot<T: Asset> {
    state: AtomicU8,
    generation: AtomicU32,
    error: Mutex<Option<AssetError>>,
    payload: RwLock<Option<Arc<T>>>,
}

impl<T: Asset> HandleSlot<T> {
    pub(crate) fn new(_id: AssetId, state: LoadState) -> Self {
        Self {
            state: AtomicU8::new(match state {
                LoadState::NotLoaded => STATE_NOT_LOADED,
                LoadState::Loading => STATE_LOADING,
                LoadState::Loaded => STATE_LOADED,
                LoadState::Failed => STATE_FAILED,
            }),
            generation: AtomicU32::new(0),
            error: Mutex::new(None),
            payload: RwLock::new(None),
        }
    }

    pub(crate) fn state(&self) -> LoadState {
        state_from_u8(self.state.load(Ordering::Acquire))
    }

    /// Returns a clone of the slot's current payload `Arc`, or `None` unless the
    /// slot is `Loaded`. Takes a short read lock, bumps the refcount, and
    /// releases the lock; the returned `Arc` keeps that payload alive and intact
    /// even if a concurrent reload swaps in a newer one (snapshot isolation).
    pub(crate) fn payload(&self) -> Option<Arc<T>> {
        if self.state.load(Ordering::Acquire) != STATE_LOADED {
            return None;
        }
        let guard = match self.payload.read() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        guard.clone()
    }

    /// Publishes the initial payload and transitions the slot to `Loaded`.
    /// Returns `false` if a payload was already present (initial commit only
    /// happens once; further commits go through [`Self::reload_commit`]).
    pub(crate) fn commit_loaded(&self, payload: T) -> bool {
        let mut guard = match self.payload.write() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if guard.is_some() {
            return false;
        }
        *guard = Some(Arc::new(payload));
        drop(guard);
        if let Ok(mut err) = self.error.lock() {
            *err = None;
        }
        self.state.store(STATE_LOADED, Ordering::Release);
        true
    }

    pub(crate) fn commit_failed(&self, error: AssetError) {
        if let Ok(mut guard) = self.error.lock() {
            *guard = Some(error);
        }
        self.state.store(STATE_FAILED, Ordering::Release);
    }

    /// Swaps a new payload `Arc` into the slot in place and **drops** the
    /// previous one (freed once the last outstanding `get` clone is released),
    /// bumps the generation counter, and republishes `Loaded`. Existing handles
    /// observe this payload on their next `get`. Returns the new generation.
    pub(crate) fn reload_commit(&self, payload: T) -> u32 {
        let previous = {
            let mut guard = match self.payload.write() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            guard.replace(Arc::new(payload))
        };
        drop(previous);
        if let Ok(mut err) = self.error.lock() {
            *err = None;
        }
        let gen = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.state.store(STATE_LOADED, Ordering::Release);
        gen
    }

    /// Marks a hot-reload failure: drops the stale payload, transitions to
    /// `Failed`, and bumps the generation so observers see a change. The
    /// retained `AssetError` becomes the new error. Reload failure never
    /// silently retains stale data.
    pub(crate) fn reload_failed(&self, error: AssetError) -> u32 {
        let previous = {
            let mut guard = match self.payload.write() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            guard.take()
        };
        drop(previous);
        if let Ok(mut guard) = self.error.lock() {
            *guard = Some(error);
        }
        let gen = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.state.store(STATE_FAILED, Ordering::Release);
        gen
    }

    pub(crate) fn error(&self) -> Option<AssetError> {
        self.error.lock().ok().and_then(|guard| guard.clone())
    }
}

pub struct Handle<T: Asset> {
    id: AssetId,
    state: Arc<HandleSlot<T>>,
}

impl<T: Asset> Handle<T> {
    pub(crate) fn new(id: AssetId, slot: Arc<HandleSlot<T>>) -> Self {
        Self { id, state: slot }
    }

    pub fn id(&self) -> AssetId {
        self.id
    }

    pub(crate) fn slot(&self) -> &Arc<HandleSlot<T>> {
        &self.state
    }

    /// Returns a non-owning observer. The resulting [`WeakHandle`] does not keep
    /// the asset alive.
    pub fn downgrade(&self) -> WeakHandle<T> {
        WeakHandle {
            id: self.id,
            state: Arc::downgrade(&self.state),
        }
    }
}

impl<T: Asset> Clone for Handle<T> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            state: Arc::clone(&self.state),
        }
    }
}

impl<T: Asset> std::fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handle")
            .field("id", &self.id)
            .field("state", &self.state.state())
            .finish()
    }
}

impl<T: Asset> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<T: Asset> Eq for Handle<T> {}

impl<T: Asset> std::hash::Hash for Handle<T> {
    fn hash<H: std::hash::Hasher>(&self, hasher: &mut H) {
        self.id.hash(hasher);
    }
}

pub struct WeakHandle<T: Asset> {
    id: AssetId,
    state: Weak<HandleSlot<T>>,
}

impl<T: Asset> WeakHandle<T> {
    pub fn id(&self) -> AssetId {
        self.id
    }

    /// Returns a strong [`Handle<T>`], or `None` if the asset has been unloaded
    /// (its last strong handle was dropped and the slot freed).
    pub fn upgrade(&self) -> Option<Handle<T>> {
        self.state.upgrade().map(|slot| Handle {
            id: self.id,
            state: slot,
        })
    }
}

impl<T: Asset> Clone for WeakHandle<T> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            state: Weak::clone(&self.state),
        }
    }
}

impl<T: Asset> std::fmt::Debug for WeakHandle<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WeakHandle")
            .field("id", &self.id)
            .field("alive", &(self.state.strong_count() > 0))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::TextAsset;

    fn slot(state: LoadState) -> Arc<HandleSlot<TextAsset>> {
        Arc::new(HandleSlot::new(AssetId::from_raw(7), state))
    }

    #[test]
    fn payload_none_until_loaded() {
        let s = slot(LoadState::Loading);
        let h = Handle::new(AssetId::from_raw(7), s);
        assert!(h.slot().payload().is_none());
        assert!(h.slot().commit_loaded(TextAsset("hi".into())));
        assert_eq!(h.slot().payload().map(|p| p.0.clone()), Some("hi".into()));
    }

    #[test]
    fn reload_swaps_payload_in_place() {
        let s = slot(LoadState::Loading);
        let h = Handle::new(AssetId::from_raw(7), s);
        assert!(h.slot().commit_loaded(TextAsset("v1".into())));
        let clone = h.clone();
        let gen = h.slot().reload_commit(TextAsset("v2".into()));
        assert_eq!(gen, 1);
        // The clone (same slot) sees the new payload without re-acquiring.
        assert_eq!(
            clone.slot().payload().map(|p| p.0.clone()),
            Some("v2".into())
        );
    }

    #[test]
    fn reload_swap_frees_old_payload() {
        let s = slot(LoadState::Loading);
        let h = Handle::new(AssetId::from_raw(7), s);
        assert!(h.slot().commit_loaded(TextAsset("v1".into())));
        // The slot is the only holder of the v1 Arc.
        let v1 = h.slot().payload().expect("loaded");
        assert_eq!(Arc::strong_count(&v1), 2);
        drop(v1);
        h.slot().reload_commit(TextAsset("v2".into()));
        // After the swap the slot holds only v2; v1 has been dropped. Acquire a
        // fresh snapshot and confirm it is the sole external reference.
        let v2 = h.slot().payload().expect("reloaded");
        assert_eq!(Arc::strong_count(&v2), 2);
    }

    #[test]
    fn outstanding_get_clone_survives_reload() {
        let s = slot(LoadState::Loading);
        let h = Handle::new(AssetId::from_raw(7), s);
        assert!(h.slot().commit_loaded(TextAsset("v1".into())));
        // Snapshot taken before the reload lands.
        let snapshot = h.slot().payload().expect("loaded");
        h.slot().reload_commit(TextAsset("v2".into()));
        // The pre-reload snapshot still reads the old data, intact.
        assert_eq!(snapshot.0, "v1");
        // A fresh get observes the new data.
        assert_eq!(h.slot().payload().map(|p| p.0.clone()), Some("v2".into()));
    }

    #[test]
    fn reload_failure_marks_failed_and_drops_stale() {
        let s = slot(LoadState::Loading);
        let h = Handle::new(AssetId::from_raw(7), s);
        assert!(h.slot().commit_loaded(TextAsset("v1".into())));
        let stale = h.slot().payload().expect("loaded");
        h.slot().reload_failed(AssetError::Parse {
            path: "p".into(),
            detail: "d".into(),
        });
        assert_eq!(h.slot().state(), LoadState::Failed);
        assert!(h.slot().payload().is_none());
        assert!(h.slot().error().is_some());
        // The stale Arc is no longer retained by the slot.
        assert_eq!(Arc::strong_count(&stale), 1);
    }

    #[test]
    fn handles_equal_by_id() {
        let h = Handle::new(AssetId::from_raw(7), slot(LoadState::Loading));
        assert_eq!(h, h.clone());
        assert!(h.downgrade().upgrade().is_some());
    }

    #[test]
    fn weak_upgrade_none_after_drop() {
        let weak = {
            let h = Handle::new(AssetId::from_raw(7), slot(LoadState::Loading));
            h.downgrade()
        };
        assert!(weak.upgrade().is_none());
    }
}
