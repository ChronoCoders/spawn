#![deny(warnings)]

//! The Spawn engine integration loop: the `App`/`Engine` aggregate and the frame
//! pipeline that wires the platform run loop, input, asset pump, fixed-step
//! accumulator (with physics as an exclusive hook), the spawn-ecs schedules,
//! render-proxy extraction, the render backend, and the audio pump into one
//! frame. A low-latency sync mode is selectable from day one; render-relevant
//! state crosses the frame boundary only through extracted, backend-owned
//! proxies (no shared lock spans the pipeline).

mod app;
mod asset;
mod audio;
mod config;
mod engine;
mod error;
mod frame;
mod input;
mod observability;
mod render;
mod time;
mod ui;

pub use app::App;
pub use asset::{FrameAssets, ReloadEvents};
pub use audio::{AudioCommand, AudioCommands};
pub use config::EngineConfig;
pub use engine::Engine;
pub use error::{EngineError, EngineResult};
pub use frame::{ScheduleLabel, SyncMode};
pub use input::InputFrame;
pub use render::{
    CameraProxy, HeadlessBackend, RenderBackend, RenderProxies, RenderProxy, RenderReload,
    RenderReport, WgpuBackend,
};
pub use time::Time;
pub use ui::DEFAULT_FONT;

pub use spawn_asset::{AppliedReport, ReloadEvent, ReloadOutcome};
pub use spawn_input::{KeyCode, Keyboard, Mouse, MouseButton};
pub use spawn_platform::WindowConfig;
pub use spawn_render::{
    AdapterInfo, DirectionalLight, Lighting, RenderResources, Renderer, ShadowConfig, SurfaceSize,
};
pub use spawn_ui::{FontId, NodeId, Style, UiTree};
