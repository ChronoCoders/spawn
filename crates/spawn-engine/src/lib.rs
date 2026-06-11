#![deny(warnings)]

//! The Spawn engine integration loop: the `App`/`Engine` aggregate and the frame
//! pipeline that wires the platform run loop, input, asset pump, fixed-step
//! accumulator (with physics as an exclusive hook), the spawn-ecs schedules,
//! render-proxy extraction, the render backend, and the audio pump into one
//! frame. A low-latency sync mode is selectable from day one; render-relevant
//! state crosses the frame boundary only through extracted, backend-owned
//! proxies (no shared lock spans the pipeline).

mod app;
mod audio;
mod config;
mod engine;
mod error;
mod frame;
mod input;
mod render;
mod time;
mod ui;

pub use app::App;
pub use audio::{AudioCommand, AudioCommands};
pub use config::EngineConfig;
pub use engine::Engine;
pub use error::{EngineError, EngineResult};
pub use frame::{ScheduleLabel, SyncMode};
pub use input::InputFrame;
pub use render::{
    CameraProxy, HeadlessBackend, RenderBackend, RenderProxies, RenderProxy, WgpuBackend,
};
pub use time::Time;
pub use ui::DEFAULT_FONT;

pub use spawn_input::{KeyCode, Keyboard, Mouse, MouseButton};
pub use spawn_platform::WindowConfig;
pub use spawn_render::{
    DirectionalLight, Lighting, RenderResources, Renderer, ShadowConfig, SurfaceSize,
};
pub use spawn_ui::{FontId, NodeId, Style, UiTree};
