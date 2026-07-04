# spawn-input

`spawn-input` turns the raw platform event stream into queryable per-frame input state and a high-level action-mapping layer. It tracks keyboard, mouse, touch, and gamepad devices, exposes edge-accurate press/release queries, and lets game and editor code bind abstract actions to concrete inputs with runtime rebinding and layered consumption. The crate exists to give downstream consumers (editor, UI, game layer) a stable, allocation-free input surface that never leaks the underlying winit or gilrs types.

## Design Decisions

Device state and action mapping are separate layers. `InputState` holds nothing but raw device facts; `ActionMap` and the context stack sit on top and read from it. This keeps the low-level layer reusable for code that wants raw input and keeps action semantics out of the hot device-update path.

Edge state (`just_pressed` / `just_released`) is computed from two bits per input, current held and held-last-frame, rather than from an event log. `begin_frame` copies held into held-last-frame; the edge is purely a comparison of those bits at query time. This makes OS key auto-repeat a non-issue (a repeat event finds the key already held, so no spurious edge fires) and keeps every query O(1) with no buffering.

A single per-frame entry point drives everything. Keyboard, mouse, and touch arrive through `process` as platform events, but gilrs has its own event queue that winit does not surface. Rather than expose a second ingestion path, `begin_frame` internally drains the gilrs queue. Callers see one lifecycle: `begin_frame`, then `process` zero or more times, then queries, regardless of device origin.

gilrs is an accepted external dependency because winit exposes no gamepad API at all; cross-platform device enumeration, hotplug, and axis polling have to come from somewhere. The dependency is fully contained: no gilrs type appears in any public signature, and gilrs init failure is mapped to `InputError::BackendInit` at the boundary. Headless hosts degrade to a no-gamepad mode instead of failing construction.

`InputState` is deliberately not `Clone`. It owns gamepad backend handles indirectly through ids, and cloning frame-edge bits would silently duplicate a half-advanced edge state: a correctness trap with no legitimate use.

Actions are generic over an `ActionId` trait with a blanket impl, so any user enum that is `Copy + Eq + Hash + 'static` works directly as an action key. A provided `Action(u32)` newtype covers the string-free integer case. No string interning, no dynamic action registry: action identity is a compile-time type chosen by the consumer.

Multiple bindings per action aggregate by fixed rules: pressed is the OR over bindings, analog `value` is the MAX, and `axis2` is the sum of 2D contributions clamped to unit length. These rules are simple, order-independent, and let a single action respond to keyboard and gamepad simultaneously without special-casing.

The context stack resolves consumption per-action rather than all-or-nothing. The top context that binds a given action wins and hides it from lower layers; an action no upper layer binds falls straight through. A menu on top of gameplay can swallow movement while a `Pause` action bound only in gameplay still resolves, because the menu does not bind it.

## Architecture

The crate splits into a device-state subtree and a set of flat action-layer modules.

`state/` owns raw device facts. `state/mod.rs` defines `InputState`, the top-level owner constructed once for the app lifetime, and drives the `begin_frame` / `process` lifecycle. `keyboard.rs`, `mouse.rs`, and `touch.rs` each own one device's state and expose read-only query accessors. `gamepad.rs` holds `Gamepads`, the gilrs wrapper, plus `GamepadId`, `GamepadButton`, `GamepadAxis`, `GamepadEvent`, and `Deadzone`.

`action.rs` defines the `ActionId` trait and the `Action` newtype. `binding.rs` defines `Binding` and `AxisDirection`: the closed set of input-to-action bindings, including key/button bindings, directional gamepad-axis bindings with a threshold, and a four-key `Composite2D` that produces a 2D vector. `map.rs` holds `ActionMap`, the `ActionState` result struct, and all rebinding operations. `context.rs` holds `InputContextStack` and `ContextId`. `error.rs` defines `InputError` and the `InputResult` alias.

The public API has three tiers. At the device tier, `InputState` hands out `&Keyboard`, `&Mouse`, `&Touch`, `&Gamepads`, each offering `is_pressed` / `just_pressed` / `just_released` plus device-specific reads (mouse position/delta/wheel, touch point iteration, gamepad axis values and connection events). At the action tier, `ActionMap` takes `bind` / `rebind` / `clear` / `remove` / `replace` mutations and recomputes all action states in `update`, after which `state` and the scalar convenience queries return aggregated `ActionState`. At the context tier, `InputContextStack` layers maps with `push` / `pop` and resolves queries against per-action consumption. `lib.rs` carries the workspace warning denial and re-exports every public type, including `KeyCode` and `MouseButton` from spawn-platform, at the crate root, so consumers need no direct spawn-platform dependency.

## Constraints

- No per-frame heap allocation. The `begin_frame` / `process` / `update` / query path allocates nothing after construction. All device storage is preallocated; the gilrs event drain reuses a fixed scratch buffer; gamepad slots (`MAX_GAMEPADS = 8`) and touch slots (`MAX_TOUCHES = 10`) are fixed-capacity arrays. Events for an over-capacity touch id are dropped, documented and not an error.
- Allocation is permitted only during setup and binding mutation. Each action's binding list is a `Vec<Binding>`; growing or replacing it on the rebind path is allowed because that path is never per-frame.
- No `unsafe`. The crate is entirely safe Rust.
- No `unwrap()`, `expect()`, or `panic!()` outside test code. Fallible operations return `InputResult` or `Option`.
- Dependencies are limited to `spawn-core` (for `Vec2`, error patterns, `ApproxEq` in tests), `spawn-platform` (for `PlatformEvent`, `KeyCode`, `MouseButton`), and `gilrs`. `gilrs` is the only external crate, and its types are confined to `gamepad.rs`: none cross the public boundary.
- Reuse over redefinition: `Vec2` comes from spawn-core; `KeyCode` and `MouseButton` come from spawn-platform and are re-exported, not redefined. The crate owns `GamepadButton`, `GamepadAxis`, `TouchPhase`, and all action-layer types.
- The per-frame contract is fixed: `begin_frame` runs exactly once at the top of a frame, then `process` any number of times, then queries. Mouse delta and wheel accumulate within a frame and zero on `begin_frame`. Edge bits are valid only after `begin_frame` has equalized them.
- Determinism: event effects apply in `process` call order. No interior mutability, no global state.
- `InputError` is `#[non_exhaustive]`; downstream matches must not assume the variant set is closed. `BackendInit` carries a `&'static str` context to avoid allocation on error construction.

## Phase 1 Scope

In scope: per-frame device state for keyboard, mouse, touch, and gamepad driven by platform events; gamepad acquisition and hotplug through gilrs; the action layer: bindings, multiple bindings per action, aggregated queries, and runtime rebinding; the context stack for layered consumption; and unit tests covering all of it. Touch tracks raw points only.

Deferred to Phase 2: serialization of bindings, which will add serde derives on `Binding` and `AxisDirection` plus a stable on-disk schema. The line falls here because rebinding mechanics and the binding type are settling in Phase 1, and committing to a persisted format before the in-memory shape is proven would lock in a schema prematurely.

Out of scope entirely, each gated behind its own future approval: input recording and playback, text and IME composition, chord/sequence/combo detection, virtual cursors, force-feedback and rumble output, raw HID, accelerometer and gyro input, cross-frame action buffering, and touch gestures (tap, swipe, pinch). The boundary is drawn at raw per-frame state plus action mapping, the minimum a game or editor needs to read input, leaving every higher-level interpretation layer for later phases that can build on a proven core.
