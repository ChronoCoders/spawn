//! `Copy` asset references for use in pipeline cache keys.
//!
//! `spawn_asset::Handle<T>` is `Arc`-based (not `Copy`), but
//! [`crate::pipeline::PipelineKey`] must be `Copy + Eq + Hash`. These newtypes
//! wrap the stable [`spawn_asset::AssetId`] (which is `Copy + Eq + Hash`) so a
//! handle can participate in a cache key while keeping the strong
//! `spawn_asset::Handle<T>` (which keeps the asset alive) at the call site.
//!
//! Identity contract: equal ids ⇒ equal handle ⇒ same compiled WGSL module
//! reused from the [`crate::pipeline::ShaderStore`].
//!
//! WGSL extension contract: `.wgsl` source is loaded as
//! `spawn_asset::TextAsset`. Registering a loader for that extension is the
//! caller's responsibility (the built-in `TextLoader` already claims `wgsl`).

use spawn_asset::{AssetId, Handle, TextAsset};

/// `Copy` identity of a compiled shader, derived from the source asset's
/// [`AssetId`]. Equal `ShaderHandle`s denote the same WGSL module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShaderHandle(AssetId);

impl ShaderHandle {
    pub const fn from_id(id: AssetId) -> Self {
        Self(id)
    }

    /// Derives the `Copy` key from a strong WGSL source handle. The caller
    /// retains the `Handle<TextAsset>` to keep the source alive for loading.
    pub fn from_handle(handle: &Handle<TextAsset>) -> Self {
        Self(handle.id())
    }

    pub const fn id(self) -> AssetId {
        self.0
    }
}
