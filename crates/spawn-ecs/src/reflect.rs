//! Minimal component reflection for the editor inspector.
//!
//! This is exactly what the Phase 2c inspector needs and no more: for a selected
//! entity, ask which components are on it, what their fields are, what type each
//! field is, and read/write a field by name. It is not serialization, not scene
//! save, and not scripting access.
//!
//! A component opts into [`Reflect`] (like [`Component`]) and exposes a static
//! list of scalar leaf fields. Aggregates (`Vec3`, `Quat`, the transforms) are
//! decomposed to their scalar leaves addressed by a dotted path
//! (`"translation.x"`). Dispatch is through stored `fn` pointers (the private
//! [`ReflectVtable`]), built once at registration, never `&dyn Reflect` in the
//! per-field path, and never per-access allocation.

use std::error::Error;
use std::fmt;

use crate::component::{Component, ComponentId};
use crate::entity::Entity;
use crate::world::World;

/// The type of a reflected scalar leaf: exactly the kinds the inspector renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    F32,
    I32,
    U32,
    U64,
    Bool,
}

/// A reflected scalar leaf value (a copy; leaves are `Copy` primitives).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldValue {
    F32(f32),
    I32(i32),
    U32(u32),
    U64(u64),
    Bool(bool),
}

impl FieldValue {
    /// The [`FieldKind`] this value carries.
    pub fn kind(self) -> FieldKind {
        match self {
            FieldValue::F32(_) => FieldKind::F32,
            FieldValue::I32(_) => FieldKind::I32,
            FieldValue::U32(_) => FieldKind::U32,
            FieldValue::U64(_) => FieldKind::U64,
            FieldValue::Bool(_) => FieldKind::Bool,
        }
    }

    /// Extracts the `f32` payload, or [`ReflectError::TypeMismatch`] for any
    /// other kind. The provided transform impls store only `f32` leaves.
    fn expect_f32(self) -> ReflectResult<f32> {
        match self {
            FieldValue::F32(v) => Ok(v),
            _ => Err(ReflectError::TypeMismatch {
                context: "reflect: field expects f32",
            }),
        }
    }
}

/// Static descriptor of one reflected field: a scalar leaf, possibly a dotted
/// path into an aggregate (e.g. `"translation.x"`, `"rotation.w"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldDesc {
    /// Stable, `'static` leaf path; also the value passed back to read/write it.
    pub name: &'static str,
    /// The leaf's primitive kind.
    pub kind: FieldKind,
}

/// A reflection failure. `&'static str` contexts / `Copy` payloads keep
/// construction allocation-free; never used to panic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReflectError {
    /// The [`ComponentId`] was never `register_reflect`-ed.
    ComponentNotReflected {
        /// The id with no reflect vtable.
        component: ComponentId,
    },
    /// The entity is dead or does not have that component.
    MissingComponent {
        /// The offending identity.
        entity: Entity,
    },
    /// The field path is not a leaf this component exposes.
    UnknownField {
        /// Failure-class context (not the dynamic field name, which is not
        /// `'static`).
        context: &'static str,
    },
    /// The written [`FieldValue`] kind differs from the field's [`FieldKind`].
    TypeMismatch {
        /// Failure-class context.
        context: &'static str,
    },
}

impl fmt::Display for ReflectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ComponentNotReflected { component } => {
                write!(f, "component not reflected: {component:?}")
            }
            Self::MissingComponent { entity } => write!(
                f,
                "entity missing component: index {} generation {}",
                entity.index(),
                entity.generation()
            ),
            Self::UnknownField { context } => write!(f, "{context}"),
            Self::TypeMismatch { context } => write!(f, "{context}"),
        }
    }
}

impl Error for ReflectError {}

/// Result alias for fallible reflection operations.
pub type ReflectResult<T> = Result<T, ReflectError>;

