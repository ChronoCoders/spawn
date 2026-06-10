# spawn-editor-shell

`spawn-editor-shell` is the visual editor: the full-stack aggregate that turns the headless editor framework into a windowed application. It composes a four-region docked layout — toolbar, outliner, viewport, inspector — over one retained UI tree, renders the live ECS world through the lit render graph with the overlay pass compositing the UI, gizmos, and selection highlight on top, drives a reflection-built inspector whose every edit is undoable, manipulates the selection with translate, rotate, and scale gizmos over the headless gizmo math, and toggles between Edit and Play over the headless snapshot contract. It is the one crate that depends on the whole upper stack at once — ECS, editor framework, render, UI, input, platform, and asset — and the only place where those become an interactive tool. It owns the live scene world it edits, unlike the headless framework beneath it, which only borrows one.

## Design Decisions

**A bespoke immediate-mode driver, not the engine's loop.** The shell implements the platform application interface directly and owns the renderer, the scene world, the editor state, and the UI tree together in one place, running a non-GPU logic pass followed by a render pass each frame. It deliberately does not build on `spawn-engine`. The editor state holds boxed command trait objects that are not `Send`, so it cannot be a `Send` extraction crossing the engine's frontend/backend boundary; the editor wants immediate-mode interaction rather than the engine's deferred render-proxy model; and the renderer must be co-located with the world and the editor state so a gesture can read and mutate both in the same frame. Depending on the engine would also be a dead dependency — its frame clock's mutators are crate-private and its loop is exactly what this driver replaces — so the shell keeps its own wall-clock sampler and a small fixed-step accumulator for play simulation instead.

**Retained UI for the editor, composited by the overlay pass.** The four panels are laid out once in a retained UI tree and updated in place; this is the right model for a data-driven, styled tool, as opposed to the immediate style that suits a debug overlay. The scene renders through the standard lit graph — a depth-only shadow pass feeding a lit forward pass — and a final overlay pass draws the UI draw list, text, and the gizmo, grid, and selection line geometry over the rendered scene. One graph, three passes, one frame.

**The inspector is built from reflection and edits through commands.** For the primary selection, the inspector enumerates each reflected scalar leaf and builds one editable widget row per leaf; every edit is routed through the editor command stack so it is undoable, and a continuous drag of a numeric field coalesces into a single undo entry through the command merge rather than producing one entry per frame. The reflected-field write is itself a command, so the inspector stays agnostic to the concrete component types it edits.

**Gizmos manipulate through transactions; picking is CPU-side.** The translate, rotate, and scale handles are drawn and hit-tested with the headless gizmo math, anchored at the primary selection. A drag is one transaction — begun when a handle is grabbed, advanced by merged transform writes, committed on release, and aborted on escape or when the pointer leaves the viewport mid-gesture — so the whole gesture is one undo step. Entity selection picks against geometry on the CPU using a ray cast, rather than reading a GPU id buffer; the id-buffer approach is deferred, and the bounds-based pick is sufficient for the current scene.

**Edit and Play are conservative, over the headless snapshot.** Play mode is entered and left through the headless framework's snapshot contract; inspector and viewport edits are suppressed while playing, the per-tick world step is a hook the host supplies, and on returning to Edit the inspector and outliner are rebuilt because the restore may have changed the entity set. The known limits of that snapshot — it covers the managed component set, not arbitrary components — are inherited from the framework rather than worked around here.

**The scene draw list is a per-frame local.** Resolving each renderable to its GPU mesh and material borrows the resource registry, so the draw list cannot be a reused field without becoming self-referential with the registry it borrows. It is rebuilt each frame as a local — the same deliberate exception the engine's wgpu backend takes — and a surface-acquire failure skips the frame rather than erroring, so a transient present problem never tears the editor down.

## Architecture

The crate splits into focused modules; `lib.rs` declares them and re-exports the principal types, keeping most modules public for explicit paths and the driver private.

