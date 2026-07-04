# spawn-asset

The asset subsystem turns logical file paths into typed, reference-counted, hot-reloadable runtime objects without ever blocking the main thread. It provides stable content-independent identity for assets, an `AssetServer` that dispatches file reads to a background IO thread pool, an `AssetLoader` trait for converting raw bytes into typed payloads, and a filesystem watcher that swaps new data into existing handles in place when files change on disk. It exists so every downstream consumer shares one identity scheme, one loading discipline, and one hot-reload mechanism rather than each inventing its own. These consumers are renderer, audio, scripting, editor, and build. Higher-level crates contribute their own loaders against this trait; the crate itself ships only two trivial built-in loaders and stays free of any format knowledge.

## Design Decisions

**Identity from canonical path, not content.** An `AssetId` is a 64-bit FNV-1a hash of the canonical path string, computed before any byte is read. This keeps identity cheap to derive, stable across runs and machines, and available the instant `load` is called: there is no chicken-and-egg dependency on loading the file first. FNV-1a is pinned exactly rather than using `DefaultHasher` so the value is reproducible across Rust versions and across the build tool; spawn-build can compute matching ids by calling the same public `canonicalize` and hash path rather than mirroring the algorithm and risking drift. The 64-bit space makes collisions astronomically unlikely, but a collision between two distinct canonical paths is a hard registration error (`AssetError::IdCollision`), never a silent overwrite and never a panic.

**Strong, Arc-based handles with deferred unload.** A `Handle<T>` is a strong reference: holding one keeps the asset's slot alive, and the asset becomes eligible for unload only when the last strong handle drops. Weak-by-default was rejected because it makes lifetime implicit and surprising: renderer and audio code want to hold an asset for exactly as long as they reference it, with no separate retention bookkeeping. The actual payload free is deferred to the next `apply_loaded` sync point so an unload can never race a concurrent load landing for the same slot. `WeakHandle<T>` covers the genuine observe-without-owning case (a debug overlay enumerating loaded assets) without distorting the default.

**Non-blocking load, single integration point.** `load` canonicalizes, hashes, deduplicates, possibly enqueues an IO job, and returns immediately: it never blocks and never returns a `Result`. All failures become observable state (`LoadState::Failed` plus a retained error) rather than something the caller must handle inline at the call site. State only ever advances at `apply_loaded`, called once per frame on the main thread. Concentrating every state transition at one defined point gives readers a stable snapshot between pumps and removes the need for `get` to coordinate with concurrent loaders.

**Failures are never papered over with defaults.** A missing, unreadable, or unparseable asset surfaces as `Failed` with a retained `AssetError`, or stays `Loading`/`NotLoaded`. The crate never substitutes a placeholder payload. Any default-on-failure policy belongs to the caller, which alone has the context to decide whether a fallback is acceptable.

**Sharded lock map over a single global lock.** The registry maps `AssetId` to slot through N independent shards, each its own `RwLock<HashMap>`. A single global lock would serialize every read against every load; sharding lets reads on one shard proceed while loads commit on another, and read locks never block other reads. This is what makes `get` cheap enough to call freely from hot paths.

**`notify` for filesystem watching.** Cross-platform file watching is mandated from day one, and the per-OS primitives (inotify, FSEvents, ReadDirectoryChangesW) are error-prone to reimplement and out of scope. `notify` is the one external dependency admitted in this phase; nothing else is.

## Architecture

The crate splits into focused modules, all public for explicit-path access, with the common types re-exported at the crate root from `lib.rs`.

**`id`**: `AssetId` (a `#[repr(transparent)]` u64 with the full set of comparison/hash derives), the pinned FNV-1a hashing, and the public free function `canonicalize` that resolves a raw path against the asset root, unifies separators to `/`, and resolves `.`/`..` lexically. Identity is defined entirely by this canonical form.

**`handle`**: `Handle<T>` and `WeakHandle<T>`, the `Asset` marker trait (`Send + Sync + 'static`), and the private `HandleSlot<T>`. The slot holds the current state, the live payload behind in-place-replaceable storage, the retained error, and a generation counter; it is never exposed. Handle equality, hashing, and `Debug` are by `AssetId` only and never touch the payload. `Clone` on a handle is an `AssetId` copy plus an atomic refcount bump: no payload copy, no allocation.

**`loader`**: the `AssetLoader` trait (an associated `Output: Asset`, an `extensions()` list, and a pure `load(bytes, ctx)` transformation), the read-only `LoadContext` carrying id, canonical path, and extension, and a private erasure shim that lets the server store heterogeneous loaders as trait objects while keeping the public trait typed and ergonomic. The two built-in payloads and loaders live here: `BinaryAsset`/`BinaryLoader` (verbatim byte copy, claims `bin`/`dat`) and `TextAsset`/`TextLoader` (UTF-8 validation, claims the common text and source extensions), alongside `register_builtin_loaders`. Loaders run on IO workers; `load` must not touch the filesystem or block, and reports errors by return value.