/// A component type that exposes its scalar leaf fields for inspection.
///
/// Opt-in per type (no blanket impl), mirroring [`Component`]. Field names are
/// stable and `'static`; the [`fields`](Reflect::fields) order is the inspector's
/// display order.
pub trait Reflect: Component {
    /// Display label shown as the inspector section header (e.g. `"Transform3D"`).
    fn type_name() -> &'static str
    where
        Self: Sized;

    /// The ordered, static list of scalar leaf fields. A `static` slice in the
    /// impl, never allocated per call.
    fn fields() -> &'static [FieldDesc]
    where
        Self: Sized;

    /// Reads the leaf at `field`. `None` if `field` is not a recognized leaf path.
    fn reflect_get(&self, field: &str) -> Option<FieldValue>;

    /// Writes the leaf at `field`. [`ReflectError::UnknownField`] if `field` is
    /// not a recognized leaf path; [`ReflectError::TypeMismatch`] if `value`'s
    /// kind differs from the field's declared [`FieldKind`]. Never panics.
    fn reflect_set(&mut self, field: &str, value: FieldValue) -> ReflectResult<()>;
}

/// A read-only descriptor of one reflected component present on an entity.
#[derive(Debug, Clone, Copy)]
pub struct ReflectedComponent {
    /// The component's dense id.
    pub component: ComponentId,
    /// Its display label ([`Reflect::type_name`]).
    pub type_name: &'static str,
    /// Its leaf fields ([`Reflect::fields`]).
    pub fields: &'static [FieldDesc],
}

/// `fn`-pointer signatures for the reflect vtable (named to satisfy
/// `clippy::type_complexity` and to give the `set` pointer a returnable type).
type PresentFn = fn(&World, Entity) -> bool;
type GetFn = fn(&World, Entity, &str) -> Option<FieldValue>;
type SetFn = fn(&mut World, Entity, &str, FieldValue) -> ReflectResult<()>;

/// Monomorphized `fn`-pointer dispatch for one reflected component type. Built
/// once at registration; `fn` pointers are `Copy`, which lets a caller copy one
/// out before taking `&mut World`, side-stepping a borrow conflict.
pub(crate) struct ReflectVtable {
    type_name: &'static str,
    fields: &'static [FieldDesc],
    present: PresentFn,
    get: GetFn,
    set: SetFn,
}

impl ReflectVtable {
    fn of<T: Reflect>() -> Self {
        Self {
            type_name: T::type_name(),
            fields: T::fields(),
            present: |world, entity| world.get::<T>(entity).is_some(),
            get: |world, entity, field| world.get::<T>(entity).and_then(|c| c.reflect_get(field)),
            set: |world, entity, field, value| match world.get_mut::<T>(entity) {
                Some(component) => component.reflect_set(field, value),
                None => Err(ReflectError::MissingComponent { entity }),
            },
        }
    }
}

/// Per-`World` table of reflect vtables, indexed by [`ComponentId`]. Private to
/// the crate; only the value/desc/trait/error types are public.
pub(crate) struct ReflectRegistry {
    vtables: Vec<Option<ReflectVtable>>,
}

impl ReflectRegistry {
    pub(crate) fn new() -> Self {
        Self {
            vtables: Vec::new(),
        }
    }

    /// Records `T`'s vtable at `id` (overwriting any prior entry, idempotent
    /// re-registration rebuilds the same vtable).
    pub(crate) fn insert<T: Reflect>(&mut self, id: ComponentId) {
        let index = id.index();
        if index >= self.vtables.len() {
            self.vtables.resize_with(index + 1, || None);
        }
        self.vtables[index] = Some(ReflectVtable::of::<T>());
    }

    fn get(&self, id: ComponentId) -> Option<&ReflectVtable> {
        self.vtables.get(id.index()).and_then(Option::as_ref)
    }

    /// Whether `id` has a reflect vtable.
    pub(crate) fn is_reflected(&self, id: ComponentId) -> bool {
        self.get(id).is_some()
    }

