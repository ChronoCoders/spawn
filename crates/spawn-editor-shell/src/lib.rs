#![deny(warnings)]

//! The Spawn visual editor shell.
//!
//! A bespoke immediate-mode windowed editor that composes the engine's
//! foundations: a four-region docked layout (toolbar / outliner / viewport /
//! inspector) over one retained [`spawn_ui`] tree, the live ECS world rendered
//! through the 2b lit graph with the 2c `Overlay2D` pass compositing the UI,
//! gizmos, and selection on top, a reflection-driven inspector, translate/rotate/
//! scale gizmos over the Phase 1 gizmo math (every drag one transaction), and an
//! Edit/Play toggle over the Phase 1 snapshot contract. See
//! `docs/specs/phase-02-spawn-editor-shell.md`.

mod app;
pub mod bridge;
pub mod camera;
pub mod error;
pub mod gizmo;
pub mod inspector;
pub mod outliner;
pub mod overlay;
pub mod panels;
pub mod picking;
pub mod playmode;
pub mod scene;
pub mod status;
pub mod theme;
pub mod toolbar;
mod util;

pub use app::{EditorApp, EditorConfig, PlayStep, RenderSetup, WorldSetup};
pub use camera::EditorCamera;
pub use error::{ShellError, ShellResult};
pub use gizmo::{GizmoController, GizmoMode};
pub use scene::Renderable;
pub use theme::Theme;
