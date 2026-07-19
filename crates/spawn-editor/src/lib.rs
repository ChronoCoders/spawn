#![deny(warnings)]

//! Headless editor framework of the Spawn engine: the data model and command
//! infrastructure that the visual editor drives. No rendering, window, widget,
//! or input handling lives here, this crate is pure logic over a
//! [`spawn_ecs::World`] it does not own.

pub mod command;
pub mod error;
pub mod gizmo;
pub mod selection;
pub mod stack;
pub mod state;
mod transaction;

pub use command::builtin::{DespawnEntity, SetTransform3D, SpawnEntity};
pub use command::Command;
pub use error::{EditorError, EditorResult};
pub use gizmo::{
    axis_drag_delta, plane_drag_delta, ray_axis_handle_hit, ray_plane_intersection,
    rotation_angle_around_axis, Axis, Ray,
};
pub use selection::{Selection, SelectionChanged};
pub use stack::CommandStack;
pub use state::{EditorMode, EditorState};