**`server`**: the public API and its private machinery. `mod.rs` holds `AssetServer`, `AssetServerConfig`, `LoadState`, `AppliedReport`, and the operations: `new`, `register_loader`, `load`, `get`, `load_state`, `error`, `reborrow_or_load`, `apply_loaded`, and the reload-event drain. `pool.rs` is the fixed IO thread pool with its bounded job and completion channels. `registry.rs` is the sharded lock map. Construction spawns the pool and the optional watcher; `Drop` joins every thread and stops the watcher with nothing left detached.

**`watch`**: the `notify` recursive watcher, per-path debounce, and the `ReloadEvent`/`ReloadOutcome` types. It feeds a debounced reload channel that `apply_loaded` drains.

**`error`**: the `#[non_exhaustive]`, `Clone` `AssetError` enum and the `AssetResult<T>` alias, with `Display`, `std::error::Error`, and bidirectional conversion to and from `spawn_core::SpawnError`.

The high-level shape: construct a server from config, register loaders, call `load` to get handles, pump `apply_loaded` each frame, and read through `get`/`load_state`/`error`. Hot-reload owners additionally drain reload events to react to in-place replacements.

## Constraints

- **Allocation.** The `get` hit path performs zero heap allocation: hash, shard index, read-lock, atomic state check, and a reference return borrowed through the read guard: no `Box`, `Vec`, `String`, or payload clone. `load` allocates only when creating a genuinely new slot and job; deduplicated loads allocate nothing beyond an atomic increment. Handle clone allocates nothing beyond the refcount.
- **Safety.** No `unsafe` anywhere. No `unwrap`, `expect`, or `panic!` in non-test code: every fallible operation returns `AssetResult<T>` or an explicit pending/absent state (`Option`, `LoadState`). Test code may use `unwrap`/`assert`.
- **Dependencies.** Among spawn crates, only `spawn-core`. Among external crates, only `notify`. Nothing else appears in `Cargo.toml`. In particular: no async runtime, no serde, no image/mesh/audio/shader codecs.
- **Thread-safety invariants.** `AssetServer`, `Handle<T>`, `AssetId`, and every payload type are `Send + Sync`; loaded payloads are `Send + Sync + 'static`. All slot mutation happens on the main thread inside `apply_loaded`; workers touch no shared mutable asset state. State and generation are atomics so a reader learns the state without a payload lock; payload publication occurs under the slot's own interior `RwLock`, reached while holding only a shard read lock, which supplies the happens-before edge to subsequent reads. Shard write locks are taken solely for map insertion and removal.
- **Identity invariant.** Every path entering the system is canonicalized against the asset root before hashing; two equivalent non-canonical paths resolve to one `AssetId` and one slot.
- **No silent defaults.** A failed or absent asset never becomes a placeholder payload from within this crate.
- **Hot-reload is always on the table.** It is mandatory from day one and active whenever enabled in config, not a bolt-on.

## Phase 1 Scope

In scope: stable asset identity, typed strong/weak handles, the non-blocking `AssetServer` with its single `apply_loaded` sync point, the `AssetLoader` trait with case-insensitive extension dispatch, the background IO pool with a bounded queue and main-thread integration pump, filesystem hot-reload with per-path debounce and in-place handle invalidation, exactly the two built-in loaders, the `AssetError`/`AssetResult` layer, and full unit and integration test coverage of all of it.

Explicitly deferred, each gated behind its own future approval:

- **Streaming, partial loads, and LOD**: the loading model here is all-or-nothing per asset.
- **Inter-asset dependency graphs**: a loader cannot request sub-assets; `LoadContext` is read-only identity and path, and a later phase extends it for this.
- **Compiled/baked asset packs**: that is spawn-build's territory.
- **Virtual filesystem and archive mounting**, **network or remote sources**, **async runtimes**, **serde deserialization helpers**, **reference-counted GPU residency**, and **metadata sidecar files**.
- **Image, mesh, audio, and shader loaders**: those types and their `AssetLoader` impls ship with spawn-render and spawn-audio, registering against this same trait. Format knowledge deliberately stays out of this crate.

The line sits exactly at a correct, non-blocking, hot-reloadable loading core with stable identity. Everything deferred either belongs to a different crate's domain or layers cleanly on top of this foundation once it is proven, so pulling it forward would only couple unrelated concerns and widen the surface before the base is settled.
