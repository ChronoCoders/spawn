//! Userdata wrappers for `Vec2`/`Vec3`/`Quat` with arithmetic metamethods and
//! constructor globals, all delegating to `spawn-core`.

use mlua::{
    FromLua, Lua, MetaMethod, Result as LuaResult, Table, UserData, UserDataFields,
    UserDataMethods, Value, Variadic,
};
use spawn_core::{Quat, Vec2, Vec3};

/// Userdata newtype for [`spawn_core::Vec2`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct LuaVec2(pub Vec2);

/// Userdata newtype for [`spawn_core::Vec3`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct LuaVec3(pub Vec3);

/// Userdata newtype for [`spawn_core::Quat`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct LuaQuat(pub Quat);

macro_rules! impl_from_lua_userdata {
    ($ty:ty, $name:literal) => {
        impl<'lua> FromLua<'lua> for $ty {
            fn from_lua(value: Value<'lua>, _lua: &'lua Lua) -> LuaResult<Self> {
                match value {
                    Value::UserData(ud) => ud.borrow::<$ty>().map(|b| *b),
                    other => Err(mlua::Error::FromLuaConversionError {
                        from: other.type_name(),
                        to: $name,
                        message: None,
                    }),
                }
            }
        }
    };
}

impl_from_lua_userdata!(LuaVec2, "Vec2");
impl_from_lua_userdata!(LuaVec3, "Vec3");
impl_from_lua_userdata!(LuaQuat, "Quat");

fn num(value: &Value) -> Option<f32> {
    match value {
        Value::Integer(i) => Some(*i as f32),
        Value::Number(n) => Some(*n as f32),
        _ => None,
    }
}

impl UserData for LuaVec2 {
    fn add_fields<'lua, F: UserDataFields<'lua, Self>>(fields: &mut F) {
        fields.add_field_method_get("x", |_, this| Ok(this.0.x));
        fields.add_field_method_get("y", |_, this| Ok(this.0.y));
        fields.add_field_method_set("x", |_, this, v: f32| {
            this.0.x = v;
            Ok(())
        });
        fields.add_field_method_set("y", |_, this, v: f32| {
            this.0.y = v;
            Ok(())
        });
    }

    fn add_methods<'lua, M: UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method("dot", |_, this, other: LuaVec2| Ok(this.0.dot(other.0)));
        methods.add_method("length", |_, this, ()| Ok(this.0.length()));
        methods.add_method("length_squared", |_, this, ()| Ok(this.0.length_squared()));
        methods.add_method("normalize", |_, this, ()| {
            Ok(this.0.normalize().map(LuaVec2))
        });
        methods.add_method("lerp", |_, this, (rhs, t): (LuaVec2, f32)| {
            Ok(LuaVec2(this.0.lerp(rhs.0, t)))
        });

        methods.add_meta_method(MetaMethod::Add, |_, this, rhs: LuaVec2| {
            Ok(LuaVec2(this.0 + rhs.0))
        });
        methods.add_meta_method(MetaMethod::Sub, |_, this, rhs: LuaVec2| {
            Ok(LuaVec2(this.0 - rhs.0))
        });
        methods.add_meta_method(MetaMethod::Mul, |_, this, rhs: Value| {
            num(&rhs)
                .map(|s| LuaVec2(this.0 * s))
                .ok_or_else(|| mlua::Error::runtime("Vec2 * expects a number"))
        });
        methods.add_meta_method(MetaMethod::Div, |_, this, rhs: f32| {
            Ok(LuaVec2(this.0 / rhs))
        });
        methods.add_meta_method(MetaMethod::Unm, |_, this, ()| Ok(LuaVec2(-this.0)));
        methods.add_meta_method(MetaMethod::Eq, |_, this, rhs: LuaVec2| Ok(this.0 == rhs.0));
        methods.add_meta_method(MetaMethod::Len, |_, this, ()| Ok(this.0.length()));
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| {
            Ok(format!("Vec2({}, {})", this.0.x, this.0.y))
        });
    }
}

impl UserData for LuaVec3 {
    fn add_fields<'lua, F: UserDataFields<'lua, Self>>(fields: &mut F) {
        fields.add_field_method_get("x", |_, this| Ok(this.0.x));
        fields.add_field_method_get("y", |_, this| Ok(this.0.y));
        fields.add_field_method_get("z", |_, this| Ok(this.0.z));
        fields.add_field_method_set("x", |_, this, v: f32| {
            this.0.x = v;
            Ok(())
        });
        fields.add_field_method_set("y", |_, this, v: f32| {
            this.0.y = v;
            Ok(())
        });
        fields.add_field_method_set("z", |_, this, v: f32| {
            this.0.z = v;
            Ok(())
        });
    }

    fn add_methods<'lua, M: UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method("dot", |_, this, other: LuaVec3| Ok(this.0.dot(other.0)));
        methods.add_method("cross", |_, this, other: LuaVec3| {
            Ok(LuaVec3(this.0.cross(other.0)))
        });
        methods.add_method("length", |_, this, ()| Ok(this.0.length()));
        methods.add_method("length_squared", |_, this, ()| Ok(this.0.length_squared()));
        methods.add_method("normalize", |_, this, ()| {
            Ok(this.0.normalize().map(LuaVec3))
        });
        methods.add_method("lerp", |_, this, (rhs, t): (LuaVec3, f32)| {
            Ok(LuaVec3(this.0.lerp(rhs.0, t)))
        });

