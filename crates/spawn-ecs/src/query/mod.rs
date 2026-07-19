//! The query engine: [`Query`], [`QueryData`], and allocation-free iterators.
//!
//! A query zips the SoA columns of every matching archetype. Items borrow
//! directly into the columns through standard slice iterators: no per-item
//! allocation, no intermediate buffer, no `unsafe`. Iteration order is
//! deterministic: archetypes in [`ArchetypeId`](crate::archetype::ArchetypeId)
//! order, rows in column order.

pub mod filter;

use crate::archetype::{Archetype, ArchetypeId, ArchetypeStore, ArchetypeTicks};
use crate::change::Tick;
use crate::component::{column_slice, column_slice_mut, Component, ComponentId, ComponentRegistry};
use crate::entity::Entity;
use crate::query::filter::QueryFilter;
use std::any::TypeId;
use std::marker::PhantomData;

mod sealed {
    pub trait Sealed {}
}

/// What a query reads and writes, used both to match archetypes and to compute
/// system access sets. Sealed.
///
/// Implemented for `&T`, `&mut T`, `Entity`, and tuples thereof up to arity 12.
pub trait QueryData: sealed::Sealed {
    /// Item yielded by shared iteration / random `get`.
    type Item<'a>;
    /// Item yielded by exclusive iteration / random `get_mut`.
    type ItemMut<'a>;
    /// Per-archetype row iterator for shared access.
    type Iter<'a>: Iterator<Item = Self::Item<'a>> + ExactSizeIterator;
    /// Per-archetype row iterator for exclusive access.
    type IterMut<'a>: Iterator<Item = Self::ItemMut<'a>> + ExactSizeIterator;

    /// Reports each accessed component's `TypeId` and whether it is a write.
    fn access(visit: &mut dyn FnMut(TypeId, &'static str, bool));

    /// Returns `true` iff `archetype` holds every component this query needs.
    fn matches(archetype: &Archetype, registry: &ComponentRegistry) -> bool;

    /// Builds a shared row iterator over `archetype`, or `None` if a needed
    /// column is missing or mistyped.
    fn iter_archetype<'a>(
        archetype: &'a Archetype,
        registry: &ComponentRegistry,
    ) -> Option<Self::Iter<'a>>;

    /// Builds an exclusive row iterator over `archetype`.
    fn iter_archetype_mut<'a>(
        archetype: &'a mut Archetype,
        registry: &ComponentRegistry,
    ) -> Option<Self::IterMut<'a>>;

    /// Reads a single row by index for random access (`get`).
    fn get_row<'a>(
        archetype: &'a Archetype,
        registry: &ComponentRegistry,
        row: usize,
    ) -> Option<Self::Item<'a>>;

    /// Reads a single row by index for exclusive random access (`get_mut`).
    fn get_row_mut<'a>(
        archetype: &'a mut Archetype,
        registry: &ComponentRegistry,
        row: usize,
    ) -> Option<Self::ItemMut<'a>>;
}

/// What a [`QueryElement`] needs to bind against an archetype. `#[doc(hidden)]`:
/// an internal detail of the sealed query traits.
#[doc(hidden)]
pub enum ElementNeed {
    /// No column required (the `Entity` element); always available.
    Entity,
    /// A registered component column with this id is required.
    Column(ComponentId),
    /// The component type was never registered; the query matches nothing.
    Unregistered,
}

/// A single query element (`&T`, `&mut T`, or `Entity`) usable as a tuple
/// member. Composed by [`QueryData`] tuple impls; sealed. `#[doc(hidden)]`:
/// an internal building block of `QueryData`.
#[doc(hidden)]
pub trait QueryElement: sealed::Sealed {
    /// Item yielded for shared access.
    type Item<'a>;
    /// Item yielded for exclusive access.
    type ItemMut<'a>;
    /// Shared per-element iterator.
    type Iter<'a>: Iterator<Item = Self::Item<'a>> + ExactSizeIterator;
    /// Exclusive per-element iterator.
    type IterMut<'a>: Iterator<Item = Self::ItemMut<'a>> + ExactSizeIterator;

    /// Reports this element's access (`TypeId`, is-write).
    fn access(visit: &mut dyn FnMut(TypeId, &'static str, bool));

    /// What this element needs from an archetype.
    fn required(registry: &ComponentRegistry) -> ElementNeed;

    /// Builds the shared iterator from an optional column and the entities slice.
    fn make_iter<'a>(
        column: Option<&'a dyn crate::component::ComponentColumn>,
        entities: &'a [Entity],
    ) -> Option<Self::Iter<'a>>;

    /// Builds the exclusive iterator from an optional column and entities slice.
    fn make_iter_mut<'a>(
        column: Option<&'a mut dyn crate::component::ComponentColumn>,
        entities: &'a [Entity],
    ) -> Option<Self::IterMut<'a>>;
}

