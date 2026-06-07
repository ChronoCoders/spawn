# spawn-platform

spawn-platform is the operating-system boundary for the engine. It owns window creation and configuration, the application run loop, and the translation of raw OS events into engine-native enums covering keyboard, mouse, and touch input. It exists to keep the rest of the engine free of any direct dependency on a windowing backend: spawn-render obtains a drawable surface, spawn-input receives raw input events, and the runtime drives its frame loop, all without ever touching `winit`. The crate is the single place where platform-specific behavior is contained and normalized.

## Design Decisions

The windowing abstraction lives entirely behind this crate. No `winit` type appears in any public signature, field, return value, or trait bound. The only sanctioned leak across the boundary is `raw-window-handle`: `Window` implements `HasWindowHandle` and `HasDisplayHandle`, which is exactly enough for spawn-render to construct a wgpu surface without itself depending on the windowing backend. This decoupling means the backend can change in a later phase without forcing edits into the render or input crates.

`winit` 0.30 backs the implementation because it is the cross-platform windowing and event-loop crate that wgpu, Bevy, and egui already target, so it tracks the same platform matrix the renderer needs. `raw-window-handle` 0.6 is the companion that carries surface handles across the crate boundary. Both dependencies are fixed; adding another external dependency requires a design amendment rather than an ad-hoc decision.

Every OS event is converted into a crate-owned enum before it reaches downstream code. winit's `WindowEvent` and `DeviceEvent` values never escape. Translation is deliberately total: any physical key with no engine mapping becomes `KeyCode::Unknown` rather than a panic or a dropped event, so the translation path can never fail at runtime.

The run loop is callback-driven, mirroring winit 0.30's `ApplicationHandler` model but presented through the `PlatformApp` trait. A callback model was chosen over an event-iterator pull model to match the underlying platform contract directly and avoid buffering or re-entrancy mismatches on platforms whose loops do not return.

Keyboard mapping is physical and layout-independent, keyed to US-QWERTY physical positions. This suits gameplay binding, where the physical key location matters more than the produced character. Logical-key and text/character input are a separate concern and are not part of this layer.

Presentation is not owned here. `vsync` on the window config is recorded and exposed as an advisory hint for spawn-render to honor when it selects a wgpu present mode; this crate does not select present modes or present frames.

Errors never wrap or re-export winit error types. Each fallible winit operation maps into a `PlatformError` variant carrying a `&'static str` context. Construction allocates nothing, matching spawn-core's error policy and keeping error paths free of heap pressure.

## Architecture

The crate is split by concern, with `lib.rs` carrying `#![deny(warnings)]`, module declarations, and root-level re-exports of every public type so callers can use either `spawn_platform::Window` or the fully qualified module path.

`window` holds window configuration and the live window handle. `WindowConfig` is a builder-style struct (consuming `with_*` methods) covering title, size, min/max bounds, mode, resizability, visibility, and the vsync hint, with a sensible default of a 1280x720 windowed `"Spawn"` window. `WindowMode` distinguishes windowed, borderless-fullscreen, and exclusive-fullscreen presentation. `Window` is an opaque, non-`Clone` handle, constructed only by the run loop and never by downstream crates; it exposes queries (`id`, `size`, `scale_factor`, `vsync_hint`, `mode`) and mutators (`set_title`, `set_mode`, `set_visible`, `request_redraw`, cursor visibility and grab). Fallible operations return `PlatformResult`; the handful winit cannot fail in 0.30 are infallible. `CursorGrab` covers released, confined, and locked cursor states for relative-motion camera control. `WindowId` is an opaque `u64`-backed identifier, `Hash`/`Eq` so other crates can key per-window state.

`app` holds the run loop and application model. `PlatformApp` is the trait downstream code implements: `init`, `event`, `update`, `redraw_requested`, and `exit`, where only `init` and `event` are required and the rest carry empty defaults. Every callback takes `&Window`, so an application can neither move nor store the window beyond the loop's lifetime. `EventLoop` constructs the platform loop (`new`) and runs it to completion (`run`), consuming both itself and the application, creating the configured window, and dropping it when the loop returns. Per-iteration dispatch order is fixed: translated events, then `event` per event, then `update`, then `redraw_requested` only when a redraw is pending; `init` fires once before the first event and `exit` once after the last.