        methods.add_meta_method(MetaMethod::Add, |_, this, rhs: LuaVec3| {
            Ok(LuaVec3(this.0 + rhs.0))
        });
        methods.add_meta_method(MetaMethod::Sub, |_, this, rhs: LuaVec3| {
            Ok(LuaVec3(this.0 - rhs.0))
        });
        methods.add_meta_method(MetaMethod::Mul, |_, this, rhs: Value| {
            num(&rhs)
                .map(|s| LuaVec3(this.0 * s))
                .ok_or_else(|| mlua::Error::runtime("Vec3 * expects a number"))
        });
        methods.add_meta_method(MetaMethod::Div, |_, this, rhs: f32| {
            Ok(LuaVec3(this.0 / rhs))
        });
        methods.add_meta_method(MetaMethod::Unm, |_, this, ()| Ok(LuaVec3(-this.0)));
        methods.add_meta_method(MetaMethod::Eq, |_, this, rhs: LuaVec3| Ok(this.0 == rhs.0));
        methods.add_meta_method(MetaMethod::Len, |_, this, ()| Ok(this.0.length()));
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| {
            Ok(format!("Vec3({}, {}, {})", this.0.x, this.0.y, this.0.z))
        });
    }
}

impl UserData for LuaQuat {
    fn add_fields<'lua, F: UserDataFields<'lua, Self>>(fields: &mut F) {
        fields.add_field_method_get("x", |_, this| Ok(this.0.x));
        fields.add_field_method_get("y", |_, this| Ok(this.0.y));
        fields.add_field_method_get("z", |_, this| Ok(this.0.z));
        fields.add_field_method_get("w", |_, this| Ok(this.0.w));
        fields.add_field_method_set("x", |_, this, v: f32| {
            this.0.x = v;
            Ok(())
        });
        fields.add_field_method_set("y", |_, this, v: f32| {
            this.0.y = v;
            Ok(())
        });
        fields.add_field_method_set("z", |_, this, v: f32| {
            this.0.z = v;
            Ok(())
        });
        fields.add_field_method_set("w", |_, this, v: f32| {
            this.0.w = v;
            Ok(())
        });
    }

    fn add_methods<'lua, M: UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method("normalize", |_, this, ()| {
            Ok(this.0.normalize().map(LuaQuat))
        });
        methods.add_method("conjugate", |_, this, ()| Ok(LuaQuat(this.0.conjugate())));
        methods.add_method("inverse", |_, this, ()| Ok(this.0.inverse().map(LuaQuat)));
        methods.add_method("slerp", |_, this, (rhs, t): (LuaQuat, f32)| {
            Ok(LuaQuat(this.0.slerp(rhs.0, t)))
        });
        methods.add_method("rotate", |_, this, v: LuaVec3| {
            Ok(LuaVec3(this.0.rotate(v.0)))
        });

        methods.add_meta_method(MetaMethod::Mul, |lua, this, rhs: Value| match rhs {
            Value::UserData(ud) => {
                if let Ok(q) = ud.borrow::<LuaQuat>() {
                    Ok(Value::UserData(lua.create_userdata(LuaQuat(this.0 * q.0))?))
                } else if let Ok(v) = ud.borrow::<LuaVec3>() {
                    Ok(Value::UserData(lua.create_userdata(LuaVec3(this.0 * v.0))?))
                } else {
                    Err(mlua::Error::runtime("Quat * expects a Quat or Vec3"))
                }
            }
            _ => Err(mlua::Error::runtime("Quat * expects a Quat or Vec3")),
        });
        methods.add_meta_method(MetaMethod::Unm, |_, this, ()| Ok(LuaQuat(-this.0)));
        methods.add_meta_method(MetaMethod::Eq, |_, this, rhs: LuaQuat| Ok(this.0 == rhs.0));
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| {
            Ok(format!(
                "Quat({}, {}, {}, {})",
                this.0.x, this.0.y, this.0.z, this.0.w
            ))
        });
    }
}

/// Registers the `Vec2`/`Vec3`/`Quat` constructor globals into `env`.
pub(crate) fn install(lua: &Lua, env: &Table) -> LuaResult<()> {
    let vec2_ctor = lua.create_function(|_, (x, y): (f32, f32)| Ok(LuaVec2(Vec2::new(x, y))))?;
    let vec3_ctor =
        lua.create_function(|_, (x, y, z): (f32, f32, f32)| Ok(LuaVec3(Vec3::new(x, y, z))))?;
    let quat_ctor = lua.create_table()?;
    // `__call` receives the constructor table as the first argument; the
    // numeric components follow it.
    let quat_call = lua.create_function(
        |_, (_ctor, args): (Value, Variadic<f32>)| -> LuaResult<LuaQuat> {
            let x = args.first().copied().unwrap_or(0.0);
            let y = args.get(1).copied().unwrap_or(0.0);
            let z = args.get(2).copied().unwrap_or(0.0);
            let w = args.get(3).copied().unwrap_or(1.0);
            Ok(LuaQuat(Quat::from_xyzw(x, y, z, w)))
        },
    )?;
    let quat_identity = lua.create_function(|_, ()| Ok(LuaQuat(Quat::IDENTITY)))?;
    quat_ctor.set("identity", quat_identity)?;
    let quat_meta = lua.create_table()?;
    quat_meta.set("__call", quat_call)?;
    quat_ctor.set_metatable(Some(quat_meta));

    env.set("Vec2", vec2_ctor)?;
    env.set("Vec3", vec3_ctor)?;
    env.set("Quat", quat_ctor)?;
    Ok(())
}