impl sealed::Sealed for Entity {}
impl QueryData for Entity {
    type Item<'a> = Entity;
    type ItemMut<'a> = Entity;
    type Iter<'a> = std::iter::Copied<std::slice::Iter<'a, Entity>>;
    type IterMut<'a> = std::iter::Copied<std::slice::Iter<'a, Entity>>;

    fn access(_visit: &mut dyn FnMut(TypeId, &'static str, bool)) {}

    fn matches(_archetype: &Archetype, _registry: &ComponentRegistry) -> bool {
        true
    }

    fn iter_archetype<'a>(
        archetype: &'a Archetype,
        _registry: &ComponentRegistry,
    ) -> Option<Self::Iter<'a>> {
        Some(archetype.entities().iter().copied())
    }

    fn iter_archetype_mut<'a>(
        archetype: &'a mut Archetype,
        _registry: &ComponentRegistry,
    ) -> Option<Self::IterMut<'a>> {
        Some(archetype.entities().iter().copied())
    }

    fn get_row<'a>(
        archetype: &'a Archetype,
        _registry: &ComponentRegistry,
        row: usize,
    ) -> Option<Self::Item<'a>> {
        archetype.entities().get(row).copied()
    }

    fn get_row_mut<'a>(
        archetype: &'a mut Archetype,
        _registry: &ComponentRegistry,
        row: usize,
    ) -> Option<Self::ItemMut<'a>> {
        archetype.entities().get(row).copied()
    }
}

impl<T: Component> sealed::Sealed for &T {}
impl<T: Component> QueryData for &T {
    type Item<'a> = &'a T;
    type ItemMut<'a> = &'a T;
    type Iter<'a> = std::slice::Iter<'a, T>;
    type IterMut<'a> = std::slice::Iter<'a, T>;

    fn access(visit: &mut dyn FnMut(TypeId, &'static str, bool)) {
        visit(TypeId::of::<T>(), std::any::type_name::<T>(), false);
    }

    fn matches(archetype: &Archetype, registry: &ComponentRegistry) -> bool {
        registry
            .component_id_of(TypeId::of::<T>())
            .map(|id| archetype.contains(id))
            .unwrap_or(false)
    }

    fn iter_archetype<'a>(
        archetype: &'a Archetype,
        registry: &ComponentRegistry,
    ) -> Option<Self::Iter<'a>> {
        let id = registry.component_id_of(TypeId::of::<T>())?;
        Some(column_slice::<T>(archetype.column(id)?)?.iter())
    }

    fn iter_archetype_mut<'a>(
        archetype: &'a mut Archetype,
        registry: &ComponentRegistry,
    ) -> Option<Self::IterMut<'a>> {
        let id = registry.component_id_of(TypeId::of::<T>())?;
        Some(column_slice::<T>(archetype.column(id)?)?.iter())
    }

    fn get_row<'a>(
        archetype: &'a Archetype,
        registry: &ComponentRegistry,
        row: usize,
    ) -> Option<Self::Item<'a>> {
        let id = registry.component_id_of(TypeId::of::<T>())?;
        column_slice::<T>(archetype.column(id)?)?.get(row)
    }

    fn get_row_mut<'a>(
        archetype: &'a mut Archetype,
        registry: &ComponentRegistry,
        row: usize,
    ) -> Option<Self::ItemMut<'a>> {
        let id = registry.component_id_of(TypeId::of::<T>())?;
        column_slice::<T>(archetype.column(id)?)?.get(row)
    }
}

impl<T: Component> sealed::Sealed for &mut T {}
impl<T: Component> QueryData for &mut T {
    type Item<'a> = &'a T;
    type ItemMut<'a> = &'a mut T;
    type Iter<'a> = std::slice::Iter<'a, T>;
    type IterMut<'a> = std::slice::IterMut<'a, T>;

