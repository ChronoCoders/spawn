#![deny(warnings)]

//! Asset build pipeline for the Spawn engine: a hand-parsed line-based manifest,
//! deterministic filesystem discovery with a small glob subset, stable
//! path-derived [`AssetId`]s shared with `spawn-asset`, FNV-1a 64-bit content
//! hashing, an incremental build cache, a Phase 1 identity compile to
//! content-addressed outputs, and a byte-precise `index.spawnpack` pack index.
//!
//! Determinism is normative: given identical source bytes and manifest, two
//! independent builds produce a byte-identical `index.spawnpack` and byte
//! -identical content-addressed outputs.

pub mod cache;
pub mod compile;
pub mod discover;
pub mod error;
pub mod glob;
pub mod hash;
pub mod manifest;
pub mod pack;
pub mod pipeline;

pub use cache::{BuildCache, CacheRecord};
pub use compile::{compile_asset, CompileOutput};
pub use discover::{discover, AssetEntry};
pub use error::{BuildError, BuildResult};
pub use glob::Pattern;
pub use hash::{
    canonical_relative_path, hash_bytes, hash_reader, Fnv1a64, FNV_OFFSET_BASIS_64, FNV_PRIME_64,
    HASH_CHUNK_SIZE,
};
pub use manifest::BuildManifest;
pub use pack::{PackEntry, PackIndex, PACK_FLAG_EXTERNAL, PACK_MAGIC, PACK_VERSION};
pub use pipeline::{BuildConfig, BuildPipeline, BuildReport};

/// Re-exported from `spawn-asset`: `spawn-build` does not define its own asset id
/// type. Ids are the FNV-1a 64-bit hash of the canonical relative path, so an id
/// baked here equals the id `spawn-asset` computes at runtime for the same path.
pub use spawn_asset::AssetId;
