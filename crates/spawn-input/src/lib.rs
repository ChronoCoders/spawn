#![deny(warnings)]

//! Input layer for the Spawn engine.
//!
//! Per-frame device state (keyboard, mouse, touch, gamepad) is driven by
//! `spawn-platform` events plus a polled `gilrs` gamepad backend; an action
//! layer maps device bindings to user-defined actions with runtime rebinding;
//! and a context stack layers input consumption. The `gilrs` dependency is owned
//! entirely by [`state`] and never appears in the public API.
//!
//! Per-frame contract: [`InputState::begin_frame`] once, then
//! [`InputState::process`] zero or more times, then
//! [`ActionMap::update`](map::ActionMap::update) /
//! [`InputContextStack::update`](context::InputContextStack::update), then
//! queries.

pub mod action;
pub mod binding;
pub mod context;
pub mod error;
pub mod map;
pub mod state;

pub use action::{Action, ActionId};
pub use binding::{AxisDirection, Binding};
pub use context::{ContextId, InputContextStack};
pub use error::{InputError, InputResult};
pub use map::{ActionMap, ActionState};
pub use state::{
    Deadzone, GamepadAxis, GamepadButton, GamepadEvent, GamepadId, Gamepads, InputState, Keyboard,
    Mouse, Touch, TouchId, TouchPhase, TouchPoint,
};