    fn access(visit: &mut dyn FnMut(TypeId, &'static str, bool)) {
        visit(TypeId::of::<T>(), std::any::type_name::<T>(), true);
    }

    fn matches(archetype: &Archetype, registry: &ComponentRegistry) -> bool {
        registry
            .component_id_of(TypeId::of::<T>())
            .map(|id| archetype.contains(id))
            .unwrap_or(false)
    }

    fn iter_archetype<'a>(
        archetype: &'a Archetype,
        registry: &ComponentRegistry,
    ) -> Option<Self::Iter<'a>> {
        let id = registry.component_id_of(TypeId::of::<T>())?;
        Some(column_slice::<T>(archetype.column(id)?)?.iter())
    }

    fn iter_archetype_mut<'a>(
        archetype: &'a mut Archetype,
        registry: &ComponentRegistry,
    ) -> Option<Self::IterMut<'a>> {
        let id = registry.component_id_of(TypeId::of::<T>())?;
        Some(column_slice_mut::<T>(archetype.column_mut(id)?)?.iter_mut())
    }

    fn get_row<'a>(
        archetype: &'a Archetype,
        registry: &ComponentRegistry,
        row: usize,
    ) -> Option<Self::Item<'a>> {
        let id = registry.component_id_of(TypeId::of::<T>())?;
        column_slice::<T>(archetype.column(id)?)?.get(row)
    }

    fn get_row_mut<'a>(
        archetype: &'a mut Archetype,
        registry: &ComponentRegistry,
        row: usize,
    ) -> Option<Self::ItemMut<'a>> {
        let id = registry.component_id_of(TypeId::of::<T>())?;
        column_slice_mut::<T>(archetype.column_mut(id)?)?.get_mut(row)
    }
}

impl QueryElement for Entity {
    type Item<'a> = Entity;
    type ItemMut<'a> = Entity;
    type Iter<'a> = std::iter::Copied<std::slice::Iter<'a, Entity>>;
    type IterMut<'a> = std::iter::Copied<std::slice::Iter<'a, Entity>>;

    fn access(_visit: &mut dyn FnMut(TypeId, &'static str, bool)) {}

    fn required(_registry: &ComponentRegistry) -> ElementNeed {
        ElementNeed::Entity
    }

    fn make_iter<'a>(
        _column: Option<&'a dyn crate::component::ComponentColumn>,
        entities: &'a [Entity],
    ) -> Option<Self::Iter<'a>> {
        Some(entities.iter().copied())
    }

    fn make_iter_mut<'a>(
        _column: Option<&'a mut dyn crate::component::ComponentColumn>,
        entities: &'a [Entity],
    ) -> Option<Self::IterMut<'a>> {
        Some(entities.iter().copied())
    }
}

impl<T: Component> QueryElement for &T {
    type Item<'a> = &'a T;
    type ItemMut<'a> = &'a T;
    type Iter<'a> = std::slice::Iter<'a, T>;
    type IterMut<'a> = std::slice::Iter<'a, T>;

    fn access(visit: &mut dyn FnMut(TypeId, &'static str, bool)) {
        visit(TypeId::of::<T>(), std::any::type_name::<T>(), false);
    }

    fn required(registry: &ComponentRegistry) -> ElementNeed {
        match registry.component_id_of(TypeId::of::<T>()) {
            Some(id) => ElementNeed::Column(id),
            None => ElementNeed::Unregistered,
        }
    }

    fn make_iter<'a>(
        column: Option<&'a dyn crate::component::ComponentColumn>,
        _entities: &'a [Entity],
    ) -> Option<Self::Iter<'a>> {
        Some(column_slice::<T>(column?)?.iter())
    }

    fn make_iter_mut<'a>(
        column: Option<&'a mut dyn crate::component::ComponentColumn>,
        _entities: &'a [Entity],
    ) -> Option<Self::IterMut<'a>> {
        Some(column_slice::<T>(column?)?.iter())
    }
}

impl<T: Component> QueryElement for &mut T {
    type Item<'a> = &'a T;
    type ItemMut<'a> = &'a mut T;
    type Iter<'a> = std::slice::Iter<'a, T>;
    type IterMut<'a> = std::slice::IterMut<'a, T>;

    fn access(visit: &mut dyn FnMut(TypeId, &'static str, bool)) {
        visit(TypeId::of::<T>(), std::any::type_name::<T>(), true);
    }

    fn required(registry: &ComponentRegistry) -> ElementNeed {
        match registry.component_id_of(TypeId::of::<T>()) {
            Some(id) => ElementNeed::Column(id),
            None => ElementNeed::Unregistered,
        }
    }

    fn make_iter<'a>(
        column: Option<&'a dyn crate::component::ComponentColumn>,
        _entities: &'a [Entity],
    ) -> Option<Self::Iter<'a>> {
        Some(column_slice::<T>(column?)?.iter())
    }

    fn make_iter_mut<'a>(
        column: Option<&'a mut dyn crate::component::ComponentColumn>,
        _entities: &'a [Entity],
    ) -> Option<Self::IterMut<'a>> {
        Some(column_slice_mut::<T>(column?)?.iter_mut())
    }
}

