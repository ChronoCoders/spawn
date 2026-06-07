#![deny(warnings)]

//! Sandboxed Lua 5.4 scripting runtime for the Spawn engine.
//!
//! Provides a [`ScriptEngine`] owning a single sandboxed Lua VM, the [`Script`]
//! ECS component and [`script_runner_system`], a typed [`ScriptValue`] bridge,
//! math userdata (`Vec2`/`Vec3`/`Quat`) with arithmetic metamethods, and a
//! Phase 1 entity API limited to `Transform3D` access. No `mlua` type appears in
//! the public surface; `mlua` is an internal implementation detail.

mod component;
mod config;
mod engine;
mod entity_api;
mod error;
mod math_binding;
mod sandbox;
mod system;
mod value;

pub use component::{Script, ScriptLifecycle, ScriptState};
pub use config::{ScriptConfig, ScriptId};
pub use engine::{ScriptContext, ScriptEngine, ScriptStatus};
pub use error::{ScriptError, ScriptResult};
pub use system::script_runner_system;
pub use value::ScriptValue;
