# spawn-script

`spawn-script` is the engine's gameplay scripting runtime. It embeds a sandboxed Lua 5.4 virtual machine and gives gameplay code a way to attach named scripts to entities, drive them through an `on_init`/`on_update`/`on_destroy` lifecycle, and move typed data across the Rust/Lua boundary without ever exposing the raw Lua VM or letting a misbehaving script touch the host. It exists so designers and gameplay programmers can iterate on behavior at runtime (including hot-reload) while the rest of the engine stays insulated from untrusted code: a faulting script is isolated, budgeted, and never crashes the process.

## Design Decisions

Lua 5.4 via `mlua` was chosen as the only scripting language for Phase 1. `mlua` is pulled in with the `vendored` feature so Lua is statically compiled from source — no build host or target needs a system Lua install, which keeps builds reproducible per the workspace mandate. The `lua54` feature pins the engine-standard language version and gives access to Lua's distinct integer subtype, which the value bridge relies on.

`mlua` is treated strictly as an internal implementation detail. No `mlua` type appears in any public signature; the public surface is built entirely from `spawn-core`/`spawn-ecs` types plus this crate's own types. This keeps the scripting backend swappable in principle and prevents `mlua`'s error and value types from leaking into callers, who would otherwise have to depend on `mlua` transitively.

The sandbox is mandatory and has no opt-out. There is deliberately no API to obtain an unrestricted Lua environment. A loaded script runs against a per-script globals table populated from a whitelist; the dangerous standard libraries and primitives (`os`, `io`, `package`, `require`, `debug`, `load`/`loadstring`/`dofile`/`loadfile`, `collectgarbage`, the raw table accessors, `string.dump`, `coroutine`) are simply not reachable. Offering an "escape hatch" was rejected because untrusted gameplay scripts must never reach the filesystem, process, or host globals — a single optional unsafe path would defeat the guarantee.

Script failures never panic the engine. Every load and call path returns `ScriptResult<T>`; any `mlua::Error` crossing an internal boundary is classified into exactly one `ScriptError` variant. A script that faults is marked `Failed` and skipped on subsequent passes, and one failing script can never abort the runner loop or corrupt another script's state. There is no `unwrap`/`expect`/`panic!` outside tests, and no `unsafe` anywhere.

Resource exhaustion is bounded by two independent mechanisms. A VM instruction-count hook fires on a coarse fixed interval and aborts a call that exceeds its per-call `instruction_budget`, so a tight infinite loop terminates with `BudgetExceeded` rather than hanging the main thread. Memory is capped with `mlua`'s native `set_memory_limit`, so an over-allocating script returns `MemoryExceeded` instead of OOM-killing the process. Both limits avoid per-instruction or per-allocation Rust-side bookkeeping, keeping the hot path cheap.

Per-frame dispatch is built around cached function references. Each script's lifecycle functions are resolved once at load (and re-resolved on reload) and cached, so `call_update` never does a string lookup into the globals table per frame. Likewise the bridge keeps the per-frame argument path allocation-free.

The lifecycle `entity` handle is passed into scripts as a Lua **table** rather than as userdata. `mlua`'s safe `Lua::scope` cannot attach per-instance scoped methods to userdata because `Scope::create_userdata` requires `'static`, and the entity API methods are closures over a transient `&mut World` borrow. A table wrapper carrying scope-bound method closures, with the opaque `Entity` userdata stored in a private metatable slot for bridge identity, is the only shape that stays 100% safe. Standalone `Entity` values that cross through `ScriptValue::Entity` remain plain opaque userdata.

## Architecture

The crate root re-exports the public types (`ScriptEngine`, `ScriptConfig`, `ScriptId`, `ScriptValue`, `Script`, `ScriptError`, `ScriptResult`, `script_runner_system`) and carries `#![deny(warnings)]`. `mlua` is never re-exported.

- **`config`** — `ScriptConfig` (instruction and memory budgets, with a non-trivial `Default` of one million instructions and 64 MiB) and `ScriptId`, an opaque monotonic handle generated per engine instance and never reused, even across unload.

- **`engine`** — `ScriptEngine`, the owner of exactly one Lua VM for its lifetime. It is `!Send + !Sync` and not `Clone`; it lives on the main/ECS-schedule thread. It exposes construction (`new`/`config`), loading and lifetime management (`load_script`, `unload`, `reload`, `is_loaded`, `status`), lifecycle invocation (`call_init`/`call_update`/`call_destroy`), and native binding registration (`register_fn`). `reload` swaps a script's environment and cached refs in place so the `ScriptId` and every `Script` component referencing it stay valid; a failed reload leaves the old script running. This module also defines `ScriptStatus` (`Loaded`/`Failed`) and `ScriptContext`, the controlled binding layer that carries a `&mut World` for the duration of one call and is the only path through which a native binding reaches the ECS world. In Phase 1 `ScriptContext` exposes just `get_transform`/`set_transform`.

- **`sandbox`** — internal. Installs the whitelist into each per-script environment, redacts the denied globals and `string.dump`, reroutes `print` to engine logging, and installs the instruction-count hook. No public API beyond what the engine installs.

