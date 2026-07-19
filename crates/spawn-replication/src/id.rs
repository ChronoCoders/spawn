//! `ReplId`, the dense, recycled replicated-entity id, and the bidirectional
//! `Entity` ↔ `ReplId` map kept on both ends.
//!
//! The id is the on-wire entity handle. The space is kept **dense** by recycling
//! freed ids through a free-list, so the id space tracks the *peak concurrent live*
//! count rather than the total ever spawned, which keeps the per-client visibility
//! bitsets (indexed by `ReplId`) compact (validated in the IM prototype, claim 5).
//!
//! The **server** allocates ids ([`allocate`](ReplIdMap::allocate)); the **client**
//! binds server-assigned ids to its local proxy entities ([`bind`](ReplIdMap::bind)).

use std::collections::HashMap;

use spawn_ecs::Entity;

/// Dense replicated-entity id (the on-wire entity handle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReplId(pub u32);

impl ReplId {
    /// The id as an array index.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Bidirectional `Entity` ↔ `ReplId` map with a free-list for dense recycling.
#[derive(Default)]
pub struct ReplIdMap {
    /// `ReplId` → `Entity`; `None` marks a freed (recyclable) slot.
    entity_of: Vec<Option<Entity>>,
    /// `Entity` → `ReplId`.
    id_of: HashMap<Entity, ReplId>,
    /// Freed ids available for reuse (server allocation path only).
    free: Vec<u32>,
    live: usize,
}

impl ReplIdMap {
    /// An empty map.
    pub fn new() -> Self {
        Self::default()
    }

    /// **Server:** assign a fresh (or recycled) id to `entity`. If `entity` already has
    /// an id, that id is returned unchanged.
    pub fn allocate(&mut self, entity: Entity) -> ReplId {
        if let Some(&id) = self.id_of.get(&entity) {
            return id;
        }
        let raw = match self.free.pop() {
            Some(raw) => {
                self.entity_of[raw as usize] = Some(entity);
                raw
            }
            None => {
                let raw = self.entity_of.len() as u32;
                self.entity_of.push(Some(entity));
                raw
            }
        };
        let id = ReplId(raw);
        self.id_of.insert(entity, id);
        self.live += 1;
        id
    }

    /// **Client:** bind a server-assigned `id` to a local `entity`, growing the id
    /// space as needed. Keeps the bidirectional map consistent: any prior binding of
    /// `id` *or* of `entity` (even to a different id) is cleared first, so no stale
    /// reverse mapping is left dangling and `live` is never miscounted.
    pub fn bind(&mut self, id: ReplId, entity: Entity) {
        // Clear any existing binding of this entity (possibly to a different id); the
        // slot is re-established below.
        if let Some(prev_id) = self.id_of.remove(&entity) {
            self.entity_of[prev_id.index()] = None;
            self.live -= 1;
        }
        let idx = id.index();
        if idx >= self.entity_of.len() {
            self.entity_of.resize(idx + 1, None);
        }
        // Clear any existing binding of this id to a different entity.
        if let Some(prev_entity) = self.entity_of[idx] {
            self.id_of.remove(&prev_entity);
            self.live -= 1;
        }
        self.entity_of[idx] = Some(entity);
        self.id_of.insert(entity, id);
        self.live += 1;
    }

    /// **Server:** release `entity`'s id back to the free-list. Returns the freed id.
    pub fn release(&mut self, entity: Entity) -> Option<ReplId> {
        let id = self.id_of.remove(&entity)?;
        self.entity_of[id.index()] = None;
        self.free.push(id.0);
        self.live -= 1;
        Some(id)
    }

    /// **Client:** release a server-assigned `id`. Returns the entity it was bound to.
    /// Does not push to the free-list (the client does not allocate ids).
    pub fn release_id(&mut self, id: ReplId) -> Option<Entity> {
        let idx = id.index();
        let entity = self.entity_of.get(idx).copied().flatten()?;
        self.entity_of[idx] = None;
        self.id_of.remove(&entity);
        self.live -= 1;
        Some(entity)
    }

    /// The id bound to `entity`, if any.
    pub fn get(&self, entity: Entity) -> Option<ReplId> {
        self.id_of.get(&entity).copied()
    }

