//! Interest management: the validated layout (IM prototype §4b).
//!
//! All per-client visibility state lives in this **server-side resource**, never as
//! components on entities (validated: archetypes stay stable under client churn, and
//! the bit-packed matrix is far smaller than per-entity-per-client storage). Relevancy
//! class is expressed by the marker components (→ archetypes); this module reads them.
//!
//! Per tick: incrementally sync the spatial grid from the world, then for each client
//! gather a `(2·r_cells+1)²` cell block around its viewer and diff `current` vs
//! `previous` to emit spawn/despawn deltas. Defaults: `cell_size ≈ 0.5 × view_radius`
//! (the validated cost/overdraw optimum) and a 25% exit-radius hysteresis
//! (`exit_radius = 1.25 × view_radius`) that collapses boundary thrash. The gather block
//! is sized to the **exit** radius — `r_cells = ceil(exit_radius / cell_size)` — so a
//! hysteresis-band entity is still gathered and not spuriously despawned; at the default
//! ratio that is a 7×7 block (the bare enter radius alone would be a 5×5 block, but
//! hysteresis extends visibility past it).

use std::collections::HashMap;

use spawn_core::Transform3D;
use spawn_ecs::{Entity, Resource, World};
use spawn_net::ClientId;

use crate::id::{ReplId, ReplIdMap};
use crate::markers::{AlwaysRelevant, OwnerOnly, Replicated, StaticRelevant};

type Cell = (i32, i32);
/// Cache sentinel meaning "this id is not currently placed in the grid".
const UNPLACED: Cell = (i32::MIN, i32::MIN);

fn cell_of(x: f32, z: f32, cell_size: f32) -> Cell {
    (
        (x / cell_size).floor() as i32,
        (z / cell_size).floor() as i32,
    )
}

/// A dense bitset over `ReplId` index space.
#[derive(Default)]
struct BitSet {
    words: Vec<u64>,
}

impl BitSet {
    fn ensure(&mut self, bits: usize) {
        let need = bits.div_ceil(64);
        if self.words.len() < need {
            self.words.resize(need, 0);
        }
    }
    fn clear(&mut self) {
        for w in &mut self.words {
            *w = 0;
        }
    }
    fn set(&mut self, i: usize) {
        self.words[i / 64] |= 1u64 << (i % 64);
    }
    fn get(&self, i: usize) -> bool {
        self.words
            .get(i / 64)
            .is_some_and(|w| w & (1u64 << (i % 64)) != 0)
    }
}

/// Per-client visibility column + its viewer and the last-computed spawn/despawn delta.
struct ClientSlot {
    client: ClientId,
    viewer: Entity,
    view_radius: f32,
    current: BitSet,
    previous: BitSet,
    spawns: Vec<ReplId>,
    despawns: Vec<ReplId>,
}

impl ClientSlot {
    fn new(client: ClientId, viewer: Entity, view_radius: f32) -> Self {
        Self {
            client,
            viewer,
            view_radius,
            current: BitSet::default(),
            previous: BitSet::default(),
            spawns: Vec::new(),
            despawns: Vec::new(),
        }
    }
}

/// Configuration for the visibility resource.
#[derive(Debug, Clone, Copy)]
pub struct VisibilityConfig {
    /// Global grid cell size. Default `0.5 × view_radius` — the validated cost/overdraw
    /// optimum. (The gather block is sized to the exit radius, so with hysteresis the
    /// default is a 7×7 block; see the module docs.)
    pub cell_size: f32,
    /// `exit_radius = view_radius × exit_factor`. Default `1.25` (25% hysteresis).
    pub exit_factor: f32,
}

impl VisibilityConfig {
    /// The validated defaults derived from a representative `view_radius`:
    /// `cell_size = 0.5 × view_radius`, `exit_factor = 1.25`.
    pub fn from_view_radius(view_radius: f32) -> Self {
        Self {
            cell_size: 0.5 * view_radius,
            exit_factor: 1.25,
        }
    }
}

/// The server-side interest-management resource: the spatial grid plus the per-client
/// visibility matrix (current/previous bitsets, `ReplId`-indexed).
pub struct ReplicationVisibility {
    cell_size: f32,
    exit_factor: f32,
    grid: HashMap<Cell, Vec<ReplId>>,
    cell_cache: Vec<Cell>,
    pos: Vec<(f32, f32)>,
    clients: Vec<ClientSlot>,
    scratch_always: Vec<ReplId>,
}

impl Resource for ReplicationVisibility {}

