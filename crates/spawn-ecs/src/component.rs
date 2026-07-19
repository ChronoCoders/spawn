//! Components, their dense ids, and the type-erased column registry.
//!
//! Columns are stored as `Box<dyn ComponentColumn>` wrapping a `Vec<T>`; all
//! type erasure is done with safe `Any` downcasts and ordinary function
//! pointers, no `unsafe`.

use std::any::{Any, TypeId};
use std::collections::HashMap;

/// A type-erased, thread-safe component value moved between columns during
/// archetype relocation. `Send + Sync` so command buffers can cross the scoped
/// threads used by the scheduler. `#[doc(hidden)] pub` only to satisfy the
/// sealed [`Bundle`](crate::bundle::Bundle) supertrait's signature visibility.
#[doc(hidden)]
pub type AnyValue = Box<dyn Any + Send + Sync>;

/// Marker trait for component types.
///
/// The `Send + Sync + 'static` bound is what makes parallel scheduling sound;
/// opting in per type (no blanket impl) keeps the set of stored types auditable.
pub trait Component: Send + Sync + 'static {}

/// Spatial transforms are first-class engine components, so they opt into
/// [`Component`] here (the orphan rule requires the impls to live in the crate
/// that owns the trait).
impl Component for spawn_core::Transform3D {}
impl Component for spawn_core::Transform2D {}

/// Dense, contiguous identifier assigned to a component type in registration
/// order, starting at `0`. Canonical key for column layout and access sets.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ComponentId(u32);

impl ComponentId {
    pub(crate) const fn new(raw: u32) -> Self {
        Self(raw)
    }

    pub(crate) const fn index(self) -> usize {
        self.0 as usize
    }

    #[cfg(test)]
    pub(crate) const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
}

/// A type-erased SoA column.
///
/// Every method is implemented in terms of safe downcasts; rows move between
/// columns by pop/push of boxed values, never by raw memory copies.
/// `#[doc(hidden)] pub` only so sealed query traits can name it; not stable API.
#[doc(hidden)]
pub trait ComponentColumn: Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    /// Removes the row at `index` by swapping the last row into its place. The
    /// removed value is returned boxed so the caller can relocate it.
    fn swap_remove(&mut self, index: usize) -> AnyValue;
    /// Pushes a boxed value previously taken from a matching column.
    fn push_boxed(&mut self, value: AnyValue);
}

/// Concrete column backing a single component type.
struct TypedColumn<T: Component> {
    data: Vec<T>,
}

impl<T: Component> ComponentColumn for TypedColumn<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn swap_remove(&mut self, index: usize) -> AnyValue {
        Box::new(self.data.swap_remove(index))
    }

    fn push_boxed(&mut self, value: AnyValue) {
        if let Ok(value) = value.downcast::<T>() {
            self.data.push(*value);
        }
    }
}

impl<T: Component> TypedColumn<T> {
    fn boxed_empty() -> Box<dyn ComponentColumn> {
        Box::new(Self { data: Vec::new() })
    }

    fn downcast(column: &dyn ComponentColumn) -> Option<&[T]> {
        column.as_any().downcast_ref::<Self>().map(|c| &c.data[..])
    }

    fn downcast_mut(column: &mut dyn ComponentColumn) -> Option<&mut [T]> {
        column
            .as_any_mut()
            .downcast_mut::<Self>()
            .map(|c| &mut c.data[..])
    }
}

/// Read a typed slice out of an erased column, or `None` on type mismatch.
pub(crate) fn column_slice<T: Component>(column: &dyn ComponentColumn) -> Option<&[T]> {
    TypedColumn::<T>::downcast(column)
}

/// Read a typed mutable slice out of an erased column, or `None` on mismatch.
pub(crate) fn column_slice_mut<T: Component>(column: &mut dyn ComponentColumn) -> Option<&mut [T]> {
    TypedColumn::<T>::downcast_mut(column)
}

/// A placeholder column with no storage, used only as a total fallback for an
/// unregistered id; the archetype layer never pushes into it.
struct UnitColumn;

