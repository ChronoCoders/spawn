# Spawn

Rust game engine with archetype ECS, wgpu rendering, Rapier physics, and Lua scripting.

**Status:** v0.1.0 — Phase 1 complete. All 14 crates implement their core APIs against written specifications ([docs/specs/](docs/specs/)), with 639 tests across the workspace. The engine integration loop, visual editor, and rollback netcode are Phase 2. Release notes: [docs/releases/v0.1.0.md](docs/releases/v0.1.0.md).

## Workspace

| Crate | Purpose |
|---|---|
| `spawn-core` | Math, primitives, errors. `#[repr(C)]`, f32, right-handed, column-major. Zero deps. |
| `spawn-ecs` | Archetype ECS: queries, deferred commands, conflict-detecting parallel scheduler. |
| `spawn-platform` | Window, run loop, OS event translation (winit, fully wrapped). |
| `spawn-input` | Device state, action mapping, contexts, runtime rebinding (gilrs gamepads). |
| `spawn-render` | wgpu renderer: pipeline cache, validated render graph, forward pass. |
| `spawn-asset` | Async loading, typed handles, day-one hot-reload via Arc-swap. |
| `spawn-audio` | kira mixer: buses, spatial attenuation/panning, null-device fallback. |
| `spawn-physics` | Rapier 2D/3D behind `dim2`/`dim3` features, fixed-step, ECS sync. |
| `spawn-script` | Sandboxed Lua 5.4 (mlua): instruction budgets, memory caps, typed bridge. |
| `spawn-editor` | Headless editor framework: undo/redo command stack, selection, gizmo math. |
| `spawn-net` | UDP transport: byte-precise protocol, salt handshake, reliable channels. |
| `spawn-ui` | Retained flexbox-subset UI: incremental layout, hit testing, draw list. |
| `spawn-debug` | Logging, frame profiler, metrics, overlay data model. Zero deps. |
| `spawn-build` | Asset compiler: manifest, content hashing, incremental, deterministic pack index. |

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
