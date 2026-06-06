#![deny(warnings)]

//! Windowing and OS abstraction for the Spawn engine.
//!
//! spawn-platform owns the windowing abstraction. No `winit` type appears in any
//! public signature; the only permitted bridge is `raw-window-handle`, which
//! [`Window`] implements so spawn-render can build a wgpu surface. All OS events
//! are translated into the engine-owned enums in [`event`]. Window/cursor
//! positions are physical pixels, origin top-left, `+x` right, `+y` down; logical
//! sizes are physical divided by `scale_factor`.

pub mod app;
pub mod error;
pub mod event;
pub mod monitor;
pub mod window;

pub use app::{EventLoop, PlatformApp};
pub use error::{PlatformError, PlatformResult};
pub use event::{
    ButtonState, KeyCode, KeyboardEvent, MouseButton, MouseEvent, PlatformEvent, ScrollDelta,
    TouchEvent, TouchPhase, WindowEvent,
};
pub use monitor::{MonitorInfo, VideoMode};
pub use window::{CursorGrab, Window, WindowConfig, WindowId, WindowMode};
