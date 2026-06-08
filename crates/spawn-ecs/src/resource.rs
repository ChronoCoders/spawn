//! World-owned singleton storage: one value per type, reached through `Res<T>`
//! (shared) and `ResMut<T>` (exclusive).
//!
//! Each resource is stored behind a `std::sync::RwLock<T>`. A scheduled system
//! holds only `&World`, shared across the scoped threads of a batch, so handing
//! out `&mut T` in safe Rust requires `Sync` interior mutability — this is the
//! deliberate alternative to an `unsafe` cell projection, keeping the crate at
//! zero `unsafe`. The scheduler's conflict relation guarantees the lock is never
//! contended (a `ResMut<T>` system is alone in its batch), so it never blocks;
//! lock poisoning is recovered with `into_inner`, so no access can panic.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Marker trait for types stored as a world singleton resource.
///
/// No blanket impl: each resource type opts in with `impl Resource for T {}`, so
/// the full set of world singletons stays auditable. `Send + Sync + 'static` is
/// what makes a resource safe behind the shared `RwLock` reached through
/// `&World` across scoped threads. A type may be both a `Component` and a
/// `Resource`; the two storages are independent.
pub trait Resource: Send + Sync + 'static {}

/// Dense, contiguous identifier assigned to a resource type on first insertion,
/// starting at `0`, in a namespace separate from `ComponentId`. Canonical key
/// for resource storage slots and for the resource access masks.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceId(u32);

impl ResourceId {
    pub(crate) const fn new(raw: u32) -> Self {
        Self(raw)
    }

    pub(crate) const fn index(self) -> usize {
        self.0 as usize
    }
}

/// A type-erased resource cell. The concrete cell for resource `T` is
/// `RwLock<T>`; access downcasts back to `&RwLock<T>` via `Any`.
trait ResourceCell: Send + Sync {
    fn as_any(&self) -> &(dyn Any + Send + Sync);
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
}

impl<T: Resource> ResourceCell for RwLock<T> {
    fn as_any(&self) -> &(dyn Any + Send + Sync) {
        self
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

/// World-owned resource store: a dense id per type plus a slot vector. A `None`
/// slot is a registered-but-absent resource (inserted then removed); its id is
/// retained so ids stay dense and stable.
pub(crate) struct Resources {
    by_type: HashMap<TypeId, ResourceId>,
    cells: Vec<Option<Box<dyn ResourceCell>>>,
}

impl Resources {
    pub(crate) fn new() -> Self {
        Self {
            by_type: HashMap::new(),
            cells: Vec::new(),
        }
    }

    pub(crate) fn id_of<T: Resource>(&self) -> Option<ResourceId> {
        self.by_type.get(&TypeId::of::<T>()).copied()
    }

    /// Assigns (or returns) `T`'s id without storing a value.
    fn register<T: Resource>(&mut self) -> ResourceId {
        let tid = TypeId::of::<T>();
        if let Some(id) = self.by_type.get(&tid) {
            return *id;
        }
        let id = ResourceId::new(self.cells.len() as u32);
        self.cells.push(None);
        self.by_type.insert(tid, id);
        id
    }

    pub(crate) fn insert<T: Resource>(&mut self, value: T) {
        let id = self.register::<T>();
        self.cells[id.index()] = Some(Box::new(RwLock::new(value)));
    }

    pub(crate) fn remove<T: Resource>(&mut self) -> Option<T> {
        let id = self.id_of::<T>()?;
        let cell = self.cells.get_mut(id.index())?.take()?;
        let lock = cell.into_any().downcast::<RwLock<T>>().ok()?;
        Some(lock.into_inner().unwrap_or_else(|e| e.into_inner()))
    }

    pub(crate) fn contains<T: Resource>(&self) -> bool {
        match self.id_of::<T>() {
            Some(id) => self
                .cells
                .get(id.index())
                .map(|slot| slot.is_some())
                .unwrap_or(false),
            None => false,
        }
    }

    pub(crate) fn get<T: Resource>(&self) -> Option<Res<'_, T>> {
        let id = self.id_of::<T>()?;
        let cell = self.cells.get(id.index())?.as_ref()?;
        let lock = cell.as_any().downcast_ref::<RwLock<T>>()?;
        Some(Res {
            guard: lock.read().unwrap_or_else(|e| e.into_inner()),
        })
    }

    pub(crate) fn get_mut<T: Resource>(&self) -> Option<ResMut<'_, T>> {
        let id = self.id_of::<T>()?;
        let cell = self.cells.get(id.index())?.as_ref()?;
        let lock = cell.as_any().downcast_ref::<RwLock<T>>()?;
        Some(ResMut {
            guard: lock.write().unwrap_or_else(|e| e.into_inner()),
        })
    }
}

/// Shared access to a resource. Multiple `Res<T>` may exist concurrently across
/// a batch (the underlying `RwLock` permits shared readers).
pub struct Res<'w, T: Resource> {
    guard: RwLockReadGuard<'w, T>,
}

impl<T: Resource> Deref for Res<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.guard
    }
}

/// Exclusive access to a resource. The scheduler guarantees this is the only
/// live accessor of `T` in its batch.
pub struct ResMut<'w, T: Resource> {
    guard: RwLockWriteGuard<'w, T>,
}

impl<T: Resource> Deref for ResMut<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T: Resource> DerefMut for ResMut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const _: () = assert!(std::mem::size_of::<ResourceId>() == 4);

    struct Counter(u32);
    impl Resource for Counter {}
    struct Flag;
    impl Resource for Flag {}

    #[test]
    fn insert_get_overwrite_remove() {
        let mut res = Resources::new();
        assert!(!res.contains::<Counter>());
        res.insert(Counter(1));
        assert!(res.contains::<Counter>());
        assert_eq!(res.get::<Counter>().unwrap().0, 1);

        res.insert(Counter(9));
        assert_eq!(res.get::<Counter>().unwrap().0, 9);

        let taken = res.remove::<Counter>().unwrap();
        assert_eq!(taken.0, 9);
        assert!(!res.contains::<Counter>());
        assert!(res.get::<Counter>().is_none());
        assert!(res.remove::<Counter>().is_none());
    }

    #[test]
    fn resmut_mutates_in_place() {
        let mut res = Resources::new();
        res.insert(Counter(0));
        {
            let mut m = res.get_mut::<Counter>().unwrap();
            m.0 += 5;
        }
        assert_eq!(res.get::<Counter>().unwrap().0, 5);
    }

    #[test]
    fn concurrent_shared_reads_coexist() {
        let mut res = Resources::new();
        res.insert(Counter(7));
        let a = res.get::<Counter>().unwrap();
        let b = res.get::<Counter>().unwrap();
        assert_eq!(a.0 + b.0, 14);
    }

    #[test]
    fn ids_are_dense_and_stable_across_remove() {
        let mut res = Resources::new();
        res.insert(Counter(0));
        res.insert(Flag);
        let counter_id = res.id_of::<Counter>().unwrap();
        let flag_id = res.id_of::<Flag>().unwrap();
        assert_eq!(counter_id.index(), 0);
        assert_eq!(flag_id.index(), 1);
        res.remove::<Counter>();
        res.insert(Counter(3));
        assert_eq!(res.id_of::<Counter>().unwrap(), counter_id);
    }
}
