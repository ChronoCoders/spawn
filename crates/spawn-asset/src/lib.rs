#![deny(warnings)]

//! Asset pipeline for the Spawn engine: stable asset identity, typed
//! reference-counted handles, a non-blocking [`AssetServer`] with a dedicated
//! IO thread pool, extension-based loader dispatch, and filesystem hot-reload
//! with in-place handle invalidation.
//!
//! Synchronization convention: all asset state visible to [`AssetServer::get`]
//! changes only at the [`AssetServer::apply_loaded`] sync point, called once per
//! frame on the main thread. Loads and reads between pumps see a stable
//! snapshot.

pub mod error;
pub mod handle;
pub mod id;
pub mod loader;
pub mod server;
pub mod watch;

pub use error::{AssetError, AssetResult};
pub use handle::{Asset, Handle, WeakHandle};
pub use id::AssetId;
pub use loader::{
    register_builtin_loaders, AssetLoader, BinaryAsset, BinaryLoader, LoadContext, TextAsset,
    TextLoader,
};
pub use server::{AppliedReport, AssetServer, AssetServerConfig, LoadState};
pub use watch::{ReloadEvent, ReloadOutcome};
