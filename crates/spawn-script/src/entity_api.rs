//! The `entity` value and its Phase 1 `Transform3D` accessors.
//!
//! `Entity` reaches scripts as an opaque [`LuaEntity`] userdata for the binding
//! boundary (so [`ScriptValue::Entity`](crate::ScriptValue) round-trips), and as
//! the `entity` argument to lifecycle functions as a table whose metatable
//! `__index` holds Transform3D accessor closures. Those closures are created
//! inside an [`mlua::Scope`] per lifecycle call, so they safely borrow the ECS
//! `World` for exactly that call without `unsafe` or any `'static` escape.

use std::cell::RefCell;

use mlua::{FromLua, Lua, Result as LuaResult, Scope, Table, UserData, Value};
use spawn_core::Transform3D;
use spawn_ecs::{Entity, World};

use crate::math_binding::{LuaQuat, LuaVec3};

/// Private metatable key under which the lifecycle `entity` table stores its
/// opaque [`LuaEntity`] handle, used to recognize an entity passed back across
/// the value bridge.
const HANDLE_KEY: &str = "__spawn_entity";

/// Opaque entity userdata. Carries no script-visible fields in Phase 1; entity
/// state is reached only through the Transform3D accessor methods.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LuaEntity(pub Entity);

impl UserData for LuaEntity {}

impl<'lua> FromLua<'lua> for LuaEntity {
    fn from_lua(value: Value<'lua>, _lua: &'lua Lua) -> LuaResult<Self> {
        match value {
            Value::UserData(ud) => ud.borrow::<LuaEntity>().map(|b| *b),
            other => Err(mlua::Error::FromLuaConversionError {
                from: other.type_name(),
                to: "Entity",
                message: None,
            }),
        }
    }
}

/// Recognizes the lifecycle `entity` table by its embedded handle. Returns the
/// `Entity` if `table` is such a value, else `None`.
pub(crate) fn entity_from_lua(table: &Table) -> Option<Entity> {
    let mt = table.get_metatable()?;
    let handle: Value = mt.get(HANDLE_KEY).ok()?;
    if let Value::UserData(ud) = handle {
        ud.borrow::<LuaEntity>().ok().map(|e| e.0)
    } else {
        None
    }
}

/// The world borrow shared with all scoped entity/binding closures during one
/// lifecycle call.
pub(crate) struct WorldCell<'w> {
    world: RefCell<&'w mut World>,
}

impl<'w> WorldCell<'w> {
    pub(crate) fn new(world: &'w mut World) -> Self {
        Self {
            world: RefCell::new(world),
        }
    }

    pub(crate) fn get_transform(&self, entity: Entity) -> Option<Transform3D> {
        self.world.borrow().get::<Transform3D>(entity).copied()
    }

    /// Writes `t` back onto `entity`'s `Transform3D` through the world command
    /// buffer (applied immediately). `false` if the entity lacks the component.
    pub(crate) fn set_transform(&self, entity: Entity, t: Transform3D) -> bool {
        let mut world = self.world.borrow_mut();
        if world.get::<Transform3D>(entity).is_none() {
            return false;
        }
        world.commands().insert(entity, t);
        world.apply_commands();
        true
    }
}

fn transform_to_table(table: &Table, t: Transform3D) -> LuaResult<()> {
    table.set("position", LuaVec3(t.translation))?;
    table.set("rotation", LuaQuat(t.rotation))?;
    table.set("scale", LuaVec3(t.scale))?;
    Ok(())
}

fn table_into_transform(table: &Table, mut base: Transform3D) -> LuaResult<Transform3D> {
    if let Value::UserData(ud) = table.get::<_, Value>("position")? {
        base.translation = ud.borrow::<LuaVec3>()?.0;
    }
    if let Value::UserData(ud) = table.get::<_, Value>("rotation")? {
        base.rotation = ud.borrow::<LuaQuat>()?.0;
    }
    if let Value::UserData(ud) = table.get::<_, Value>("scale")? {
        base.scale = ud.borrow::<LuaVec3>()?.0;
    }
    Ok(base)
}

/// Builds the per-call `entity` value: a table whose metatable `__index` holds
/// scoped Transform3D accessor closures, plus the opaque [`LuaEntity`] handle
/// for cross-bridge identity. When `cell` is `None` (the worldless `call_*`
/// convenience path) the accessors are graceful no-ops returning `nil`.
pub(crate) fn make_entity<'lua, 'scope, 'w>(
    lua: &'lua Lua,
    scope: &Scope<'lua, 'scope>,
    cell: Option<&'scope WorldCell<'w>>,
    entity: Entity,
) -> LuaResult<Value<'lua>>
where
    'w: 'scope,
{
    let methods = lua.create_table()?;

    methods.set(
        "get_position",
        scope.create_function(move |_, _this: Value| {
            Ok(cell
                .and_then(|c| c.get_transform(entity))
                .map(|t| LuaVec3(t.translation)))
        })?,
    )?;
    methods.set(
        "set_position",
        scope.create_function(move |_, (_this, p): (Value, LuaVec3)| {
            if let Some(c) = cell {
                if let Some(mut t) = c.get_transform(entity) {
                    t.translation = p.0;
                    c.set_transform(entity, t);
                }
            }
            Ok(())
        })?,
    )?;
    methods.set(
        "get_rotation",
        scope.create_function(move |_, _this: Value| {
            Ok(cell
                .and_then(|c| c.get_transform(entity))
                .map(|t| LuaQuat(t.rotation)))
        })?,
    )?;
    methods.set(
        "set_rotation",
        scope.create_function(move |_, (_this, r): (Value, LuaQuat)| {
            if let Some(c) = cell {
                if let Some(mut t) = c.get_transform(entity) {
                    t.rotation = r.0;
                    c.set_transform(entity, t);
                }
            }
            Ok(())
        })?,
    )?;
    methods.set(
        "get_scale",
        scope.create_function(move |_, _this: Value| {
            Ok(cell
                .and_then(|c| c.get_transform(entity))
                .map(|t| LuaVec3(t.scale)))
        })?,
    )?;
    methods.set(
        "set_scale",
        scope.create_function(move |_, (_this, s): (Value, LuaVec3)| {
            if let Some(c) = cell {
                if let Some(mut t) = c.get_transform(entity) {
                    t.scale = s.0;
                    c.set_transform(entity, t);
                }
            }
            Ok(())
        })?,
    )?;
    methods.set(
        "get_transform",
        scope.create_function(move |lua, _this: Value| {
            match cell.and_then(|c| c.get_transform(entity)) {
                Some(t) => {
                    let table = lua.create_table()?;
                    transform_to_table(&table, t)?;
                    Ok(Value::Table(table))
                }
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    methods.set(
        "set_transform",
        scope.create_function(move |_, (_this, table): (Value, Table)| {
            if let Some(c) = cell {
                if let Some(base) = c.get_transform(entity) {
                    let merged = table_into_transform(&table, base)?;
                    c.set_transform(entity, merged);
                }
            }
            Ok(())
        })?,
    )?;

    let metatable = lua.create_table()?;
    metatable.set("__index", methods)?;
    metatable.set(HANDLE_KEY, lua.create_userdata(LuaEntity(entity))?)?;

    let ud = lua.create_table()?;
    ud.set_metatable(Some(metatable));
    Ok(Value::Table(ud))
}
