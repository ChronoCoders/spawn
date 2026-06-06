//! The query engine: [`Query`], [`QueryData`], and allocation-free iterators.
//!
//! A query zips the SoA columns of every matching archetype. Items borrow
//! directly into the columns through standard slice iterators: no per-item
//! allocation, no intermediate buffer, no `unsafe`. Iteration order is
//! deterministic — archetypes in [`ArchetypeId`](crate::archetype::ArchetypeId)
//! order, rows in column order.

pub mod filter;

use crate::archetype::{Archetype, ArchetypeStore};
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
/// [`with`](Query::with) / [`without`](Query::without).
pub struct Query<'w, Q: QueryData, F: QueryFilter> {
    store: QueryStore<'w>,
    registry: &'w ComponentRegistry,
    _marker: QueryMarker<Q, F>,
}

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
    pub(crate) fn new_shared(store: &'w ArchetypeStore, registry: &'w ComponentRegistry) -> Self {
        Self {
            store: QueryStore::Shared(store),
            registry,
            _marker: PhantomData,
        }
    }

    pub(crate) fn new_exclusive(
        store: &'w mut ArchetypeStore,
        registry: &'w ComponentRegistry,
    ) -> Self {
        Self {
            store: QueryStore::Exclusive(store),
            registry,
            _marker: PhantomData,
        }
    }

    fn archetype_matches(&self, archetype: &Archetype) -> bool {
        Q::matches(archetype, self.registry) && F::matches(archetype, self.registry)
    }

    /// Adds a [`With<T>`](filter::With) presence constraint.
    pub fn with<T: Component>(self) -> Query<'w, Q, (F, filter::With<T>)> {
        Query {
            store: self.store,
            registry: self.registry,
            _marker: PhantomData,
        }
    }

    /// Adds a [`Without<T>`](filter::Without) absence constraint.
    pub fn without<T: Component>(self) -> Query<'w, Q, (F, filter::Without<T>)> {
        Query {
            store: self.store,
            registry: self.registry,
            _marker: PhantomData,
        }
    }

    /// Shared iteration over matching rows. Allocation-free.
    pub fn iter(&self) -> QueryIter<'_, Q, F> {
        QueryIter {
            store: self.store.as_ref(),
            registry: self.registry,
            archetype: 0,
            current: None,
            remaining: self.count(),
            _marker: PhantomData,
        }
    }

    /// Exclusive iteration over matching rows; required for any `&mut T` in `Q`.
    /// Allocation-free. A shared-only query yields nothing here.
    pub fn iter_mut(&mut self) -> QueryIterMut<'_, Q, F> {
        let remaining = self.count();
        let registry = self.registry;
        let archetypes = match &mut self.store {
            QueryStore::Shared(_) => [].iter_mut(),
            QueryStore::Exclusive(s) => s.archetypes_mut().iter_mut(),
        };
        QueryIterMut {
            archetypes,
            registry,
            current: None,
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
        Q::get_row(archetype, self.registry, row)
    }

    /// Exclusive random access by entity, respecting filters.
    pub fn get_mut(&mut self, entity: Entity) -> Option<Q::ItemMut<'_>> {
        let matches;
        let (aid, row) = {
            let store = self.store.as_ref();
            let (aid, row) = store.location(entity)?;
            matches = self.archetype_matches(store.archetype(aid));
            (aid, row)
        };
        if !matches {
            return None;
        }
        let store = match &mut self.store {
            QueryStore::Shared(_) => return None,
            QueryStore::Exclusive(s) => s,
        };
        Q::get_row_mut(store.archetype_mut(aid), self.registry, row)
    }

    /// Entities matching the query, in deterministic iteration order.
    pub fn iter_entities(&self) -> impl Iterator<Item = Entity> + '_ {
        let store = self.store.as_ref();
        store
            .archetypes()
            .iter()
            .filter(move |a| self.archetype_matches(a))
            .flat_map(|a| a.entities().iter().copied())
    }

    /// Matching-entity count without touching component columns.
    pub fn count(&self) -> usize {
        let store = self.store.as_ref();
        store
            .archetypes()
            .iter()
            .filter(|a| self.archetype_matches(a))
            .map(|a| a.len())
            .sum()
    }

    /// Returns `true` iff no entity matches.
    pub fn is_empty(&self) -> bool {
        let store = self.store.as_ref();
        !store
            .archetypes()
            .iter()
            .any(|a| self.archetype_matches(a) && a.len() > 0)
    }
}

/// Allocation-free shared row iterator. State is an archetype cursor plus the
/// current archetype's row iterator.
pub struct QueryIter<'q, Q: QueryData, F: QueryFilter> {
    store: &'q ArchetypeStore,
    registry: &'q ComponentRegistry,
    archetype: usize,
    current: Option<Q::Iter<'q>>,
    remaining: usize,
    _marker: PhantomData<fn() -> F>,
}

impl<'q, Q: QueryData, F: QueryFilter> Iterator for QueryIter<'q, Q, F> {
    type Item = Q::Item<'q>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(iter) = &mut self.current {
                if let Some(item) = iter.next() {
                    self.remaining -= 1;
                    return Some(item);
                }
                self.current = None;
            }
            let archetypes = self.store.archetypes();
            while self.archetype < archetypes.len() {
                let a = &archetypes[self.archetype];
                self.archetype += 1;
                if Q::matches(a, self.registry) && F::matches(a, self.registry) {
                    self.current = Q::iter_archetype(a, self.registry);
                    if self.current.is_some() {
                        break;
                    }
                }
            }
            self.current.as_ref()?;
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<'q, Q: QueryData, F: QueryFilter> ExactSizeIterator for QueryIter<'q, Q, F> {}

/// Allocation-free exclusive row iterator. Walks archetypes via a slice
/// `iter_mut`, so each archetype's columns are borrowed disjointly for the
/// iterator's lifetime without `unsafe`.
pub struct QueryIterMut<'q, Q: QueryData, F: QueryFilter> {
    archetypes: std::slice::IterMut<'q, Archetype>,
    registry: &'q ComponentRegistry,
    current: Option<Q::IterMut<'q>>,
    remaining: usize,
    _marker: PhantomData<fn() -> F>,
}

impl<'q, Q: QueryData, F: QueryFilter> Iterator for QueryIterMut<'q, Q, F> {
    type Item = Q::ItemMut<'q>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(iter) = &mut self.current {
                if let Some(item) = iter.next() {
                    self.remaining -= 1;
                    return Some(item);
                }
                self.current = None;
            }
            let registry = self.registry;
            let next = self
                .archetypes
                .by_ref()
                .find(|a| Q::matches(a, registry) && F::matches(a, registry))?;
            self.current = Q::iter_archetype_mut(next, registry);
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<'q, Q: QueryData, F: QueryFilter> ExactSizeIterator for QueryIterMut<'q, Q, F> {}
