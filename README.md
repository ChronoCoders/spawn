<p align="center">
  <img src="assets/logo.svg" width="72" height="72" />
</p>

<h1 align="center">Spawn</h1>
<p align="center">A production-grade game engine written in Rust.</p>

---

# Spawn

Rust game engine with archetype ECS, wgpu rendering, Rapier physics, and Lua scripting.

**Status:** v0.2.0, Phase 2 complete. All 18 crates implement their core APIs against written specifications, with 851 tests across the workspace. Phase 2 added the engine integration loop, render-graph derivation with directional lighting and shadows, the visual editor, and server-authoritative netcode. Release notes: [v0.2.0 GitHub release](https://github.com/ChronoCoders/spawn/releases/tag/v0.2.0).

## Workspace

| Crate | Purpose |
|---|---|
| [`spawn-core`](crates/spawn-core/) | Math, primitives, errors. `#[repr(C)]`, f32, right-handed, column-major. Zero deps. |
| [`spawn-ecs`](crates/spawn-ecs/) | Archetype ECS: queries, deferred commands, conflict-detecting parallel scheduler. |
| [`spawn-platform`](crates/spawn-platform/) | Window, run loop, OS event translation (winit, fully wrapped). |
| [`spawn-input`](crates/spawn-input/) | Device state, action mapping, contexts, runtime rebinding (gilrs gamepads). |
| [`spawn-render`](crates/spawn-render/) | wgpu renderer: pipeline cache, validated render graph, forward pass. |
| [`spawn-asset`](crates/spawn-asset/) | Async loading, typed handles, day-one hot-reload via Arc-swap. |
| [`spawn-audio`](crates/spawn-audio/) | kira mixer: buses, spatial attenuation/panning, null-device fallback. |
| [`spawn-physics`](crates/spawn-physics/) | Rapier 2D/3D behind `dim2`/`dim3` features, fixed-step, ECS sync. |
| [`spawn-engine`](crates/spawn-engine/) | Integration loop: frame pipeline, fixed-step accumulator, frontend/backend proxy split with low-latency sync. |
| [`spawn-script`](crates/spawn-script/) | Sandboxed Lua 5.4 (mlua): instruction budgets, memory caps, typed bridge. |
| [`spawn-editor`](crates/spawn-editor/) | Headless editor framework: undo/redo command stack, selection, gizmo math. |
| [`spawn-editor-shell`](crates/spawn-editor-shell/) | Visual editor: scene view, reflection inspector, translate/rotate/scale gizmos, Edit/Play. |
| [`spawn-net`](crates/spawn-net/) | UDP transport: byte-precise protocol, salt handshake, reliable channels. |
| [`spawn-serialize`](crates/spawn-serialize/) | Bit-level serialization codec: read/write-symmetric streams, quantization, smallest-three quaternions. |
| [`spawn-replication`](crates/spawn-replication/) | Server-authoritative netcode: interest management, acked-baseline delta snapshots, interpolation, prediction, RPC. |
| [`spawn-ui`](crates/spawn-ui/) | Retained flexbox-subset UI: incremental layout, hit testing, draw list. |
| [`spawn-debug`](crates/spawn-debug/) | Logging, frame profiler, metrics, overlay data model. Zero deps. |
| [`spawn-build`](crates/spawn-build/) | Asset compiler: manifest, content hashing, incremental, deterministic pack index. |

## Building

```sh
cargo build --workspace
cargo test --workspace
```

Requires stable Rust (edition 2021). On Linux, `cpal` needs ALSA headers (`libasound2-dev` on Debian/Ubuntu). Tests run headless: GPU tests use a fallback adapter or skip, audio tests use the null backend.

The workspace builds under `#![deny(warnings)]`. The commit gate:

```sh
cargo clippy --all-features --all-targets -- -D warnings
cargo fmt --check
cargo test --workspace
cargo deny check
```

## License

Spawn is licensed under the [Business Source License 1.1](LICENSE).

- Free for non-production use (development, testing, evaluation)
- Converts to the Apache License 2.0 on **2029-05-14**
- Commercial licensing: [altug@bytus.io](mailto:altug@bytus.io)