- **`value`** — `ScriptValue`, the typed bridge enum, plus the Rust⇄Lua conversion rules and ergonomic `From` impls. `ScriptValue::Table` is a deliberately limited "table-lite": an ordered list of string-keyed entries, nestable, with non-string keys and cycles rejected at conversion time.

- **`math_binding`** — internal registration of `Vec2`/`Vec3`/`Quat` as Lua userdata, with constructor globals, field index/newindex metamethods, arithmetic metamethods, and methods that all delegate to `spawn-core`.

- **`entity_api`** — the Transform3D accessors exposed as methods on the lifecycle `entity` table, backed by `ScriptContext`.

- **`component`** — the `Script` ECS component (`script: ScriptId`, `state: ScriptState`), `ScriptState` (an opaque per-entity lifecycle flag plus state-table handle), and `ScriptLifecycle` (`Pending`/`Active`/`Failed`).

- **`system`** — `script_runner_system(world, engine, dt)`, the once-per-frame schedulable system that drives lifecycle transitions and threads the world borrow into bindings.

- **`error`** — `ScriptError` (a `#[non_exhaustive]` enum with `Init`, `Load`, `Runtime`, `BudgetExceeded`, `MemoryExceeded`, `Conversion`, `UnknownScript`, `ScriptFailed`, `InvalidArgument`) and the `ScriptResult<T>` alias. `ScriptError` implements `Error`/`Display`, deliberately omits `Clone`/`PartialEq` because it carries owned diagnostics, and converts into `spawn_core::SpawnError`.

The runner system performs three passes per frame: `Pending` scripts get `call_init` and advance to `Active` or `Failed`; `Active` scripts get `call_update`; scripts removed or destroyed this frame that were `Active` get exactly one `call_destroy`. A per-entity error marks only that script `Failed` and the loop continues.

## Constraints

- **Allocation.** Scalar and userdata `ScriptValue` variants convert across the boundary without heap allocation; only `Str` and `Table` allocate. The per-frame `call_update` path (an `Entity` plus an `f32`, returning `Nil`/scalar/userdata) is allocation-free. Math metamethods return new userdata and must not introduce hidden per-call heap traffic on the hot path. No per-allocation or per-instruction Rust-side bookkeeping — both budgets are enforced by `mlua`-native mechanisms.
- **Safety.** No `unsafe`. No `unwrap`/`expect`/`panic!` outside `#[cfg(test)]` code. A native binding closure registered via `register_fn` must not panic; a panic there is a host bug, not a script error. Script-originated errors are always caught and converted to `ScriptError`.
- **Dependencies.** May depend only on `spawn-core`, `spawn-ecs`, and `mlua` (`lua54`, `vendored`). No `mlua` type may appear in any `pub` item, and `mlua` is never re-exported. `vendored` is mandatory for reproducible, system-Lua-free builds.
- **Sandbox invariant.** Every denied global resolves to `nil` inside a script; the script's global environment is its own per-script table and can never reach the real Lua globals. The instruction hook guarantees termination of unbounded loops; the budget is enforced to within one hook interval.
- **Determinism.** Lua table iteration order and float formatting are not guaranteed stable. Scripts are gameplay logic and are not part of any lockstep-deterministic contract in Phase 1.
- **Float boundary.** `f32` is the engine-facing float type; Lua floats are `f64` internally. The bridge narrows `f64`→`f32` at the boundary, documented as lossy for `Float`. Lua's integer subtype maps to `Int(i64)`.
- All public items carry `///` documentation treated as API contract.

## Phase 1 Scope

In scope: the sandboxed Lua 5.4 runtime; load/unload/hot-reload of named scripts; per-call instruction budgets and per-VM memory limits; the `on_init`/`on_update`/`on_destroy` lifecycle; the `Script` component and the runner system; the `ScriptValue` typed bridge; native function registration through `ScriptContext`; `Vec2`/`Vec3`/`Quat` math userdata with arithmetic metamethods and `spawn-core`-backed methods; an entity API restricted to reading and writing `Transform3D` fields; non-panicking `ScriptError`/`ScriptResult` handling; and full unit/integration test coverage of the above.

Explicitly deferred to Phase 2 (each needing its own approved design): general component reflection — arbitrary `get_component`/`set_component`, adding or removing components, spawning or destroying entities from scripts, and world queries. The `ScriptContext` boundary is the single planned extension point for that work, which is why the entire ECS surface for scripts funnels through it even when Phase 1 only needs two methods.

Also out of scope (no fixed phase): coroutines and async scripts; loading compiled Lua bytecode; per-script (rather than per-VM) memory accounting; multi-threaded or parallel script execution; serialization of per-entity `state`; debugging/REPL/profiler hooks beyond the budget hook; and any non-Lua scripting language.

The line was drawn at "attach behavior to an entity and safely mutate its transform." That covers the smallest end-to-end gameplay loop — load, lifecycle, sandbox, budgets, value bridge, transform access — without committing to a full reflection API whose surface and safety model warrant their own design pass. Keeping reflection out also keeps the `ScriptContext` borrow model minimal and easy to prove safe for the first release.