include!("tuples.rs");

/// A prepared view over the archetypes matching `Q` and `F`. Construct via
/// [`World::query`](crate::world::World::query) /
/// [`query_mut`](crate::world::World::query_mut); attach filters with
/// [`with`](Query::with) / [`without`](Query::without) /
/// [`added`](Query::added) / [`changed`](Query::changed).
pub struct Query<'w, Q: QueryData, F: QueryFilter> {
    store: QueryStore<'w>,
    registry: &'w ComponentRegistry,
    last_run: Tick,
    change_tick: Tick,
    _marker: QueryMarker<Q, F>,
}

const NO_TICKS: &[ArchetypeTicks] = &[];

type QueryMarker<Q, F> = PhantomData<(fn() -> Q, fn() -> F)>;

enum QueryStore<'w> {
    Shared(&'w ArchetypeStore),
    Exclusive(&'w mut ArchetypeStore),
}

impl<'w> QueryStore<'w> {
    fn as_ref(&self) -> &ArchetypeStore {
        match self {
            QueryStore::Shared(s) => s,
            QueryStore::Exclusive(s) => s,
        }
    }
}

impl<'w, Q: QueryData, F: QueryFilter> Query<'w, Q, F> {
    pub(crate) fn new_shared(
        store: &'w ArchetypeStore,
        registry: &'w ComponentRegistry,
        last_run: Tick,
    ) -> Self {
        Self {
            store: QueryStore::Shared(store),
            registry,
            last_run,
            change_tick: Tick::ZERO,
            _marker: PhantomData,
        }
    }

    pub(crate) fn new_exclusive(
        store: &'w mut ArchetypeStore,
        registry: &'w ComponentRegistry,
        change_tick: Tick,
    ) -> Self {
        Self {
            store: QueryStore::Exclusive(store),
            registry,
            last_run: Tick::ZERO,
            change_tick,
            _marker: PhantomData,
        }
    }

    fn archetype_matches(&self, archetype: &Archetype) -> bool {
        Q::matches(archetype, self.registry) && F::matches(archetype, self.registry)
    }

    fn collect_writes(&self) -> ([Option<ComponentId>; 12], usize) {
        let registry = self.registry;
        let mut writes: [Option<ComponentId>; 12] = [None; 12];
        let mut n = 0usize;
        Q::access(&mut |tid, _name, write| {
            if write {
                if let Some(slot) = writes.get_mut(n) {
                    *slot = registry.component_id_of(tid);
                }
                n += 1;
            }
        });
        (writes, n)
    }

    /// Adds a [`With<T>`](filter::With) presence constraint.
    pub fn with<T: Component>(self) -> Query<'w, Q, (F, filter::With<T>)> {
        Query {
            store: self.store,
            registry: self.registry,
            last_run: self.last_run,
            change_tick: self.change_tick,
            _marker: PhantomData,
        }
    }

    /// Adds a [`Without<T>`](filter::Without) absence constraint.
    pub fn without<T: Component>(self) -> Query<'w, Q, (F, filter::Without<T>)> {
        Query {
            store: self.store,
            registry: self.registry,
            last_run: self.last_run,
            change_tick: self.change_tick,
            _marker: PhantomData,
        }
    }

    /// Adds an [`Added<T>`](filter::Added) change-detection constraint.
    pub fn added<T: Component>(self) -> Query<'w, Q, (F, filter::Added<T>)> {
        Query {
            store: self.store,
            registry: self.registry,
            last_run: self.last_run,
            change_tick: self.change_tick,
            _marker: PhantomData,
        }
    }

    /// Adds a [`Changed<T>`](filter::Changed) change-detection constraint.
    pub fn changed<T: Component>(self) -> Query<'w, Q, (F, filter::Changed<T>)> {
        Query {
            store: self.store,
            registry: self.registry,
            last_run: self.last_run,
            change_tick: self.change_tick,
            _marker: PhantomData,
        }
    }

    /// Shared iteration over matching rows. Allocation-free.
    pub fn iter(&self) -> QueryIter<'_, Q, F> {
        QueryIter {
            store: self.store.as_ref(),
            registry: self.registry,
            archetype: 0,
            current_arch: 0,
            current: None,
            row: 0,
            last_run: self.last_run,
            remaining: self.count(),
            _marker: PhantomData,
        }
    }

    /// Exclusive iteration over matching rows; required for any `&mut T` in `Q`.
    /// Allocation-free. A shared-only query yields nothing here. Stamps the
    /// changed tick of every `&mut` component in matching archetypes.
    pub fn iter_mut(&mut self) -> QueryIterMut<'_, Q, F> {
        let remaining = self.count();
        let (writes, nw) = self.collect_writes();
        let change_tick = self.change_tick;
        let last_run = self.last_run;
        let registry = self.registry;
        if nw > 0 {
            if let QueryStore::Exclusive(s) = &mut self.store {
                let arch_count = s.archetypes().len();
                for idx in 0..arch_count {
                    let matches = Q::matches(&s.archetypes()[idx], registry);
                    if !matches {
                        continue;
                    }
                    let aid = ArchetypeId::new(idx as u32);
                    for w in writes.iter().take(nw.min(12)).flatten() {
                        s.stamp_changed_all(aid, *w, change_tick);
                    }
                }
            }
        }
        let (archetypes, ticks) = match &mut self.store {
            QueryStore::Shared(_) => ([].iter_mut().enumerate(), NO_TICKS),
            QueryStore::Exclusive(s) => {
                let (a, t) = s.archetypes_and_ticks_mut();
                (a.iter_mut().enumerate(), t)
            }
        };
        QueryIterMut {
            archetypes,
            ticks,
            registry,
            current: None,
            current_arch: 0,
            row: 0,
            last_run,
            remaining,
            _marker: PhantomData,
        }
    }

    /// Random access by entity, respecting filters. `None` if the entity is not
    /// live, lacks a queried component, or fails a filter.
    pub fn get(&self, entity: Entity) -> Option<Q::Item<'_>> {
        let store = self.store.as_ref();
        let (aid, row) = store.location(entity)?;
        let archetype = store.archetype(aid);
        if !self.archetype_matches(archetype) {
            return None;
        }
        if F::NEEDS_ROW_FILTER
            && !F::row_matches(store.tick(aid), self.registry, row, self.last_run)
        {
            return None;
        }
        Q::get_row(archetype, self.registry, row)
    }

    /// Exclusive random access by entity, respecting filters. Stamps the changed
    /// tick of each `&mut` component at that row.
    pub fn get_mut(&mut self, entity: Entity) -> Option<Q::ItemMut<'_>> {
        let change_tick = self.change_tick;
        let last_run = self.last_run;
        let (writes, nw) = self.collect_writes();
        let (aid, row, ok) = {
            let store = self.store.as_ref();
            let (aid, row) = store.location(entity)?;
            let archetype = store.archetype(aid);
            let mut ok = self.archetype_matches(archetype);
            if ok && F::NEEDS_ROW_FILTER {
                ok = F::row_matches(store.tick(aid), self.registry, row, last_run);
            }
            (aid, row, ok)
        };
        if !ok {
            return None;
        }
        let store = match &mut self.store {
            QueryStore::Shared(_) => return None,
            QueryStore::Exclusive(s) => s,
        };
        for w in writes.iter().take(nw.min(12)).flatten() {
            store.mark_changed(aid, *w, row, change_tick);
        }
        Q::get_row_mut(store.archetype_mut(aid), self.registry, row)
    }

    /// Entities matching the query (and any row filters), in deterministic order.
    pub fn iter_entities(&self) -> impl Iterator<Item = Entity> + '_ {
        let store = self.store.as_ref();
        let registry = self.registry;
        let last_run = self.last_run;
        store
            .archetypes()
            .iter()
            .enumerate()
            .flat_map(move |(idx, a)| {
                let matches = Q::matches(a, registry) && F::matches(a, registry);
                let ticks = store.ticks();
                a.entities()
                    .iter()
                    .copied()
                    .enumerate()
                    .filter_map(move |(row, e)| {
                        if matches
                            && (!F::NEEDS_ROW_FILTER
                                || F::row_matches(&ticks[idx], registry, row, last_run))
                        {
                            Some(e)
                        } else {
                            None
                        }
                    })
            })
    }

    /// Matching-row count, honoring row filters.
    pub fn count(&self) -> usize {
        let store = self.store.as_ref();
        if !F::NEEDS_ROW_FILTER {
            return store
                .archetypes()
                .iter()
                .filter(|a| self.archetype_matches(a))
                .map(|a| a.len())
                .sum();
        }
        let ticks = store.ticks();
        let mut total = 0;
        for (idx, a) in store.archetypes().iter().enumerate() {
            if self.archetype_matches(a) {
                let t = &ticks[idx];
                for row in 0..a.len() {
                    if F::row_matches(t, self.registry, row, self.last_run) {
                        total += 1;
                    }
                }
            }
        }
        total
    }

    /// Returns `true` iff no row matches.
    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }
}