    /// The reflected components present on `entity`, in [`ComponentId`] order.
    /// Cold path (selection-change), so a `Vec` is acceptable.
    pub(crate) fn present_on(&self, world: &World, entity: Entity) -> Vec<ReflectedComponent> {
        let mut out = Vec::new();
        for (index, slot) in self.vtables.iter().enumerate() {
            if let Some(vtable) = slot {
                if (vtable.present)(world, entity) {
                    out.push(ReflectedComponent {
                        component: ComponentId::new(index as u32),
                        type_name: vtable.type_name,
                        fields: vtable.fields,
                    });
                }
            }
        }
        out
    }

    /// Reads a leaf via the stored `get` pointer; `None` if `id` is not
    /// reflected. Allocation-free.
    pub(crate) fn get_field(
        &self,
        world: &World,
        entity: Entity,
        id: ComponentId,
        field: &str,
    ) -> Option<FieldValue> {
        let get = self.get(id)?.get;
        get(world, entity, field)
    }

    /// Resolves the `set` pointer for `id`, or [`ReflectError::ComponentNotReflected`].
    /// The pointer is `Copy`, so the caller invokes it after the registry borrow
    /// ends (allowing `&mut World`). Allocation-free.
    pub(crate) fn set_fn(&self, id: ComponentId) -> ReflectResult<SetFn> {
        match self.get(id) {
            Some(vtable) => Ok(vtable.set),
            None => Err(ReflectError::ComponentNotReflected { component: id }),
        }
    }
}

static TRANSFORM3D_FIELDS: &[FieldDesc] = &[
    FieldDesc {
        name: "translation.x",
        kind: FieldKind::F32,
    },
    FieldDesc {
        name: "translation.y",
        kind: FieldKind::F32,
    },
    FieldDesc {
        name: "translation.z",
        kind: FieldKind::F32,
    },
    FieldDesc {
        name: "rotation.x",
        kind: FieldKind::F32,
    },
    FieldDesc {
        name: "rotation.y",
        kind: FieldKind::F32,
    },
    FieldDesc {
        name: "rotation.z",
        kind: FieldKind::F32,
    },
    FieldDesc {
        name: "rotation.w",
        kind: FieldKind::F32,
    },
    FieldDesc {
        name: "scale.x",
        kind: FieldKind::F32,
    },
    FieldDesc {
        name: "scale.y",
        kind: FieldKind::F32,
    },
    FieldDesc {
        name: "scale.z",
        kind: FieldKind::F32,
    },
];

impl Reflect for spawn_core::Transform3D {
    fn type_name() -> &'static str {
        "Transform3D"
    }

    fn fields() -> &'static [FieldDesc] {
        TRANSFORM3D_FIELDS
    }

    fn reflect_get(&self, field: &str) -> Option<FieldValue> {
        let value = match field {
            "translation.x" => self.translation.x,
            "translation.y" => self.translation.y,
            "translation.z" => self.translation.z,
            "rotation.x" => self.rotation.x,
            "rotation.y" => self.rotation.y,
            "rotation.z" => self.rotation.z,
            "rotation.w" => self.rotation.w,
            "scale.x" => self.scale.x,
            "scale.y" => self.scale.y,
            "scale.z" => self.scale.z,
            _ => return None,
        };
        Some(FieldValue::F32(value))
    }

    fn reflect_set(&mut self, field: &str, value: FieldValue) -> ReflectResult<()> {
        let slot: &mut f32 = match field {
            "translation.x" => &mut self.translation.x,
            "translation.y" => &mut self.translation.y,
            "translation.z" => &mut self.translation.z,
            "rotation.x" => &mut self.rotation.x,
            "rotation.y" => &mut self.rotation.y,
            "rotation.z" => &mut self.rotation.z,
            "rotation.w" => &mut self.rotation.w,
            "scale.x" => &mut self.scale.x,
            "scale.y" => &mut self.scale.y,
            "scale.z" => &mut self.scale.z,
            _ => {
                return Err(ReflectError::UnknownField {
                    context: "reflect: unknown Transform3D field",
                })
            }
        };
        *slot = value.expect_f32()?;
        Ok(())
    }
}

