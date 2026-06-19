//! Query filters: [`With`] / [`Without`] presence constraints and the
//! [`Added`] / [`Changed`] change-detection filters.
//!
//! Presence filters constrain which archetypes match but never contribute to
//! read/write access sets — they require only presence or absence, not data
//! access. Change filters additionally evaluate a per-row tick against the
//! querying system's last-run tick (see [`crate::change::Tick`]). A filter on an
//! unregistered component matches nothing (`With`/`Added`/`Changed`) or every
//! archetype (`Without`), never an error.

use crate::archetype::{Archetype, ArchetypeTicks};
use crate::change::Tick;
use crate::component::{Component, ComponentRegistry};
use std::any::TypeId;
use std::marker::PhantomData;

mod sealed {
    pub trait Sealed {}
}

/// Requires that matched archetypes contain `T`.
pub struct With<T: Component>(PhantomData<fn() -> T>);

/// Requires that matched archetypes do not contain `T`.
pub struct Without<T: Component>(PhantomData<fn() -> T>);

/// Matches only rows whose `T` was added since the querying system last ran.
pub struct Added<T: Component>(PhantomData<fn() -> T>);

/// Matches only rows whose `T` was added or mutably accessed since the querying
/// system last ran.
pub struct Changed<T: Component>(PhantomData<fn() -> T>);

/// Archetype presence/absence constraint, optionally refined per row. Sealed.
pub trait QueryFilter: sealed::Sealed {
    /// Whether this filter needs per-row evaluation. `false` for `()`, `With`,
    /// `Without`, and tuples of those — the iterators then skip the per-row check
    /// entirely (the allocation-free fast path).
    const NEEDS_ROW_FILTER: bool = false;

    /// Returns `true` iff `archetype` satisfies this filter at the archetype
    /// level.
    fn matches(archetype: &Archetype, registry: &ComponentRegistry) -> bool;

    /// Returns `true` iff row `row` satisfies this filter, given the archetype's
    /// ticks and the reader's `last_run`. Default `true` (archetype-level filters
    /// accept every row of a matching archetype).
    fn row_matches(
        _ticks: &ArchetypeTicks,
        _registry: &ComponentRegistry,
        _row: usize,
        _last_run: Tick,
    ) -> bool {
        true
    }
}

impl sealed::Sealed for () {}
impl QueryFilter for () {
    fn matches(_archetype: &Archetype, _registry: &ComponentRegistry) -> bool {
        true
    }
}

impl<T: Component> sealed::Sealed for With<T> {}
impl<T: Component> QueryFilter for With<T> {
    fn matches(archetype: &Archetype, registry: &ComponentRegistry) -> bool {
        match registry.component_id_of(TypeId::of::<T>()) {
            Some(id) => archetype.contains(id),
            None => false,
        }
    }
}

impl<T: Component> sealed::Sealed for Without<T> {}
impl<T: Component> QueryFilter for Without<T> {
    fn matches(archetype: &Archetype, registry: &ComponentRegistry) -> bool {
        match registry.component_id_of(TypeId::of::<T>()) {
            Some(id) => !archetype.contains(id),
            None => true,
        }
    }
}

impl<T: Component> sealed::Sealed for Added<T> {}
impl<T: Component> QueryFilter for Added<T> {
    const NEEDS_ROW_FILTER: bool = true;

    fn matches(archetype: &Archetype, registry: &ComponentRegistry) -> bool {
        match registry.component_id_of(TypeId::of::<T>()) {
            Some(id) => archetype.contains(id),
            None => false,
        }
    }

    fn row_matches(
        ticks: &ArchetypeTicks,
        registry: &ComponentRegistry,
        row: usize,
        last_run: Tick,
    ) -> bool {
        match registry.component_id_of(TypeId::of::<T>()) {
            Some(id) => ticks
                .added_at(id, row)
                .map(|t| t > last_run)
                .unwrap_or(false),
            None => false,
        }
    }
}

impl<T: Component> sealed::Sealed for Changed<T> {}
impl<T: Component> QueryFilter for Changed<T> {
    const NEEDS_ROW_FILTER: bool = true;

    fn matches(archetype: &Archetype, registry: &ComponentRegistry) -> bool {
        match registry.component_id_of(TypeId::of::<T>()) {
            Some(id) => archetype.contains(id),
            None => false,
        }
    }

    fn row_matches(
        ticks: &ArchetypeTicks,
        registry: &ComponentRegistry,
        row: usize,
        last_run: Tick,
    ) -> bool {
        match registry.component_id_of(TypeId::of::<T>()) {
            Some(id) => ticks
                .changed_at(id, row)
                .map(|t| t > last_run)
                .unwrap_or(false),
            None => false,
        }
    }
}

macro_rules! impl_filter_tuple {
    ($($name:ident),+) => {
        impl<$($name: QueryFilter),+> sealed::Sealed for ($($name,)+) {}

        impl<$($name: QueryFilter),+> QueryFilter for ($($name,)+) {
            const NEEDS_ROW_FILTER: bool = $($name::NEEDS_ROW_FILTER)||+;

            fn matches(archetype: &Archetype, registry: &ComponentRegistry) -> bool {
                $($name::matches(archetype, registry))&&+
            }

            fn row_matches(
                ticks: &ArchetypeTicks,
                registry: &ComponentRegistry,
                row: usize,
                last_run: Tick,
            ) -> bool {
                $($name::row_matches(ticks, registry, row, last_run))&&+
            }
        }
    };
}

impl_filter_tuple!(A);
impl_filter_tuple!(A, B);
impl_filter_tuple!(A, B, C);
impl_filter_tuple!(A, B, C, D);
impl_filter_tuple!(A, B, C, D, E);
impl_filter_tuple!(A, B, C, D, E, F);
impl_filter_tuple!(A, B, C, D, E, F, G);
impl_filter_tuple!(A, B, C, D, E, F, G, H);