/// Allocation-free shared row iterator over a query's matching rows.
pub struct QueryIter<'q, Q: QueryData, F: QueryFilter> {
    store: &'q ArchetypeStore,
    registry: &'q ComponentRegistry,
    archetype: usize,
    current_arch: usize,
    current: Option<Q::Iter<'q>>,
    row: usize,
    last_run: Tick,
    remaining: usize,
    _marker: PhantomData<fn() -> F>,
}

impl<'q, Q: QueryData, F: QueryFilter> Iterator for QueryIter<'q, Q, F> {
    type Item = Q::Item<'q>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(iter) = &mut self.current {
                for item in iter.by_ref() {
                    let r = self.row;
                    self.row += 1;
                    if F::NEEDS_ROW_FILTER
                        && !F::row_matches(
                            &self.store.ticks()[self.current_arch],
                            self.registry,
                            r,
                            self.last_run,
                        )
                    {
                        continue;
                    }
                    self.remaining -= 1;
                    return Some(item);
                }
                self.current = None;
            }
            let archetypes = self.store.archetypes();
            let mut advanced = false;
            while self.archetype < archetypes.len() {
                let idx = self.archetype;
                let a = &archetypes[idx];
                self.archetype += 1;
                if Q::matches(a, self.registry) && F::matches(a, self.registry) {
                    if let Some(it) = Q::iter_archetype(a, self.registry) {
                        self.current = Some(it);
                        self.current_arch = idx;
                        self.row = 0;
                        advanced = true;
                        break;
                    }
                }
            }
            if !advanced {
                return None;
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<'q, Q: QueryData, F: QueryFilter> ExactSizeIterator for QueryIter<'q, Q, F> {}

/// Allocation-free exclusive row iterator. Walks archetypes via a slice
/// `iter_mut`, so each archetype's columns are borrowed disjointly for the
/// iterator's lifetime without `unsafe`; ticks are read from the parallel
/// shared borrow.
pub struct QueryIterMut<'q, Q: QueryData, F: QueryFilter> {
    archetypes: std::iter::Enumerate<std::slice::IterMut<'q, Archetype>>,
    ticks: &'q [ArchetypeTicks],
    registry: &'q ComponentRegistry,
    current: Option<Q::IterMut<'q>>,
    current_arch: usize,
    row: usize,
    last_run: Tick,
    remaining: usize,
    _marker: PhantomData<fn() -> F>,
}

