# spawn-debug

`spawn-debug` is the engine's self-contained instrumentation layer: leveled logging, a per-frame profiler, atomic metrics, and a data model for the eventual debug overlay. It exists because the engine controls its own log and profile hot paths and treats the overhead of those paths as a fixed, measurable cost rather than something delegated to a third-party facade. Everything here is built on `std` plus `spawn-core`, with no other dependencies, so the cost of every disabled log call and every profile scope is known and audit-able down to the instruction.

## Design Decisions

The crate deliberately avoids `log` and `tracing`. A logging facade interposes a global dispatcher, introduces contention on that dispatcher, adds a version-churn surface outside engine control, and makes a disabled log level cost more than a single predictable branch. Owning the logging path instead yields deterministic overhead, sinks the overlay can drain directly without a bridge layer, and freedom from external API drift. This trade is settled, not provisional.

Logging is infallible at the call site. A log macro returns nothing and never panics; a sink that fails to write (file I/O error, for example) increments a per-sink dropped-record counter that rolls up into a global total, and the failure goes no further. `DebugError` appears only from explicit setup paths such as `Logger::init` and `FileSink::open`. Pushing fallibility into setup keeps the steady-state path free of `Result` plumbing and lets call sites stay terse.

Hot paths allocate nothing once warm. A disabled record costs one atomic load and one branch, and the format arguments are not even evaluated when the level is below the active floor. An enabled record carries its message as `core::fmt::Arguments`, so a sink that chooses not to materialize a `String` pays no heap cost beyond what `format_args!` already requires. Scope names and log targets are `&'static str`, eliminating per-record and per-scope string allocation. The single exception is `RingBufferSink`, which materializes an owned `String` per retained record — an opt-in debug convenience, not part of the engine's steady-state path.

Timestamps are monotonic `Duration` values measured from logger init, never wall-clock. Wall-clock formatting carries no value for frame-level instrumentation and is excluded.

Thread-safety is split by subsystem and stated as contract. Logging and metrics are callable from any thread: the `Logger` and all sinks are `Send + Sync` with writes serialized per sink, and metrics are lock-free atomic `u64`. The Phase 1 profiler is main-thread-only by design — `Profiler` is not `Sync`, and `profile_scope!` is valid only on the thread owning the active profiler. Restricting the profiler to one thread keeps the scope-stack machinery free of synchronization on its hottest path; multi-thread aggregation is a named, forward-compatible extension rather than an oversight.

A recurring constraint across the crate is the absence of `unsafe`. The profiler's thread-local current-pointer mechanism, which would naturally reach for a raw pointer, instead routes through a `thread_local!` `Cell` holding an index into thread-local owned storage, keeping the whole path in safe code.

## Architecture

The crate is organized by subsystem, with `lib.rs` carrying the workspace warning denial, module declarations, crate-root re-exports of the public types, and the `#[macro_export]` logging macros. Modules stay public so explicit paths remain available alongside the root re-exports.

`error` defines `DebugError` and the `DebugResult<T>` alias. `DebugError` is `#[non_exhaustive]`, covers already-initialized, not-initialized, invalid-config (with a `&'static str` context), and wrapped `io::Error`, and implements `Error`/`Display`/`From<io::Error>`. It is intentionally not `Clone` or `PartialEq` because it wraps `io::Error`.

`log` holds the logging core: `LogLevel` (a `#[repr(u8)]` enum ordered `Error` through `Trace`, enabled when `level <= active_max`), the borrowed `LogRecord<'a>` carrying `fmt::Arguments`, the `ThreadTag` stable per-thread id, the `LogSink` trait, `LogConfig`, and the process-global `Logger`. `Logger` owns once-only init, the monotonic clock, runtime floor get/set via an atomic, per-target prefix filters resolved allocation-free with longest-prefix-wins, flush, and the dropped-record total. The `log::macros` submodule exports `spawn_error!`/`spawn_warn!`/`spawn_info!`/`spawn_debug!`/`spawn_trace!`, each taking an optional leading `target:` (defaulting to `module_path!()`) followed by `format_args!` tokens; their expansion first consults the compile-time floor and emits nothing for stripped levels, then performs the runtime atomic check, constructing a record only on success. The `log::sinks` submodule provides `StderrSink` (mutex-guarded formatted stderr), `FileSink` with size-based rotation across a configured file count, and `RingBufferSink` retaining the last N records as drainable `OwnedRecord` values.

