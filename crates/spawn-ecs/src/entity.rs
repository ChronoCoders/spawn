//! Entity identity and the generation-recycling allocator.
//!
//! An [`Entity`] is an opaque `(index, generation)` pair. The `generation` is
//! bumped each time a slot is recycled so a stale handle can never alias a live
//! entity. Identities are only ever constructed by the allocator (and thus by
//! `World`); there is no public constructor.

use std::sync::atomic::{AtomicU32, Ordering};

/// An opaque handle to an entity.
///
/// Identity is `(index, generation)`: `index` selects an allocator slot and
/// `generation` distinguishes successive occupants of that slot, so a handle
/// kept past its entity's despawn is reliably rejected rather than aliasing a
/// later entity.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Entity {
    index: u32,
    generation: u32,
}

impl Entity {
    /// Reserved sentinel (`index = u32::MAX`, `generation = 0`). Never live and
    /// never returned by `spawn`; lets downstream code default-initialize entity
    /// fields without `Option`.
    pub const PLACEHOLDER: Entity = Entity {
        index: u32::MAX,
        generation: 0,
    };

    #[inline]
    pub const fn index(self) -> u32 {
        self.index
    }

    #[inline]
    pub const fn generation(self) -> u32 {
        self.generation
    }

    /// Returns `true` iff this is the reserved [`Entity::PLACEHOLDER`] sentinel.
    #[inline]
    pub const fn is_placeholder(self) -> bool {
        self.index == u32::MAX && self.generation == 0
    }

    pub(crate) const fn from_raw(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }
}

/// Per-slot allocation state.
#[derive(Clone, Copy)]
struct Slot {
    generation: u32,
    live: bool,
    /// Once a slot's generation saturates it is retired and never recycled.
    retired: bool,
}

/// Allocates entity identities with generation recycling.
///
/// Reserved ids (from `Commands::spawn`) come from `next_reserved`, an atomic
/// high-water counter shared across concurrently-running systems, so they never
/// collide; those slots become live only when the command buffer is applied.
pub(crate) struct EntityAllocator {
    slots: Vec<Slot>,
    free: Vec<u32>,
    live_count: usize,
    next_reserved: AtomicU32,
}

impl EntityAllocator {
    pub(crate) fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            live_count: 0,
            next_reserved: AtomicU32::new(0),
        }
    }

    /// Allocates a live entity, reusing a free slot (with a bumped generation)
    /// when available, otherwise extending the high-water mark.
    pub(crate) fn allocate(&mut self) -> Entity {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.live = true;
            self.live_count += 1;
            return Entity {
                index,
                generation: slot.generation,
            };
        }
        self.grow_one()
    }

    fn grow_one(&mut self) -> Entity {
        let index = self.slots.len() as u32;
        self.slots.push(Slot {
            generation: 0,
            live: true,
            retired: false,
        });
        self.sync_reserved(index + 1);
        self.live_count += 1;
        Entity {
            index,
            generation: 0,
        }
    }

    fn sync_reserved(&self, high_water: u32) {
        let mut current = self.next_reserved.load(Ordering::Relaxed);
        while current < high_water {
            match self.next_reserved.compare_exchange(
                current,
                high_water,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
    }

    /// Reserves an identity without placing it. Used by `Commands::spawn` from
    /// concurrently-running systems; the slot becomes live on buffer apply.
    pub(crate) fn reserve(&self) -> Entity {
        let index = self.next_reserved.fetch_add(1, Ordering::Relaxed);
        let generation = self
            .slots
            .get(index as usize)
            .map(|s| s.generation)
            .unwrap_or(0);
        Entity { index, generation }
    }

    /// Materializes a previously reserved identity into a live slot, growing the
    /// slot table to cover it. Returns `false` if the reservation is stale (the
    /// slot already advanced past this generation).
    pub(crate) fn materialize(&mut self, entity: Entity) -> bool {
        let index = entity.index as usize;
        while self.slots.len() <= index {
            self.slots.push(Slot {
                generation: 0,
                live: false,
                retired: false,
            });
        }
        let slot = &mut self.slots[index];
        if slot.live || slot.generation != entity.generation {
            return false;
        }
        slot.live = true;
        self.live_count += 1;
        self.sync_reserved(index as u32 + 1);
        true
    }

    pub(crate) fn is_live(&self, entity: Entity) -> bool {
        match self.slots.get(entity.index as usize) {
            Some(slot) => slot.live && slot.generation == entity.generation,
            None => false,
        }
    }

    /// Frees a live slot, bumping its generation. A slot whose generation would
    /// saturate at `u32::MAX` is retired (never recycled) to keep the
    /// no-aliasing guarantee; this leaks one `u32` slot, documented as acceptable.
    /// Returns `false` if the entity was not live.
    pub(crate) fn free(&mut self, entity: Entity) -> bool {
        let index = entity.index as usize;
        let slot = match self.slots.get_mut(index) {
            Some(slot) => slot,
            None => return false,
        };
        if !slot.live || slot.generation != entity.generation {
            return false;
        }
        slot.live = false;
        self.live_count -= 1;
        if slot.generation == u32::MAX {
            slot.retired = true;
        } else {
            slot.generation += 1;
            if !slot.retired {
                self.free.push(entity.index);
            }
        }
        true
    }

    pub(crate) fn live_count(&self) -> usize {
        self.live_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const _: () = assert!(std::mem::size_of::<Entity>() == 8);
    const _: () = assert!(std::mem::align_of::<Entity>() == 4);

    #[test]
    fn placeholder_is_never_live() {
        let alloc = EntityAllocator::new();
        assert!(Entity::PLACEHOLDER.is_placeholder());
        assert!(!alloc.is_live(Entity::PLACEHOLDER));
    }

    #[test]
    fn fresh_slots_start_at_generation_zero() {
        let mut alloc = EntityAllocator::new();
        let a = alloc.allocate();
        let b = alloc.allocate();
        assert_eq!(a.index(), 0);
        assert_eq!(a.generation(), 0);
        assert_eq!(b.index(), 1);
        assert_eq!(alloc.live_count(), 2);
    }

    #[test]
    fn recycle_reuses_index_with_bumped_generation() {
        let mut alloc = EntityAllocator::new();
        let a = alloc.allocate();
        assert!(alloc.free(a));
        let b = alloc.allocate();
        assert_eq!(a.index(), b.index());
        assert_eq!(b.generation(), a.generation() + 1);
        assert!(!alloc.is_live(a));
        assert!(alloc.is_live(b));
    }

    #[test]
    fn stale_free_is_rejected() {
        let mut alloc = EntityAllocator::new();
        let a = alloc.allocate();
        assert!(alloc.free(a));
        assert!(!alloc.free(a));
    }

    #[test]
    fn generation_saturation_retires_slot() {
        let mut alloc = EntityAllocator::new();
        let e = alloc.allocate();
        alloc.slots[e.index() as usize].generation = u32::MAX;
        let stale = Entity {
            index: e.index(),
            generation: u32::MAX,
        };
        assert!(alloc.free(stale));
        assert!(alloc.slots[e.index() as usize].retired);
        let next = alloc.allocate();
        assert_ne!(next.index(), e.index());
    }

    #[test]
    fn reserve_then_materialize() {
        let mut alloc = EntityAllocator::new();
        let r = alloc.reserve();
        assert!(!alloc.is_live(r));
        assert!(alloc.materialize(r));
        assert!(alloc.is_live(r));
        assert!(!alloc.materialize(r));
    }
}
