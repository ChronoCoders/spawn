//! The [`Bundle`] trait: a type-erased set of components placed into an
//! archetype in one move. Implemented for single components and for tuples up to
//! arity 12; sealed so downstream crates compose tuples rather than impl it.
//!
//! The public trait is a marker; the actual registration/erasure logic lives on
//! the crate-private sealed supertrait so the component registry never leaks
//! into the public API.

use crate::component::{AnyValue, Component, ComponentId, ComponentRegistry};

mod sealed {
    pub trait Sealed {}
}

/// A composable set of components placed into one archetype in a single move.
/// Sealed: implemented for single components and tuples up to arity 12; compose
/// tuples rather than implementing it.
pub trait Bundle: BundleWrite + sealed::Sealed + Send + Sync + 'static {}

/// Placement logic for a [`Bundle`]. `#[doc(hidden)]` and not intended for
/// external implementation — its signatures use crate-internal erased types,
/// and the [`sealed::Sealed`] supertrait keeps it crate-private in practice.
#[doc(hidden)]
pub trait BundleWrite: sealed::Sealed {
    /// Registers every component type, assigning ids, and returns the erased
    /// values to store.
    fn write_into(self, registry: &mut ComponentRegistry) -> Vec<(ComponentId, AnyValue)>;

    /// Like [`write_into`](BundleWrite::write_into) but without registering: a
    /// component type not already registered is skipped. Used by deferred
    /// `Commands::spawn_with`, which holds only a shared registry.
    fn write_into_registered(self, registry: &ComponentRegistry) -> Vec<(ComponentId, AnyValue)>;
}

impl<T: Component> sealed::Sealed for T {}
impl<T: Component> Bundle for T {}

impl<T: Component> BundleWrite for T {
    fn write_into(self, registry: &mut ComponentRegistry) -> Vec<(ComponentId, AnyValue)> {
        let id = registry.register::<T>();
        vec![(id, Box::new(self) as AnyValue)]
    }

    fn write_into_registered(self, registry: &ComponentRegistry) -> Vec<(ComponentId, AnyValue)> {
        match registry.component_id::<T>() {
            Some(id) => vec![(id, Box::new(self) as AnyValue)],
            None => Vec::new(),
        }
    }
}

macro_rules! impl_bundle_tuple {
    ($($name:ident),+) => {
        impl<$($name: Component),+> sealed::Sealed for ($($name,)+) {}
        impl<$($name: Component),+> Bundle for ($($name,)+) {}

        impl<$($name: Component),+> BundleWrite for ($($name,)+) {
            #[allow(non_snake_case)]
            fn write_into(
                self,
                registry: &mut ComponentRegistry,
            ) -> Vec<(ComponentId, AnyValue)> {
                let ($($name,)+) = self;
                vec![
                    $({
                        let id = registry.register::<$name>();
                        (id, Box::new($name) as AnyValue)
                    },)+
                ]
            }

            #[allow(non_snake_case)]
            fn write_into_registered(
                self,
                registry: &ComponentRegistry,
            ) -> Vec<(ComponentId, AnyValue)> {
                let ($($name,)+) = self;
                let mut out = Vec::new();
                $(if let Some(id) = registry.component_id::<$name>() {
                    out.push((id, Box::new($name) as AnyValue));
                })+
                out
            }
        }
    };
}

impl_bundle_tuple!(A);
impl_bundle_tuple!(A, B);
impl_bundle_tuple!(A, B, C);
impl_bundle_tuple!(A, B, C, D);
impl_bundle_tuple!(A, B, C, D, E);
impl_bundle_tuple!(A, B, C, D, E, F);
impl_bundle_tuple!(A, B, C, D, E, F, G);
impl_bundle_tuple!(A, B, C, D, E, F, G, H);
impl_bundle_tuple!(A, B, C, D, E, F, G, H, I);
impl_bundle_tuple!(A, B, C, D, E, F, G, H, I, J);
impl_bundle_tuple!(A, B, C, D, E, F, G, H, I, J, K);
impl_bundle_tuple!(A, B, C, D, E, F, G, H, I, J, K, L);

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct A(u32);
    #[derive(Debug, PartialEq)]
    struct B(f32);
    impl Component for A {}
    impl Component for B {}

    #[test]
    fn single_component_bundle() {
        let mut reg = ComponentRegistry::new();
        let pairs = A(3).write_into(&mut reg);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, reg.component_id::<A>().unwrap());
    }

    #[test]
    fn tuple_bundle_registers_all() {
        let mut reg = ComponentRegistry::new();
        let pairs = (A(1), B(2.0)).write_into(&mut reg);
        assert_eq!(pairs.len(), 2);
        assert!(reg.component_id::<A>().is_some());
        assert!(reg.component_id::<B>().is_some());
    }
}