static TRANSFORM2D_FIELDS: &[FieldDesc] = &[
    FieldDesc {
        name: "translation.x",
        kind: FieldKind::F32,
    },
    FieldDesc {
        name: "translation.y",
        kind: FieldKind::F32,
    },
    FieldDesc {
        name: "rotation",
        kind: FieldKind::F32,
    },
    FieldDesc {
        name: "scale.x",
        kind: FieldKind::F32,
    },
    FieldDesc {
        name: "scale.y",
        kind: FieldKind::F32,
    },
];

impl Reflect for spawn_core::Transform2D {
    fn type_name() -> &'static str {
        "Transform2D"
    }

    fn fields() -> &'static [FieldDesc] {
        TRANSFORM2D_FIELDS
    }

    fn reflect_get(&self, field: &str) -> Option<FieldValue> {
        let value = match field {
            "translation.x" => self.translation.x,
            "translation.y" => self.translation.y,
            "rotation" => self.rotation,
            "scale.x" => self.scale.x,
            "scale.y" => self.scale.y,
            _ => return None,
        };
        Some(FieldValue::F32(value))
    }

    fn reflect_set(&mut self, field: &str, value: FieldValue) -> ReflectResult<()> {
        let slot: &mut f32 = match field {
            "translation.x" => &mut self.translation.x,
            "translation.y" => &mut self.translation.y,
            "rotation" => &mut self.rotation,
            "scale.x" => &mut self.scale.x,
            "scale.y" => &mut self.scale.y,
            _ => {
                return Err(ReflectError::UnknownField {
                    context: "reflect: unknown Transform2D field",
                })
            }
        };
        *slot = value.expect_f32()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::World;
    use spawn_core::{Transform2D, Transform3D, Vec3};

    #[test]
    fn field_value_kind_roundtrips() {
        assert_eq!(FieldValue::F32(1.0).kind(), FieldKind::F32);
        assert_eq!(FieldValue::I32(1).kind(), FieldKind::I32);
        assert_eq!(FieldValue::U32(1).kind(), FieldKind::U32);
        assert_eq!(FieldValue::U64(1).kind(), FieldKind::U64);
        assert_eq!(FieldValue::Bool(true).kind(), FieldKind::Bool);
    }

    #[test]
    fn transform3d_fields_are_ten_f32_leaves_in_order() {
        let fields = Transform3D::fields();
        assert_eq!(fields.len(), 10);
        assert!(fields.iter().all(|f| f.kind == FieldKind::F32));
        assert_eq!(fields[0].name, "translation.x");
        assert_eq!(fields[6].name, "rotation.w");
        assert_eq!(fields[9].name, "scale.z");
        assert_eq!(Transform3D::type_name(), "Transform3D");
    }

    #[test]
    fn transform3d_get_set_roundtrip() {
        let mut t = Transform3D::from_translation(Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(t.reflect_get("translation.y"), Some(FieldValue::F32(2.0)));
        t.reflect_set("translation.y", FieldValue::F32(9.0))
            .unwrap();
        assert_eq!(t.reflect_get("translation.y"), Some(FieldValue::F32(9.0)));
        assert_eq!(t.translation.y, 9.0);
    }

    #[test]
    fn transform3d_unknown_field_is_none_and_err() {
        let mut t = Transform3D::IDENTITY;
        assert_eq!(t.reflect_get("nope"), None);
        assert!(matches!(
            t.reflect_set("nope", FieldValue::F32(1.0)),
            Err(ReflectError::UnknownField { .. })
        ));
    }

    #[test]
    fn transform3d_wrong_kind_is_type_mismatch() {
        let mut t = Transform3D::IDENTITY;
        assert!(matches!(
            t.reflect_set("scale.x", FieldValue::Bool(true)),
            Err(ReflectError::TypeMismatch { .. })
        ));
    }

    #[test]
    fn transform2d_fields_and_roundtrip() {
        let fields = Transform2D::fields();
        assert_eq!(fields.len(), 5);
        assert_eq!(fields[2].name, "rotation");
        let mut t = Transform2D::IDENTITY;
        t.reflect_set("rotation", FieldValue::F32(0.5)).unwrap();
        assert_eq!(t.reflect_get("rotation"), Some(FieldValue::F32(0.5)));
        assert_eq!(Transform2D::type_name(), "Transform2D");
    }

    #[test]
    fn register_reflect_is_idempotent_and_is_reflected_tracks_it() {
        let mut world = World::new();
        let a = world.register_reflect::<Transform3D>();
        let b = world.register_reflect::<Transform3D>();
        assert_eq!(a, b);
        assert!(world.is_reflected(a));
        let plain = world.register::<Transform2D>();
        assert!(!world.is_reflected(plain));
    }

    #[test]
    fn reflected_components_enumerates_present_in_id_order() {
        let mut world = World::new();
        let t3 = world.register_reflect::<Transform3D>();
        let t2 = world.register_reflect::<Transform2D>();
        let e = world.spawn();
        world.insert(e, Transform3D::IDENTITY).unwrap();
        world.insert(e, Transform2D::IDENTITY).unwrap();
        let listed: Vec<_> = world
            .reflected_components(e)
            .iter()
            .map(|c| c.component)
            .collect();
        assert_eq!(listed, vec![t3, t2]);
        assert_eq!(world.reflected_components(e)[0].type_name, "Transform3D");
    }

    #[test]
    fn reflected_components_empty_for_absent_and_dead() {
        let mut world = World::new();
        world.register_reflect::<Transform3D>();
        let bare = world.spawn();
        assert!(world.reflected_components(bare).is_empty());
        let e = world.spawn_with((Transform3D::IDENTITY,));
        world.despawn(e).unwrap();
        assert!(world.reflected_components(e).is_empty());
    }

    #[test]
    fn reflect_get_set_field_by_id_roundtrips_through_storage() {
        let mut world = World::new();
        let id = world.register_reflect::<Transform3D>();
        let e = world.spawn_with((Transform3D::from_translation(Vec3::new(1.0, 2.0, 3.0)),));
        assert_eq!(
            world.reflect_get_field(e, id, "translation.z"),
            Some(FieldValue::F32(3.0))
        );
        world
            .reflect_set_field(e, id, "translation.z", FieldValue::F32(7.0))
            .unwrap();
        // The write went through normal storage, so a typed read sees it.
        assert_eq!(world.get::<Transform3D>(e).unwrap().translation.z, 7.0);
    }

    #[test]
    fn reflect_set_field_errors_are_distinct() {
        let mut world = World::new();
        let id = world.register_reflect::<Transform3D>();
        let unreflected = world.register::<Transform2D>();
        let e = world.spawn_with((Transform3D::IDENTITY,));

        assert!(matches!(
            world.reflect_set_field(e, unreflected, "translation.x", FieldValue::F32(1.0)),
            Err(ReflectError::ComponentNotReflected { .. })
        ));

        let bare = world.spawn();
        assert!(matches!(
            world.reflect_set_field(bare, id, "translation.x", FieldValue::F32(1.0)),
            Err(ReflectError::MissingComponent { .. })
        ));
        assert!(matches!(
            world.reflect_set_field(e, id, "nope", FieldValue::F32(1.0)),
            Err(ReflectError::UnknownField { .. })
        ));
        assert!(matches!(
            world.reflect_set_field(e, id, "scale.x", FieldValue::Bool(true)),
            Err(ReflectError::TypeMismatch { .. })
        ));
    }

    #[test]
    fn reflect_get_field_is_none_for_unreflected_or_absent() {
        let mut world = World::new();
        let id = world.register_reflect::<Transform3D>();
        let unreflected = world.register::<Transform2D>();
        let e = world.spawn_with((Transform3D::IDENTITY,));
        assert!(world.reflect_get_field(e, unreflected, "x").is_none());
        let bare = world.spawn();
        assert!(world.reflect_get_field(bare, id, "translation.x").is_none());
    }
}