- **`app`** — `EditorApp` and `EditorConfig`, the windowed driver, and the `LiveEditor` that holds the renderer, world, editor state, UI, panels, camera, gizmo controller, and per-frame buffers. It runs the logic pass (input, UI routing, viewport interaction, inspector edits, simulation, panel refresh, layout, overlay assembly) and the render pass, and builds the lit-plus-overlay graph. The `WorldSetup`, `RenderSetup`, and `PlayStep` hook types let a host populate the scene, build GPU resources, and define the play step.
- **`bridge`** — translation from the input state to the UI's input model and the pointer-target test that decides whether a pointer event belongs to the viewport or a panel.
- **`camera`** — `EditorCamera`, an orbit/pan/dolly viewport camera that also produces the picking ray and the render camera for a given aspect.
- **`gizmo`** — `GizmoController` and `GizmoMode`, the drag state machine that turns viewport interaction into merged transform transactions, plus the handle line geometry.
- **`inspector`** — reflection-driven row construction, per-frame value refresh, and the edit-application functions, backed by a reflected-field-write command.
- **`outliner`** — the flat entity list panel and its click handling.
- **`overlay`** — assembly of the grid and selection line geometry the overlay pass draws.
- **`panels`** — the four-region layout build and the viewport-rectangle query.
- **`picking`** — CPU ray-against-scene entity picking.
- **`playmode`** — the Edit/Play toggle over the snapshot contract and the edit-suppression predicate.
- **`scene`** — the `Renderable` component and the draw-list extraction.
- **`status`** / **`toolbar`** / **`theme`** — the status-bar text, the mode/play/undo/redo toolbar with its action enum, and the dark theme with its reserved accent.
- **`util`** — small UI-tree helpers; **`error`** — `ShellError` and `ShellResult<T>`.

## Constraints

- **Allocation.** The shell runs on the editor interaction path — a per-frame loop driven by user gestures, not the engine's allocation-free schedule — so the retained UI tree, boxed setup and play hooks, and command boxing are accepted. The one notable per-frame allocation is the scene draw list, a local because it borrows the resource registry and so cannot be a reused field.
- **Safety.** 100% safe Rust, zero `unsafe`.
- **Panics.** No `unwrap`, `expect`, or `panic!` outside tests. Fallible operations return `ShellResult`; an editor must never crash on a stale reflected field, and a surface-acquire failure skips the frame rather than propagating.
- **Dependencies.** `spawn-core`, `spawn-ecs`, `spawn-asset`, `spawn-editor`, `spawn-render`, `spawn-ui`, `spawn-input`, and `spawn-platform`, plus `std`. It deliberately does not depend on `spawn-engine` or `spawn-debug`. The only third-party crate is winit, and only as a dev-dependency for the GPU-gated device test's window creation — verifiable through `cargo tree -p spawn-editor-shell`.
- **Threading and windows.** Single-window, single-threaded; the driver owns everything the editor touches.
- **Documentation.** Every public item carries a `///` contract doc comment; `#![deny(warnings)]` is in force at the crate root.

## Phase 2 Scope

In scope: the four-region docked editor over a retained UI tree; the scene view rendering the live world through the lit graph with the overlay pass; the orbit/pan/dolly viewport camera; the reflection-built inspector whose edits route through undoable, coalescing transactions; translate, rotate, and scale gizmos with CPU ray picking; the flat outliner; the Edit/Play toggle over the snapshot contract; the toolbar, status bar, and dark theme; and a GPU-gated device test that composes the editor's pieces and renders one frame through the lit-plus-overlay graph.

Deferred, each to a later approved phase: GPU id-buffer picking (bounds-based CPU picking is used now); image rendering in the overlay pass; vector or TrueType text (a bitmap monospace font is used at runtime, with the design-target metrics already matched); detachable and multi-monitor panels; an entity hierarchy with parenting (the outliner is flat); an asset browser; scene serialization to and from disk; prefabs; copy and paste; and multi-document tabs. Play-in-editor world isolation beyond the headless snapshot contract is gated on a dedicated research pass. On-target interactive validation of the editor window is pending, since it requires a desktop window station — the same constraint that gates the Phase 1 present path. The line falls at a coherent, reviewed visual editor whose remaining features either need a GPU capability not yet built (id-buffer picking, image overlays), an ECS capability not yet built (entity hierarchy, scene serialization), or a research pass (play isolation) before they can land.