`profile` holds the frame profiler. `Profiler` drives `begin_frame`/`end_frame`, exposes `frame_index`, `last_report`, `history`, per-scope `scope_stats`, and `clear`, and installs itself as the thread's current profiler so `profile_scope!` can find it. `profile::scope` defines the RAII `ScopeGuard` (neither `Send` nor `Sync`) and `ScopeNode`, the merged hierarchical tree node; same-name siblings under one parent collapse into a single node with summed duration and incremented call count to keep the tree bounded. `profile::report` defines `FrameReport` with `fps`, `flatten`, and top-K `hottest`, plus a `malformed` flag for unbalanced scopes. `profile::stats` defines `RollingStats`, a fixed-size sample ring exposing `avg`/`min`/`max`/`p99`/`count` with no per-sample allocation.

`metrics` provides atomic `Counter` and `Gauge` (the gauge stores `u64` bits and offers saturating signed add), the `MetricsRegistry` with get-or-create `counter`/`gauge` returning stable references, a sorted `snapshot`, and a name lookup, plus a process-global default registry via `global()`.

`overlay` provides `DebugOverlayData` and its supporting `ScopeStat` and `OverlayConfig`. It is data only — no rendering. `assemble` pulls frame time and fps from the profiler's last report, the frame-time graph from history, hottest scopes enriched with rolling stats, filtered metric snapshots, and the recent log tail from a ring buffer sink. `frame_graph_slice` hands a renderer a `&[f32]` to plot without copying.

## Constraints

- No allocation in any steady-state hot path. A disabled log record is one atomic load plus one branch with arguments unevaluated; an enabled record formats through `fmt::Arguments` with no intermediate `String` unless a sink materializes one. A profile scope is two monotonic clock reads plus one slot write. A metric mutation is one atomic operation. The profiler frame path is allocation-free once the scope tree's shape and name set stabilize; a first-seen scope name or a tree deeper or wider than any prior frame allocates (pool node, child-vector growth, stats-map entry). There is no fixed warm-up frame count and no depth or name-count bound — warm means shape-stable.
- The only logging allocation is `RingBufferSink`'s per-record `String`, confined to that opt-in sink. The only overlay allocation is `assemble`, which runs at most once per frame.
- Scope names and log targets are `&'static str`. No per-scope or per-record string allocation anywhere else.
- No `unsafe`. The profiler's current-pointer mechanism uses a `thread_local!` `Cell` over an index into thread-local owned storage rather than raw pointers.
- No `unwrap()`, `expect()`, or `panic!()` outside test code. Fallible operations return `DebugResult` or `Option`. Logging never returns `Result` and never panics; sink write failures increment a dropped-record counter instead of propagating.
- Dependencies are `spawn-core` plus `std` only. No `log`, no `tracing`, no bridge to either, no other external crate. `cargo tree -p spawn-debug` shows exactly std and spawn-core.
- Logging and metrics are callable from any thread; logging serializes per sink and metrics are lock-free. The profiler is main-thread-only — `Profiler` is not `Sync`, and `profile_scope!` is valid only on the profiler-owning thread.
- All public items carry `///` documentation describing their contract.

## Phase 1 Scope

In scope: engine-owned leveled logging — levels, the global logger, the export macros, a compile-time level floor driven by cargo features, the stderr / rotating-file / ring-buffer sinks, and per-target filters; the frame profiler — scoped RAII timers, the hierarchical merged scope tree, frame reports, rolling per-scope statistics, and the history ring; atomic metrics — counters, gauges, and the registry; and the overlay data model, assembled on demand. Unit tests cover every one of these.

The compile-time floor is feature-selected (`max_level_trace` through `max_level_off`), with the highest-verbosity selected feature fixing `const COMPILE_MAX_LEVEL`; macros for levels above the floor expand to nothing and cost zero in the final binary. A public `COMPILE_OFF` flag distinguishes "off" from a floor of `Error` for feature-matrix tests. The `thread_tag()` helper and `Logger::dispatch` are `#[doc(hidden)]` plumbing the macros require, not stable API.

Explicitly deferred, each requiring its own approved design before work begins: overlay rendering itself (it depends on spawn-render and spawn-ui, so Phase 1 ships only the data those systems will later draw); GPU timestamp queries; structured or JSON log formats; network log shipping; async or non-blocking sink I/O; any use of or bridge to `log`/`tracing`; allocator and heap profiling; and flamegraph export. Multi-thread profiling — per-thread scope stacks aggregated on the main thread at `end_frame` — is deferred to a later phase; the current `FrameReport` and `ScopeNode` shapes are forward-compatible with a future `Profiler::aggregate_thread` that merges worker trees without changing them.

The line sits at what the engine needs to instrument and inspect itself using nothing but the standard library and its own core crate. Anything that pulls in a rendering backend, a GPU API surface, an async runtime, or an external logging ecosystem crosses that line and waits for a dedicated design.
