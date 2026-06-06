//! Query filters: [`With`] / [`Without`] presence/absence constraints.
//!
//! Filters constrain which archetypes match but never contribute to read/write
//! access sets — they require only presence or absence, not data access. A
//! filter on an unregistered component matches nothing (`With`) or every
//! archetype (`Without`), never an error.

use crate::archetype::Archetype;
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

/// Archetype presence/absence constraint. Sealed.
pub trait QueryFilter: sealed::Sealed {
    /// Returns `true` iff `archetype` satisfies this filter.
    fn matches(archetype: &Archetype, registry: &ComponentRegistry) -> bool;
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

macro_rules! impl_filter_tuple {
    ($($name:ident),+) => {
        impl<$($name: QueryFilter),+> sealed::Sealed for ($($name,)+) {}

        impl<$($name: QueryFilter),+> QueryFilter for ($($name,)+) {
            fn matches(archetype: &Archetype, registry: &ComponentRegistry) -> bool {
                $($name::matches(archetype, registry))&&+
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
