//! Archetype storage: each archetype is a unique sorted set of [`ComponentId`]
//! holding one SoA column per component plus a parallel `Vec<Entity>`.
//!
//! Rows move between archetypes on `insert`/`remove` via swap-remove + push of
//! boxed values; layout and columns are private internals.

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
    /// (each `Some` if present, `None` if its id is `None`/absent), borrowed
    /// disjointly. Entities come from a separate field, so a mutable tuple query
    /// can read entity ids while holding mutable column borrows — all without
    /// `unsafe`.
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

/// Holds every archetype and the entity-location map.
pub(crate) struct ArchetypeStore {
    archetypes: Vec<Archetype>,
    by_set: HashMap<Vec<ComponentId>, ArchetypeId>,
    /// `entity index -> (archetype, row)` for live entities.
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

    pub(crate) fn archetypes_mut(&mut self) -> &mut [Archetype] {
        &mut self.archetypes
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
    /// its columns via `factory`) if unseen.
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
    /// in column order and fixing up the swapped-in row's location. Returns
    /// `None` if the entity had no location.
    fn extract_row(&mut self, entity: Entity) -> Option<Vec<(ComponentId, AnyValue)>> {
        let (aid, row) = self.location(entity)?;
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
        Some(pairs)
    }

    /// Despawns an entity's storage row. Returns `false` if it had no location.
    pub(crate) fn remove_entity(&mut self, entity: Entity) -> bool {
        self.extract_row(entity).is_some()
    }

    /// Moves `entity` into the archetype whose set is its current set plus the
    /// keys of `overrides`, carrying every surviving value forward. Values in
    /// `overrides` replace existing ones; new component ids extend the set.
    /// `factory` builds fresh columns for any newly created archetype. The
    /// entity must currently be placed.
    pub(crate) fn insert_components<F>(
        &mut self,
        entity: Entity,
        overrides: Vec<(ComponentId, AnyValue)>,
        factory: F,
    ) where
        F: FnMut(ComponentId) -> Box<dyn ComponentColumn>,
    {
        let mut carried: HashMap<ComponentId, AnyValue> = HashMap::new();
        if let Some(pairs) = self.extract_row(entity) {
            for (cid, val) in pairs {
                carried.insert(cid, val);
            }
        }
        for (cid, val) in overrides {
            carried.insert(cid, val);
        }
        let mut set: Vec<ComponentId> = carried.keys().copied().collect();
        set.sort_unstable();
        self.write_row(entity, set, carried, factory);
    }

    /// Moves `entity` to the archetype with `id` removed, dropping that value.
    /// Returns the removed boxed value, or `None` if absent. The entity must be
    /// placed.
    pub(crate) fn remove_component<F>(
        &mut self,
        entity: Entity,
        id: ComponentId,
        factory: F,
    ) -> Option<AnyValue>
    where
        F: FnMut(ComponentId) -> Box<dyn ComponentColumn>,
    {
        let pairs = self.extract_row(entity)?;
        let mut carried: HashMap<ComponentId, AnyValue> = HashMap::new();
        let mut removed = None;
        for (cid, val) in pairs {
            if cid == id {
                removed = Some(val);
            } else {
                carried.insert(cid, val);
            }
        }
        let mut set: Vec<ComponentId> = carried.keys().copied().collect();
        set.sort_unstable();
        self.write_row(entity, set, carried, factory);
        removed
    }

    fn write_row<F>(
        &mut self,
        entity: Entity,
        set: Vec<ComponentId>,
        mut values: HashMap<ComponentId, AnyValue>,
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
    }
}
