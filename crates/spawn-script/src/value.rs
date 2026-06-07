//! The typed value bridge between Rust and Lua.
//!
//! Conversion is allocation-minimized: scalars and userdata variants convert
//! without heap allocation; only [`ScriptValue::Str`] and [`ScriptValue::Table`]
//! allocate.

use mlua::{Lua, Value as LuaValue};
use spawn_core::{Quat, Vec2, Vec3};
use spawn_ecs::Entity;

use crate::entity_api::{entity_from_lua, LuaEntity};
use crate::error::{ScriptError, ScriptResult};
use crate::math_binding::{LuaQuat, LuaVec2, LuaVec3};

/// A value crossing the Rust/Lua boundary.
///
/// [`ScriptValue::Table`] is a "table-lite": an ordered list of string-keyed
/// entries modelling a flat Lua table with string keys. Non-string keys and
/// cyclic tables are rejected at conversion. Nesting is allowed.
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptValue {
    Nil,
    Bool(bool),
    Int(i64),
    /// Lua numbers are `f64`; converting a Lua float to this variant narrows to
    /// `f32` (documented lossy).
    Float(f32),
    Str(String),
    Vec2(Vec2),
    Vec3(Vec3),
    Quat(Quat),
    Entity(Entity),
    Table(Vec<(String, ScriptValue)>),
}

impl From<bool> for ScriptValue {
    fn from(v: bool) -> Self {
        ScriptValue::Bool(v)
    }
}
impl From<i64> for ScriptValue {
    fn from(v: i64) -> Self {
        ScriptValue::Int(v)
    }
}
impl From<f32> for ScriptValue {
    fn from(v: f32) -> Self {
        ScriptValue::Float(v)
    }
}
impl From<&str> for ScriptValue {
    fn from(v: &str) -> Self {
        ScriptValue::Str(v.to_owned())
    }
}
impl From<String> for ScriptValue {
    fn from(v: String) -> Self {
        ScriptValue::Str(v)
    }
}
impl From<Vec2> for ScriptValue {
    fn from(v: Vec2) -> Self {
        ScriptValue::Vec2(v)
    }
}
impl From<Vec3> for ScriptValue {
    fn from(v: Vec3) -> Self {
        ScriptValue::Vec3(v)
    }
}
impl From<Quat> for ScriptValue {
    fn from(v: Quat) -> Self {
        ScriptValue::Quat(v)
    }
}
impl From<Entity> for ScriptValue {
    fn from(v: Entity) -> Self {
        ScriptValue::Entity(v)
    }
}

const MAX_TABLE_DEPTH: usize = 16;

pub(crate) fn to_lua<'lua>(lua: &'lua Lua, value: &ScriptValue) -> ScriptResult<LuaValue<'lua>> {
    let v = match value {
        ScriptValue::Nil => LuaValue::Nil,
        ScriptValue::Bool(b) => LuaValue::Boolean(*b),
        ScriptValue::Int(i) => LuaValue::Integer(*i),
        ScriptValue::Float(f) => LuaValue::Number(*f as f64),
        ScriptValue::Str(s) => {
            LuaValue::String(lua.create_string(s).map_err(|_| ScriptError::Conversion {
                context: "string -> lua",
            })?)
        }
        ScriptValue::Vec2(v) => {
            LuaValue::UserData(lua.create_userdata(LuaVec2(*v)).map_err(|_| {
                ScriptError::Conversion {
                    context: "vec2 -> lua",
                }
            })?)
        }
        ScriptValue::Vec3(v) => {
            LuaValue::UserData(lua.create_userdata(LuaVec3(*v)).map_err(|_| {
                ScriptError::Conversion {
                    context: "vec3 -> lua",
                }
            })?)
        }
        ScriptValue::Quat(q) => {
            LuaValue::UserData(lua.create_userdata(LuaQuat(*q)).map_err(|_| {
                ScriptError::Conversion {
                    context: "quat -> lua",
                }
            })?)
        }
        ScriptValue::Entity(e) => {
            LuaValue::UserData(lua.create_userdata(LuaEntity(*e)).map_err(|_| {
                ScriptError::Conversion {
                    context: "entity -> lua",
                }
            })?)
        }
        ScriptValue::Table(entries) => {
            let table = lua.create_table().map_err(|_| ScriptError::Conversion {
                context: "table -> lua",
            })?;
            for (k, val) in entries {
                let lv = to_lua(lua, val)?;
                table
                    .set(k.as_str(), lv)
                    .map_err(|_| ScriptError::Conversion {
                        context: "table set -> lua",
                    })?;
            }
            LuaValue::Table(table)
        }
    };
    Ok(v)
}

pub(crate) fn from_lua(value: &LuaValue) -> ScriptResult<ScriptValue> {
    from_lua_depth(value, 0)
}

fn from_lua_depth(value: &LuaValue, depth: usize) -> ScriptResult<ScriptValue> {
    match value {
        LuaValue::Nil => Ok(ScriptValue::Nil),
        LuaValue::Boolean(b) => Ok(ScriptValue::Bool(*b)),
        LuaValue::Integer(i) => Ok(ScriptValue::Int(*i)),
        LuaValue::Number(n) => Ok(ScriptValue::Float(*n as f32)),
        LuaValue::String(s) => {
            let text = s.to_str().map_err(|_| ScriptError::Conversion {
                context: "lua string not utf-8",
            })?;
            Ok(ScriptValue::Str(text.to_owned()))
        }
        LuaValue::UserData(ud) => {
            if let Ok(v) = ud.borrow::<LuaVec2>() {
                Ok(ScriptValue::Vec2(v.0))
            } else if let Ok(v) = ud.borrow::<LuaVec3>() {
                Ok(ScriptValue::Vec3(v.0))
            } else if let Ok(q) = ud.borrow::<LuaQuat>() {
                Ok(ScriptValue::Quat(q.0))
            } else if let Ok(e) = ud.borrow::<LuaEntity>() {
                Ok(ScriptValue::Entity(e.0))
            } else {
                Err(ScriptError::Conversion {
                    context: "unknown userdata -> value",
                })
            }
        }
        LuaValue::Table(table) => {
            if depth >= MAX_TABLE_DEPTH {
                return Err(ScriptError::Conversion {
                    context: "table nesting too deep / cyclic",
                });
            }
            if let Some(entity) = entity_from_lua(table) {
                return Ok(ScriptValue::Entity(entity));
            }
            let mut entries = Vec::new();
            for pair in table.clone().pairs::<LuaValue, LuaValue>() {
                let (k, v) = pair.map_err(|_| ScriptError::Conversion {
                    context: "table iteration",
                })?;
                let key = match k {
                    LuaValue::String(s) => s
                        .to_str()
                        .map_err(|_| ScriptError::Conversion {
                            context: "non-utf8 table key",
                        })?
                        .to_owned(),
                    _ => {
                        return Err(ScriptError::Conversion {
                            context: "non-string table key",
                        })
                    }
                };
                entries.push((key, from_lua_depth(&v, depth + 1)?));
            }
            Ok(ScriptValue::Table(entries))
        }
        _ => Err(ScriptError::Conversion {
            context: "unsupported lua value",
        }),
    }
}