`event` defines the translated event hierarchy. `PlatformEvent` is the top-level enum dividing into window, keyboard, mouse, and touch payloads, all `Copy` with no heap data. Submodules cover keyboard (`KeyboardEvent`, the `KeyCode` enumeration, and the physical-key map), mouse (`MouseEvent`, `MouseButton`, `ScrollDelta`), and touch (`TouchEvent`, `TouchPhase`). `WindowEvent` carries resize, scale-factor change, close request, focus, occlusion, and move. `ButtonState` (pressed/released) is shared by keyboard and mouse buttons. `KeyCode` is `#[non_exhaustive]` so later phases can extend the key set without breaking callers.

`monitor` provides setup-time display enumeration. `MonitorInfo` reports name, physical resolution, virtual-desktop position, scale factor, current refresh rate, and the list of `VideoMode`s. Enumeration entry points (`available_monitors`, `primary_monitor`) live on `EventLoop`. This is the one area permitted to allocate, since it runs at setup rather than per frame.

`error` defines `PlatformError`, a `#[non_exhaustive]` enum of six variants (event-loop creation, window creation, fullscreen, cursor grab, not-supported, OS error), each carrying a `&'static str` context, plus the `PlatformResult<T>` alias. It implements `std::error::Error` and `Display` but is intentionally neither `Clone` nor `PartialEq`.

## Constraints

- Per-event translation allocates nothing. Converting a winit event into a `PlatformEvent` performs zero heap allocations, and the run loop allocates nothing per frame. All event payload types are `Copy` with no `Vec`/`String`/`Box` content. Monitor enumeration is the sole exception and may allocate, as it is a setup-time operation.
- Error construction allocates nothing: contexts are `&'static str`, never owned strings.
- No `unsafe`. If a `raw-window-handle` implementation forces it, the call is delegated to winit's handle accessors, which are safe in 0.30; any unavoidable `unsafe` carries a mandatory `SAFETY` comment.
- No `unwrap`, `expect`, or `panic!` outside test code. Every fallible operation returns `PlatformResult<T>`, and winit errors are mapped into `PlatformError` rather than surfaced.
- The only spawn-crate dependency is spawn-core. No other engine crate is a dependency.
- External dependencies are exactly `winit` 0.30 and `raw-window-handle` 0.6. Additions require a design amendment.
- No `winit` type may appear in any public signature, field, or trait bound, with the single exception of the `raw-window-handle` traits on `Window`. winit error types are neither wrapped nor re-exported.
- Coordinate convention, normative for spawn-input and spawn-render: window and cursor positions are physical pixels, origin top-left, `+x` right, `+y` down. Logical size equals physical size divided by `scale_factor`.
- All public items carry `///` doc comments describing their contract.
- The public API matches this design exactly. No extra public items, no omissions; deviations require amendment and re-approval.

## Phase 1 Scope

In scope: window creation and configuration, the run loop, OS-event translation into the engine event enums, raw keyboard/mouse/touch delivery, monitor enumeration and metadata, surface-handle exposure for spawn-render, and the platform error type. Target platforms are desktop only: Windows, macOS, and Linux.

Out of scope, deferred to later approved work: clipboard, IME and text composition, drag-and-drop, file dialogs, custom cursors and cursor images, window icons, system tray, raw mouse-motion deltas, and multi-window support beyond a single primary window. Gamepad input and input mapping/rebinding belong to spawn-input (gamepad via gilrs) and are not handled here. Mobile and web targets are excluded; on those platforms winit's `run` may never return, and the run loop is built around the desktop contract where it does.

The line sits at the raw OS boundary. This crate produces normalized, layout-independent, allocation-free events and a backend-agnostic window handle; interpreting those events (binding, text, semantic input) and presenting frames (present-mode selection, surface management) are the responsibilities of spawn-input and spawn-render respectively. Keeping presentation and input semantics out preserves a clean single-responsibility boundary and lets each consuming crate evolve independently.