impl ReplicationVisibility {
    /// A new, empty resource with the given configuration.
    pub fn new(config: VisibilityConfig) -> Self {
        Self {
            cell_size: config.cell_size,
            exit_factor: config.exit_factor,
            grid: HashMap::new(),
            cell_cache: Vec::new(),
            pos: Vec::new(),
            clients: Vec::new(),
            scratch_always: Vec::new(),
        }
    }

    /// Register a client with its `viewer` entity (the view center is the viewer's
    /// `Transform3D` translation, projected to the X/Z ground plane) and `view_radius`.
    /// Re-registering a client updates its viewer/radius.
    pub fn add_client(&mut self, client: ClientId, viewer: Entity, view_radius: f32) {
        if let Some(slot) = self.clients.iter_mut().find(|s| s.client == client) {
            slot.viewer = viewer;
            slot.view_radius = view_radius;
        } else {
            self.clients
                .push(ClientSlot::new(client, viewer, view_radius));
        }
    }

    /// Drop a client's visibility column (no archetype impact — it is resource state).
    pub fn remove_client(&mut self, client: ClientId) {
        self.clients.retain(|s| s.client != client);
    }

    /// Remove an entity's id from the spatial grid (called by the driver when the
    /// entity leaves replication). Gather is robust to a missing call — a despawned id
    /// is also skipped via the live check — but this reclaims the grid slot.
    pub fn remove_entity(&mut self, id: ReplId) {
        let idx = id.index();
        if idx < self.cell_cache.len() && self.cell_cache[idx] != UNPLACED {
            let cell = self.cell_cache[idx];
            if let Some(v) = self.grid.get_mut(&cell) {
                v.retain(|&x| x != id);
            }
            self.cell_cache[idx] = UNPLACED;
        }
    }

    fn ensure_capacity(&mut self, cap: usize) {
        if self.cell_cache.len() < cap {
            self.cell_cache.resize(cap, UNPLACED);
            self.pos.resize(cap, (0.0, 0.0));
        }
        for slot in &mut self.clients {
            slot.current.ensure(cap);
            slot.previous.ensure(cap);
        }
    }

    fn place(&mut self, id: ReplId, x: f32, z: f32, move_existing: bool) {
        let idx = id.index();
        self.pos[idx] = (x, z);
        let cell = cell_of(x, z, self.cell_size);
        let old = self.cell_cache[idx];
        if old == UNPLACED {
            self.grid.entry(cell).or_default().push(id);
            self.cell_cache[idx] = cell;
        } else if move_existing && old != cell {
            if let Some(v) = self.grid.get_mut(&old) {
                v.retain(|&x| x != id);
            }
            self.grid.entry(cell).or_default().push(id);
            self.cell_cache[idx] = cell;
        }
    }

    /// Advance one tick: sync the grid from `world`, then recompute every client's
    /// visibility and its spawn/despawn delta (readable via [`spawns`]/[`despawns`]).
    ///
    /// [`spawns`]: ReplicationVisibility::spawns
    /// [`despawns`]: ReplicationVisibility::despawns
    pub fn update(&mut self, world: &World, ids: &ReplIdMap) {
        self.ensure_capacity(ids.capacity());
        self.sync_grid(world, ids);
        self.gather(world, ids);
        self.diff();
    }

    fn sync_grid(&mut self, world: &World, ids: &ReplIdMap) {
        // Dynamic spatial entities: re-synced every tick (moved on cell change).
        let dynamic = world
            .query::<(Entity, &Transform3D)>()
            .with::<Replicated>()
            .without::<AlwaysRelevant>()
            .without::<OwnerOnly>()
            .without::<StaticRelevant>();
        for (e, t) in dynamic.iter() {
            if let Some(id) = ids.get(e) {
                self.place(id, t.translation.x, t.translation.z, true);
            }
        }
        // Static spatial entities: placed once, then never re-tested.
        let statics = world
            .query::<(Entity, &Transform3D)>()
            .with::<Replicated>()
            .with::<StaticRelevant>();
        for (e, t) in statics.iter() {
            if let Some(id) = ids.get(e) {
                self.place(id, t.translation.x, t.translation.z, false);
            }
        }
    }