    /// The entity bound to `id`, if any.
    pub fn entity(&self, id: ReplId) -> Option<Entity> {
        self.entity_of.get(id.index()).copied().flatten()
    }

    /// Iterate the live `(Entity, ReplId)` bindings (unordered). Used by the server
    /// driver to detect entities that left replication.
    pub fn entities(&self) -> impl Iterator<Item = (Entity, ReplId)> + '_ {
        self.id_of.iter().map(|(&e, &id)| (e, id))
    }

    /// The id-space high-water mark, the count to size `ReplId`-indexed bitsets to.
    pub fn capacity(&self) -> usize {
        self.entity_of.len()
    }

    /// The number of currently-bound ids.
    pub fn live(&self) -> usize {
        self.live
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A throwaway entity allocator for tests: spawn entities from a World.
    fn entities(n: usize) -> Vec<Entity> {
        let mut w = spawn_ecs::World::new();
        (0..n).map(|_| w.spawn()).collect()
    }

    #[test]
    fn allocate_is_stable_per_entity() {
        let e = entities(2);
        let mut m = ReplIdMap::new();
        let a = m.allocate(e[0]);
        assert_eq!(m.allocate(e[0]), a, "same entity keeps its id");
        let b = m.allocate(e[1]);
        assert_ne!(a, b);
        assert_eq!(m.get(e[0]), Some(a));
        assert_eq!(m.entity(b), Some(e[1]));
        assert_eq!(m.live(), 2);
    }

    #[test]
    fn release_recycles_ids_keeping_the_space_dense() {
        let es = entities(100);
        let mut m = ReplIdMap::new();
        for &e in &es {
            m.allocate(e);
        }
        assert_eq!(m.capacity(), 100);
        // Free half, then allocate 50 fresh entities: capacity must not grow.
        for &e in es.iter().take(50) {
            m.release(e);
        }
        assert_eq!(m.live(), 50);
        let more = entities(50);
        for &e in &more {
            m.allocate(e);
        }
        assert_eq!(m.live(), 100);
        assert_eq!(m.capacity(), 100, "recycled ids keep the space dense");
    }

    #[test]
    fn released_id_does_not_alias_a_live_entity() {
        let es = entities(3);
        let mut m = ReplIdMap::new();
        let a = m.allocate(es[0]);
        m.allocate(es[1]);
        m.release(es[0]);
        assert_eq!(m.entity(a), None, "freed slot resolves to no entity");
        // The next allocation reuses the freed id but binds the new entity.
        let reused = m.allocate(es[2]);
        assert_eq!(reused, a);
        assert_eq!(m.entity(a), Some(es[2]));
    }

    #[test]
    fn client_bind_and_release_roundtrip() {
        let es = entities(2);
        let mut m = ReplIdMap::new();
        m.bind(ReplId(7), es[0]);
        assert_eq!(m.entity(ReplId(7)), Some(es[0]));
        assert_eq!(m.get(es[0]), Some(ReplId(7)));
        assert_eq!(m.capacity(), 8, "space grew to hold id 7");
        assert_eq!(m.release_id(ReplId(7)), Some(es[0]));
        assert_eq!(m.entity(ReplId(7)), None);
        assert_eq!(m.live(), 0);
        // Rebinding the same id to a new entity is clean.
        m.bind(ReplId(7), es[1]);
        assert_eq!(m.entity(ReplId(7)), Some(es[1]));
        assert_eq!(m.live(), 1);
    }

    #[test]
    fn rebinding_an_entity_to_a_new_id_leaves_no_stale_slot() {
        let es = entities(1);
        let mut m = ReplIdMap::new();
        m.bind(ReplId(3), es[0]);
        // Same entity now arrives under a different id (e.g. a re-spawn on the wire).
        m.bind(ReplId(9), es[0]);
        assert_eq!(m.get(es[0]), Some(ReplId(9)));
        assert_eq!(m.entity(ReplId(9)), Some(es[0]));
        assert_eq!(
            m.entity(ReplId(3)),
            None,
            "old slot cleared, no dangling reverse map"
        );
        assert_eq!(m.live(), 1, "live counts the entity once, not twice");
    }
}
