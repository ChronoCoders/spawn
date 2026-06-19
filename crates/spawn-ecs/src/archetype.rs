//! Archetype storage: each archetype is a unique sorted set of [`ComponentId`]
//! holding one SoA column per component plus a parallel `Vec<Entity>`.
//!
//! Rows move between archetypes on `insert`/`remove` via swap-remove + push of
//! boxed values; layout and columns are private internals. Per-row change-detection
//! ticks ([`ArchetypeTicks`]) are stored in the [`ArchetypeStore`] parallel to the
//! archetypes — not inside the type-erased column — so a filtered `iter_mut` can
//! read ticks (shared) while mutating columns (exclusive) without `unsafe`.

use crate::change::Tick;
use crate::component::{AnyValue, ComponentColumn, ComponentId};
use crate::entity::Entity;
use std::collections::HashMap;

/// Opaque archetype identifier, exposed only for debug/metrics. Dense, assigned
/// in creation order; archetype `0` is always the empty archetype.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArchetypeId(u32);

impl ArchetypeId {
    pub(crate) const fn new(raw: u32) -> Self {
        Self(raw)
    }

    pub(crate) const fn index(self) -> usize {
        self.0 as usize
    }
}

/// A single archetype: sorted component set + per-component columns + entities.
///
/// `#[doc(hidden)] pub` only so the sealed [`QueryData`](crate::query::QueryData)
/// trait can name it in its (hidden) method signatures; not stable surface.
#[doc(hidden)]
pub struct Archetype {
    components: Vec<ComponentId>,
    columns: Vec<Box<dyn ComponentColumn>>,
    entities: Vec<Entity>,
}

impl Archetype {
    pub(crate) fn entities(&self) -> &[Entity] {
        &self.entities
    }

    pub(crate) fn len(&self) -> usize {
        self.entities.len()
    }

    pub(crate) fn contains(&self, id: ComponentId) -> bool {
        self.components.binary_search(&id).is_ok()
    }

    fn column_index(&self, id: ComponentId) -> Option<usize> {
        self.components.binary_search(&id).ok()
    }

    pub(crate) fn column(&self, id: ComponentId) -> Option<&dyn ComponentColumn> {
        self.column_index(id).map(|i| self.columns[i].as_ref())
    }

    pub(crate) fn column_mut(&mut self, id: ComponentId) -> Option<&mut dyn ComponentColumn> {
        match self.column_index(id) {
            Some(i) => Some(self.columns[i].as_mut()),
            None => None,
        }
    }

    /// Returns the entities slice together with the `wanted` component columns
    /// (each `Some` if present), borrowed disjointly. Entities come from a separate
    /// field, so a mutable tuple query can read entity ids while holding mutable
    /// column borrows — all without `unsafe`.
    pub(crate) fn entities_and_columns_mut<const N: usize>(
        &mut self,
        wanted: [Option<ComponentId>; N],
    ) -> (&[Entity], [Option<&mut dyn ComponentColumn>; N]) {
        let target_idx: [Option<usize>; N] =
            std::array::from_fn(|i| wanted[i].and_then(|id| col_index(&self.components, id)));
        let cols = peel_columns(&mut self.columns, target_idx);
        (&self.entities, cols)
    }
}

fn col_index(components: &[ComponentId], id: ComponentId) -> Option<usize> {
    components.binary_search(&id).ok()
}

type ExtractedRow = (Vec<(ComponentId, AnyValue)>, Vec<(ComponentId, Tick)>);