    fn gather(&mut self, world: &World, ids: &ReplIdMap) {
        // Collect always-relevant ids into the reusable scratch (taken out to split the
        // borrow against `&mut self.clients`).
        let mut always = std::mem::take(&mut self.scratch_always);
        always.clear();
        let arq = world
            .query::<Entity>()
            .with::<Replicated>()
            .with::<AlwaysRelevant>();
        for e in arq.iter() {
            if let Some(id) = ids.get(e) {
                always.push(id);
            }
        }

        for slot in &mut self.clients {
            slot.current.clear();
            for &id in &always {
                slot.current.set(id.index());
            }
        }
        self.scratch_always = always;

        // Owner-only: set the bit only on the owning client's column.
        let oq = world.query::<(Entity, &OwnerOnly)>().with::<Replicated>();
        for (e, owner) in oq.iter() {
            if let Some(id) = ids.get(e) {
                if let Some(slot) = self.clients.iter_mut().find(|s| s.client == owner.0) {
                    slot.current.set(id.index());
                }
            }
        }

        // Spatial: gather the cell block around each viewer, with hysteresis.
        for slot in &mut self.clients {
            let Some(vt) = world.get::<Transform3D>(slot.viewer) else {
                continue; // viewer gone this tick: keep always/owner bits only
            };
            let (cx, cz) = (vt.translation.x, vt.translation.z);
            let enter2 = slot.view_radius * slot.view_radius;
            let exit_r = slot.view_radius * self.exit_factor;
            let exit2 = exit_r * exit_r;
            // The block must cover the EXIT radius, not just the enter radius: a
            // previously-visible entity stays visible out to `exit_r` (hysteresis), so it
            // must still be gathered there — otherwise it falls outside the block and the
            // diff spuriously despawns it, reintroducing the boundary thrash hysteresis
            // exists to prevent. At the default ratio this is a 7×7 block (ceil(20/8)=3).
            let r_cells = (exit_r / self.cell_size).ceil() as i32;
            let base = cell_of(cx, cz, self.cell_size);
            for gx in (base.0 - r_cells)..=(base.0 + r_cells) {
                for gz in (base.1 - r_cells)..=(base.1 + r_cells) {
                    let Some(cell_ids) = self.grid.get(&(gx, gz)) else {
                        continue;
                    };
                    for &id in cell_ids {
                        if ids.entity(id).is_none() {
                            continue; // despawned id lingering in the grid: skip
                        }
                        let (px, pz) = self.pos[id.index()];
                        let d2 = (px - cx) * (px - cx) + (pz - cz) * (pz - cz);
                        let threshold = if slot.previous.get(id.index()) {
                            exit2
                        } else {
                            enter2
                        };
                        if d2 <= threshold {
                            slot.current.set(id.index());
                        }
                    }
                }
            }
        }
    }

    fn diff(&mut self) {
        for slot in &mut self.clients {
            slot.spawns.clear();
            slot.despawns.clear();
            let words = slot.current.words.len().max(slot.previous.words.len());
            for w in 0..words {
                let cur = slot.current.words.get(w).copied().unwrap_or(0);
                let prev = slot.previous.words.get(w).copied().unwrap_or(0);
                let mut added = cur & !prev;
                while added != 0 {
                    let b = added.trailing_zeros() as usize;
                    added &= added - 1;
                    slot.spawns.push(ReplId((w * 64 + b) as u32));
                }
                let mut removed = prev & !cur;
                while removed != 0 {
                    let b = removed.trailing_zeros() as usize;
                    removed &= removed - 1;
                    slot.despawns.push(ReplId((w * 64 + b) as u32));
                }
            }
            std::mem::swap(&mut slot.current, &mut slot.previous);
        }
    }

    fn slot(&self, client: ClientId) -> Option<&ClientSlot> {
        self.clients.iter().find(|s| s.client == client)
    }

    /// Ids that became visible to `client` on the last [`update`](Self::update).
    pub fn spawns(&self, client: ClientId) -> &[ReplId] {
        self.slot(client).map_or(&[], |s| &s.spawns)
    }

    /// Ids that stopped being visible to `client` on the last [`update`](Self::update).
    pub fn despawns(&self, client: ClientId) -> &[ReplId] {
        self.slot(client).map_or(&[], |s| &s.despawns)
    }

    /// Whether `id` is currently visible to `client` (after the last update). `previous`
    /// holds the just-computed visibility (the diff swaps current→previous).
    pub fn is_visible(&self, client: ClientId, id: ReplId) -> bool {
        self.slot(client)
            .is_some_and(|s| s.previous.get(id.index()))
    }