impl ComponentColumn for UnitColumn {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn swap_remove(&mut self, _index: usize) -> AnyValue {
        Box::new(())
    }
    fn push_boxed(&mut self, _value: AnyValue) {}
}

pub(crate) fn unit_column() -> Box<dyn ComponentColumn> {
    Box::new(UnitColumn)
}

/// Metadata recorded for a registered component type.
struct ComponentInfo {
    factory: fn() -> Box<dyn ComponentColumn>,
}

/// Maps component types to dense [`ComponentId`]s and remembers how to build a
/// fresh column for each.
///
/// Public only to satisfy the sealed [`Bundle`](crate::bundle::Bundle) trait's
/// supertrait visibility; it is `#[doc(hidden)]`, never re-exported, and not
/// part of the engine's stable surface.
#[doc(hidden)]
pub struct ComponentRegistry {
    by_type: HashMap<TypeId, ComponentId>,
    infos: Vec<ComponentInfo>,
}

impl ComponentRegistry {
    pub(crate) fn new() -> Self {
        Self {
            by_type: HashMap::new(),
            infos: Vec::new(),
        }
    }

    /// Registers `T` (idempotent): re-registering returns the existing id.
    pub(crate) fn register<T: Component>(&mut self) -> ComponentId {
        let type_id = TypeId::of::<T>();
        if let Some(id) = self.by_type.get(&type_id) {
            return *id;
        }
        let id = ComponentId::new(self.infos.len() as u32);
        self.infos.push(ComponentInfo {
            factory: TypedColumn::<T>::boxed_empty,
        });
        self.by_type.insert(type_id, id);
        id
    }

    /// Returns the id of `T`, or `None` if `T` was never registered.
    pub(crate) fn component_id<T: Component>(&self) -> Option<ComponentId> {
        self.by_type.get(&TypeId::of::<T>()).copied()
    }

    pub(crate) fn component_id_of(&self, type_id: TypeId) -> Option<ComponentId> {
        self.by_type.get(&type_id).copied()
    }

    pub(crate) fn new_column(&self, id: ComponentId) -> Option<Box<dyn ComponentColumn>> {
        self.infos.get(id.index()).map(|info| (info.factory)())
    }

    #[cfg(test)]
    pub(crate) fn count(&self) -> usize {
        self.infos.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const _: () = assert!(std::mem::size_of::<ComponentId>() == 4);

    struct A(u32);
    struct B;
    impl Component for A {}
    impl Component for B {}

    #[test]
    fn registration_is_dense_and_idempotent() {
        let mut reg = ComponentRegistry::new();
        let a0 = reg.register::<A>();
        let b0 = reg.register::<B>();
        let a1 = reg.register::<A>();
        assert_eq!(a0, a1);
        assert_eq!(a0, ComponentId::new(0));
        assert_eq!(b0, ComponentId::new(1));
        assert_eq!(reg.count(), 2);
    }

    #[test]
    fn lookup_none_when_unregistered() {
        let reg = ComponentRegistry::new();
        assert!(reg.component_id::<A>().is_none());
    }

    #[test]
    fn column_roundtrip() {
        let mut reg = ComponentRegistry::new();
        let id = reg.register::<A>();
        let mut col = reg.new_column(id).unwrap();
        col.push_boxed(Box::new(A(7)));
        col.push_boxed(Box::new(A(9)));
        assert_eq!(column_slice::<A>(col.as_ref()).unwrap().len(), 2);
        assert_eq!(column_slice::<A>(col.as_ref()).unwrap()[1].0, 9);
        let removed = col.swap_remove(0);
        assert_eq!(removed.downcast::<A>().unwrap().0, 7);
        assert_eq!(column_slice::<A>(col.as_ref()).unwrap()[0].0, 9);
    }

    #[test]
    fn typed_mismatch_returns_none() {
        let mut reg = ComponentRegistry::new();
        let id = reg.register::<A>();
        let col = reg.new_column(id).unwrap();
        assert!(column_slice::<B>(col.as_ref()).is_none());
    }
}