/// Peels the requested columns out of `columns` as disjoint mutable borrows by
/// walking front-to-back in ascending index order; no `unsafe`, no allocation.
fn peel_columns<'a, const N: usize>(
    columns: &'a mut [Box<dyn ComponentColumn>],
    target_idx: [Option<usize>; N],
) -> [Option<&'a mut dyn ComponentColumn>; N] {
    let mut order: [usize; N] = std::array::from_fn(|i| i);
    order.sort_unstable_by_key(|&slot| target_idx[slot].unwrap_or(usize::MAX));
    let mut remaining: &mut [Box<dyn ComponentColumn>] = columns;
    let mut base = 0usize;
    let mut out: [Option<&'a mut dyn ComponentColumn>; N] = std::array::from_fn(|_| None);
    for &slot in &order {
        let col_idx = match target_idx[slot] {
            Some(idx) => idx,
            None => continue,
        };
        let offset = col_idx - base;
        let taken = std::mem::take(&mut remaining);
        let (_, rest) = taken.split_at_mut(offset);
        if let Some((head, tail)) = rest.split_first_mut() {
            out[slot] = Some(head.as_mut());
            remaining = tail;
            base = col_idx + 1;
        }
    }
    out
}

/// Per-archetype change-detection ticks, held parallel to the archetype's columns
/// (one `added`/`changed` tick per row per column).
///
/// `#[doc(hidden)] pub` only so the sealed [`QueryFilter`](crate::query::filter::QueryFilter)
/// trait can name it; not stable surface.
#[doc(hidden)]
pub struct ArchetypeTicks {
    components: Vec<ComponentId>,
    added: Vec<Vec<Tick>>,
    changed: Vec<Vec<Tick>>,
}

impl ArchetypeTicks {
    fn empty() -> Self {
        Self {
            components: Vec::new(),
            added: Vec::new(),
            changed: Vec::new(),
        }
    }

    fn for_set(components: &[ComponentId]) -> Self {
        let n = components.len();
        Self {
            components: components.to_vec(),
            added: vec![Vec::new(); n],
            changed: vec![Vec::new(); n],
        }
    }

    fn col(&self, id: ComponentId) -> Option<usize> {
        self.components.binary_search(&id).ok()
    }

    /// The tick `T` was added on `entity`'s row, or `None` if `id` is absent.
    pub(crate) fn added_at(&self, id: ComponentId, row: usize) -> Option<Tick> {
        self.col(id).and_then(|c| self.added[c].get(row).copied())
    }

    /// The tick `T` was last changed on `entity`'s row, or `None` if `id` is absent.
    pub(crate) fn changed_at(&self, id: ComponentId, row: usize) -> Option<Tick> {
        self.col(id).and_then(|c| self.changed[c].get(row).copied())
    }
}

/// Holds every archetype, its parallel change ticks, and the entity-location map.
pub(crate) struct ArchetypeStore {
    archetypes: Vec<Archetype>,
    ticks: Vec<ArchetypeTicks>,
    by_set: HashMap<Vec<ComponentId>, ArchetypeId>,
    locations: Vec<Option<(ArchetypeId, usize)>>,
}

impl ArchetypeStore {
    pub(crate) fn new() -> Self {
        let empty = Archetype {
            components: Vec::new(),
            columns: Vec::new(),
            entities: Vec::new(),
        };
        let mut by_set = HashMap::new();
        by_set.insert(Vec::new(), ArchetypeId::new(0));
        Self {
            archetypes: vec![empty],
            ticks: vec![ArchetypeTicks::empty()],
            by_set,
            locations: Vec::new(),
        }
    }

    pub(crate) fn count(&self) -> usize {
        self.archetypes.len()
    }

    pub(crate) fn empty_archetype(&self) -> ArchetypeId {
        ArchetypeId::new(0)
    }

    pub(crate) fn archetype(&self, id: ArchetypeId) -> &Archetype {
        &self.archetypes[id.index()]
    }

    pub(crate) fn archetype_mut(&mut self, id: ArchetypeId) -> &mut Archetype {
        &mut self.archetypes[id.index()]
    }

    pub(crate) fn archetypes(&self) -> &[Archetype] {
        &self.archetypes
    }

    pub(crate) fn ticks(&self) -> &[ArchetypeTicks] {
        &self.ticks
    }

    pub(crate) fn tick(&self, id: ArchetypeId) -> &ArchetypeTicks {
        &self.ticks[id.index()]
    }

    /// Disjoint split borrow of the archetypes (mutable, for column access) and
    /// their ticks (shared, for change filtering) — the seam that lets a filtered
    /// `iter_mut` evaluate ticks while mutating columns without `unsafe`.
    pub(crate) fn archetypes_and_ticks_mut(&mut self) -> (&mut [Archetype], &[ArchetypeTicks]) {
        (&mut self.archetypes, &self.ticks)
    }

    /// Stamps the changed tick of `component` on a single row.
    pub(crate) fn mark_changed(
        &mut self,
        id: ArchetypeId,
        component: ComponentId,
        row: usize,
        tick: Tick,
    ) {
        let t = &mut self.ticks[id.index()];
        if let Some(col) = t.col(component) {
            if let Some(slot) = t.changed[col].get_mut(row) {
                *slot = tick;
            }
        }
    }

    /// Stamps the changed tick of `component` on every row of the archetype —
    /// the conservative mutable-access stamp applied when an exclusive iterator
    /// over that column is created.
    pub(crate) fn stamp_changed_all(
        &mut self,
        id: ArchetypeId,
        component: ComponentId,
        tick: Tick,
    ) {
        let t = &mut self.ticks[id.index()];
        if let Some(col) = t.col(component) {
            for slot in t.changed[col].iter_mut() {
                *slot = tick;
            }
        }
    }

    pub(crate) fn location(&self, entity: Entity) -> Option<(ArchetypeId, usize)> {
        self.locations
            .get(entity.index() as usize)
            .copied()
            .flatten()
    }

    fn set_location(&mut self, entity: Entity, loc: Option<(ArchetypeId, usize)>) {
        let idx = entity.index() as usize;
        if self.locations.len() <= idx {
            self.locations.resize(idx + 1, None);
        }
        self.locations[idx] = loc;
    }

    /// Returns the archetype id for the sorted `components` set, creating it (and
    /// its columns + tick matrix) if unseen.
    fn get_or_create<F>(&mut self, components: Vec<ComponentId>, mut factory: F) -> ArchetypeId
    where
        F: FnMut(ComponentId) -> Box<dyn ComponentColumn>,
    {
        if let Some(id) = self.by_set.get(&components) {
            return *id;
        }
        let id = ArchetypeId::new(self.archetypes.len() as u32);
        let columns: Vec<Box<dyn ComponentColumn>> =
            components.iter().map(|&cid| factory(cid)).collect();
        self.ticks.push(ArchetypeTicks::for_set(&components));
        self.by_set.insert(components.clone(), id);
        self.archetypes.push(Archetype {
            components,
            columns,
            entities: Vec::new(),
        });
        id
    }

    /// Places a brand-new entity (no components) into the empty archetype.
    pub(crate) fn place_empty(&mut self, entity: Entity) {
        let id = self.empty_archetype();
        let row = self.archetypes[id.index()].entities.len();
        self.archetypes[id.index()].entities.push(entity);
        self.set_location(entity, Some((id, row)));
    }

    /// Removes an entity's row, returning its `(component id, boxed value)` pairs
    /// and its per-component added ticks, fixing up the swapped-in row's location
    /// and ticks. Returns `None` if the entity had no location.
    fn extract_row(&mut self, entity: Entity) -> Option<ExtractedRow> {
        let (aid, row) = self.location(entity)?;
        let added_ticks: Vec<(ComponentId, Tick)> = {
            let t = &self.ticks[aid.index()];
            t.components
                .iter()
                .enumerate()
                .map(|(col, &cid)| (cid, t.added[col][row]))
                .collect()
        };
        let arch = &mut self.archetypes[aid.index()];
        let mut pairs = Vec::with_capacity(arch.columns.len());
        for (col, &cid) in arch.columns.iter_mut().zip(arch.components.iter()) {
            pairs.push((cid, col.swap_remove(row)));
        }
        arch.entities.swap_remove(row);
        let moved = arch.entities.get(row).copied();
        if let Some(moved) = moved {
            self.set_location(moved, Some((aid, row)));
        }
        self.set_location(entity, None);
        let t = &mut self.ticks[aid.index()];
        for col in 0..t.components.len() {
            t.added[col].swap_remove(row);
            t.changed[col].swap_remove(row);
        }
        Some((pairs, added_ticks))
    }

    /// Despawns an entity's storage row. Returns `false` if it had no location.
    pub(crate) fn remove_entity(&mut self, entity: Entity) -> bool {
        self.extract_row(entity).is_some()
    }

    /// Moves `entity` into the archetype whose set is its current set plus the
    /// keys of `overrides`, carrying every surviving value (and its added tick)
    /// forward. New/overwritten components stamp `changed = current_tick`; newly
    /// inserted components also stamp `added = current_tick`.
    pub(crate) fn insert_components<F>(
        &mut self,
        entity: Entity,
        overrides: Vec<(ComponentId, AnyValue)>,
        current_tick: Tick,
        factory: F,
    ) where
        F: FnMut(ComponentId) -> Box<dyn ComponentColumn>,
    {
        let mut carried: HashMap<ComponentId, AnyValue> = HashMap::new();
        let mut carried_added: HashMap<ComponentId, Tick> = HashMap::new();
        if let Some((pairs, added)) = self.extract_row(entity) {
            for (cid, val) in pairs {
                carried.insert(cid, val);
            }
            for (cid, tick) in added {
                carried_added.insert(cid, tick);
            }
        }
        for (cid, val) in overrides {
            carried.insert(cid, val);
        }
        let mut set: Vec<ComponentId> = carried.keys().copied().collect();
        set.sort_unstable();
        self.write_row(entity, set, carried, carried_added, current_tick, factory);
    }

    /// Moves `entity` to the archetype with `id` removed, dropping that value.
    /// Returns the removed boxed value, or `None` if absent.
    pub(crate) fn remove_component<F>(
        &mut self,
        entity: Entity,
        id: ComponentId,
        current_tick: Tick,
        factory: F,
    ) -> Option<AnyValue>
    where
        F: FnMut(ComponentId) -> Box<dyn ComponentColumn>,
    {
        let (pairs, added) = self.extract_row(entity)?;
        let mut carried: HashMap<ComponentId, AnyValue> = HashMap::new();
        let mut carried_added: HashMap<ComponentId, Tick> = HashMap::new();
        let mut removed = None;
        for (cid, val) in pairs {
            if cid == id {
                removed = Some(val);
            } else {
                carried.insert(cid, val);
            }
        }
        for (cid, tick) in added {
            if cid != id {
                carried_added.insert(cid, tick);
            }
        }
        let mut set: Vec<ComponentId> = carried.keys().copied().collect();
        set.sort_unstable();
        self.write_row(entity, set, carried, carried_added, current_tick, factory);
        removed
    }

    fn write_row<F>(
        &mut self,
        entity: Entity,
        set: Vec<ComponentId>,
        mut values: HashMap<ComponentId, AnyValue>,
        carried_added: HashMap<ComponentId, Tick>,
        current_tick: Tick,
        factory: F,
    ) where
        F: FnMut(ComponentId) -> Box<dyn ComponentColumn>,
    {
        let target = self.get_or_create(set.clone(), factory);
        let arch = &mut self.archetypes[target.index()];
        let row = arch.entities.len();
        arch.entities.push(entity);
        for (col_idx, cid) in set.iter().enumerate() {
            if let Some(val) = values.remove(cid) {
                arch.columns[col_idx].push_boxed(val);
            }
        }
        self.set_location(entity, Some((target, row)));
        let t = &mut self.ticks[target.index()];
        for (col_idx, cid) in set.iter().enumerate() {
            let added = carried_added.get(cid).copied().unwrap_or(current_tick);
            t.added[col_idx].push(added);
            t.changed[col_idx].push(current_tick);
        }
    }
}