impl<'q, Q: QueryData, F: QueryFilter> Iterator for QueryIterMut<'q, Q, F> {
    type Item = Q::ItemMut<'q>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(iter) = &mut self.current {
                for item in iter.by_ref() {
                    let r = self.row;
                    self.row += 1;
                    if F::NEEDS_ROW_FILTER
                        && !F::row_matches(
                            &self.ticks[self.current_arch],
                            self.registry,
                            r,
                            self.last_run,
                        )
                    {
                        continue;
                    }
                    self.remaining -= 1;
                    return Some(item);
                }
                self.current = None;
            }
            let registry = self.registry;
            let mut found = None;
            for (idx, a) in self.archetypes.by_ref() {
                if Q::matches(a, registry) && F::matches(a, registry) {
                    if let Some(it) = Q::iter_archetype_mut(a, registry) {
                        found = Some((idx, it));
                        break;
                    }
                }
            }
            match found {
                Some((idx, it)) => {
                    self.current = Some(it);
                    self.current_arch = idx;
                    self.row = 0;
                }
                None => return None,
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<'q, Q: QueryData, F: QueryFilter> ExactSizeIterator for QueryIterMut<'q, Q, F> {}

/// Per-system persistent state for a [`Query`] parameter: the change tick at
/// which the system last ran, used to evaluate [`Added`](filter::Added) /
/// [`Changed`](filter::Changed) filters.
#[doc(hidden)]
#[derive(Default)]
pub struct QueryState {
    last_run: Tick,
}

impl QueryState {
    pub(crate) fn take_last_run(&mut self, current: Tick) -> Tick {
        let prev = self.last_run;
        self.last_run = current;
        prev
    }
}