    /// Collect the ids currently visible to `client` into `out` (cleared first). Used by
    /// the server driver to derive the per-client update set (visible minus spawns).
    pub fn visible_into(&self, client: ClientId, out: &mut Vec<ReplId>) {
        out.clear();
        let Some(slot) = self.slot(client) else {
            return;
        };
        for (w, &word) in slot.previous.words.iter().enumerate() {
            let mut bits = word;
            while bits != 0 {
                let b = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                out.push(ReplId((w * 64 + b) as u32));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_core::Vec3;

    fn world() -> World {
        let mut w = World::new();
        w.register::<Transform3D>();
        w.register::<Replicated>();
        w.register::<AlwaysRelevant>();
        w.register::<OwnerOnly>();
        w.register::<StaticRelevant>();
        w
    }

    fn at(x: f32, z: f32) -> Transform3D {
        Transform3D::from_translation(Vec3::new(x, 0.0, z))
    }

    fn vis() -> ReplicationVisibility {
        ReplicationVisibility::new(VisibilityConfig::from_view_radius(16.0))
    }

    #[test]
    fn enter_and_exit_emit_exactly_one_event() {
        let mut w = world();
        let mut ids = ReplIdMap::new();
        let viewer = w.spawn_with((at(0.0, 0.0), Replicated, OwnerOnly(ClientId(1))));
        ids.allocate(viewer);
        let e = w.spawn_with((at(100.0, 0.0), Replicated)); // far away
        let id = ids.allocate(e);

        let mut v = vis();
        v.add_client(ClientId(1), viewer, 16.0);

        v.update(&w, &ids);
        assert!(v.spawns(ClientId(1)).iter().all(|&s| s != id));
        assert!(!v.is_visible(ClientId(1), id));

        // Move into view.
        *w.get_mut::<Transform3D>(e).unwrap() = at(5.0, 0.0);
        v.update(&w, &ids);
        assert_eq!(v.spawns(ClientId(1)), &[id], "exactly one spawn on enter");
        assert!(v.despawns(ClientId(1)).is_empty());
        assert!(v.is_visible(ClientId(1), id));

        // Stay in view: no event.
        v.update(&w, &ids);
        assert!(v.spawns(ClientId(1)).is_empty());
        assert!(v.despawns(ClientId(1)).is_empty());

        // Leave view.
        *w.get_mut::<Transform3D>(e).unwrap() = at(100.0, 0.0);
        v.update(&w, &ids);
        assert!(v.spawns(ClientId(1)).is_empty());
        assert_eq!(
            v.despawns(ClientId(1)),
            &[id],
            "exactly one despawn on exit"
        );
    }

    #[test]
    fn always_relevant_reaches_every_client_owner_only_reaches_owner() {
        let mut w = world();
        let mut ids = ReplIdMap::new();
        // Two viewers far apart so neither sees the others spatially.
        let v1 = w.spawn_with((at(0.0, 0.0), Replicated, OwnerOnly(ClientId(1))));
        let v2 = w.spawn_with((at(1000.0, 0.0), Replicated, OwnerOnly(ClientId(2))));
        ids.allocate(v1);
        ids.allocate(v2);
        let global = w.spawn_with((at(500.0, 500.0), Replicated, AlwaysRelevant));
        let gid = ids.allocate(global);
        let owned2 = w.spawn_with((at(2000.0, 0.0), Replicated, OwnerOnly(ClientId(2))));
        let oid = ids.allocate(owned2);

        let mut v = vis();
        v.add_client(ClientId(1), v1, 16.0);
        v.add_client(ClientId(2), v2, 16.0);
        v.update(&w, &ids);

        assert!(
            v.is_visible(ClientId(1), gid),
            "always-relevant reaches client 1"
        );
        assert!(
            v.is_visible(ClientId(2), gid),
            "always-relevant reaches client 2"
        );
        assert!(
            !v.is_visible(ClientId(1), oid),
            "owner-only of client 2 is invisible to client 1"
        );
        assert!(
            v.is_visible(ClientId(2), oid),
            "owner-only reaches its owner"
        );
    }

    #[test]
    fn static_relevant_is_placed_and_visible_in_range() {
        let mut w = world();
        let mut ids = ReplIdMap::new();
        let viewer = w.spawn_with((at(0.0, 0.0), Replicated, OwnerOnly(ClientId(1))));
        ids.allocate(viewer);
        let wall = w.spawn_with((at(8.0, 0.0), Replicated, StaticRelevant));
        let wid = ids.allocate(wall);

        let mut v = vis();
        v.add_client(ClientId(1), viewer, 16.0);
        v.update(&w, &ids);
        assert!(
            v.is_visible(ClientId(1), wid),
            "static entity in range is visible"
        );
        // A second update produces no spawn churn (placed once, no movement).
        v.update(&w, &ids);
        assert!(v.spawns(ClientId(1)).is_empty());
        assert!(v.despawns(ClientId(1)).is_empty());
    }

    #[test]
    fn hysteresis_collapses_boundary_thrash() {
        // An entity oscillating across the enter boundary (16) should fire once with
        // the 25% exit margin (exit = 20), not every tick.
        let mut w = world();
        let mut ids = ReplIdMap::new();
        let viewer = w.spawn_with((at(0.0, 0.0), Replicated, OwnerOnly(ClientId(1))));
        ids.allocate(viewer);
        let e = w.spawn_with((at(15.0, 0.0), Replicated));
        let id = ids.allocate(e);

        let mut v = vis();
        v.add_client(ClientId(1), viewer, 16.0);

        let mut events = 0;
        for tick in 0..40 {
            // Oscillate between 15 (inside enter) and 17 (outside enter, inside exit=20).
            let x = if tick % 2 == 0 { 15.0 } else { 17.0 };
            *w.get_mut::<Transform3D>(e).unwrap() = at(x, 0.0);
            v.update(&w, &ids);
            // Count only events for the oscillating entity (the viewer also spawns once).
            events += v.spawns(ClientId(1)).iter().filter(|&&s| s == id).count()
                + v.despawns(ClientId(1)).iter().filter(|&&s| s == id).count();
        }
        assert!(
            events <= 1,
            "hysteresis must collapse thrash, got {events} events"
        );
    }

    #[test]
    fn hysteresis_band_entity_off_axis_is_not_spuriously_despawned() {
        // Regression: the gather block must cover the EXIT radius, not just the enter
        // radius. An entity that enters, then drifts into the hysteresis band on the
        // negative axis (where the viewer sits at the low edge of its base cell) must
        // stay visible — not fall outside the block and get despawned.
        let mut w = world();
        let mut ids = ReplIdMap::new();
        let viewer = w.spawn_with((at(0.0, 0.0), Replicated, OwnerOnly(ClientId(1))));
        ids.allocate(viewer);
        let e = w.spawn_with((at(-10.0, 0.0), Replicated)); // inside enter (10 < 16)
        let id = ids.allocate(e);

        let mut v = vis();
        v.add_client(ClientId(1), viewer, 16.0);
        v.update(&w, &ids);
        assert!(v.is_visible(ClientId(1), id), "entity enters at -10");

        // Into the hysteresis band on the negative axis: 16 < 18 < exit 20.
        *w.get_mut::<Transform3D>(e).unwrap() = at(-18.0, 0.0);
        v.update(&w, &ids);
        assert!(
            v.is_visible(ClientId(1), id),
            "still visible in the hysteresis band (block covers the exit radius)"
        );
        assert!(
            v.despawns(ClientId(1)).iter().all(|&s| s != id),
            "no spurious despawn for a hysteresis-band entity"
        );
    }

    #[test]
    fn five_by_five_block_covers_the_view_radius() {
        // cell_size = 0.5*view_radius => r_cells = 2 => 5x5 block. An entity at the
        // far edge of the view radius along a diagonal must still be gathered.
        let mut w = world();
        let mut ids = ReplIdMap::new();
        let viewer = w.spawn_with((at(0.0, 0.0), Replicated, OwnerOnly(ClientId(1))));
        ids.allocate(viewer);
        // Just inside the radius (16) on the diagonal: (11.0, 11.0) -> dist ~15.56.
        let e = w.spawn_with((at(11.0, 11.0), Replicated));
        let id = ids.allocate(e);

        let mut v = vis();
        v.add_client(ClientId(1), viewer, 16.0);
        v.update(&w, &ids);
        assert!(
            v.is_visible(ClientId(1), id),
            "an in-range diagonal entity is covered by the 5x5 block"
        );
    }

    #[test]
    fn remove_entity_prunes_the_grid() {
        let mut w = world();
        let mut ids = ReplIdMap::new();
        let viewer = w.spawn_with((at(0.0, 0.0), Replicated, OwnerOnly(ClientId(1))));
        ids.allocate(viewer);
        let e = w.spawn_with((at(5.0, 0.0), Replicated));
        let id = ids.allocate(e);

        let mut v = vis();
        v.add_client(ClientId(1), viewer, 16.0);
        v.update(&w, &ids);
        assert!(v.is_visible(ClientId(1), id));

        // Despawn: remove from the world and from replication, then update.
        w.despawn(e).unwrap();
        ids.release(e);
        v.remove_entity(id);
        v.update(&w, &ids);
        assert_eq!(v.despawns(ClientId(1)), &[id], "removed entity despawns");
        assert!(!v.is_visible(ClientId(1), id));
    }
}
