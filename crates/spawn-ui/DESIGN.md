# spawn-ui

`spawn-ui` is a retained-mode UI layout and event-routing core. It owns a tree of styled nodes, runs a flexbox-subset layout pass over them, routes pointer and wheel input to the node under the cursor, and emits a flat, renderer-agnostic list of draw commands. It exists to give the engine a single source of truth for UI structure and geometry that knows nothing about GPUs, windows, or fonts — every backend concern (rasterization, glyph shaping, scissor intersection) is pushed across a clean boundary to a consumer such as `spawn-render`. The crate produces data; it does not paint.

## Design Decisions

**Retained tree, not immediate mode.** The tree persists across frames and tracks dirty state, so an unchanged frame costs nothing to re-layout. This trades the simplicity of immediate-mode rebuilding for incrementality and stable node identity, which the editor and event model both require. Nodes have lifetimes and handles; callers mutate the tree, not a per-frame command stream.

**Generational handles instead of references or raw indices.** `NodeId` is a `{ index, generation }` slotmap handle with private fields and no public constructor. A stale handle (one whose generation no longer matches the live slot) resolves to "invalid" rather than aliasing a recycled slot. Queries against an invalid handle return `None`; mutations return `Err(InvalidNode)`. Nothing in the public API can panic on a bad id, and ids cannot be forged. There is deliberately no null/sentinel `NodeId` — absence is expressed with `Option`, not a magic value.

**Single-pass measure-then-arrange layout.** Layout is one bottom-up measure descent followed by one top-down arrange pass per dirty subtree, O(n) in dirty nodes. A full multi-pass flexbox solver was rejected for Phase 1: the single pass cannot re-distribute free space after min/max clamping, which is a documented limitation, but it keeps the algorithm linear, predictable, and cheap. `Percent` against an indefinite (`Auto`) parent main size resolving to `0` falls directly out of this structure and matches CSS `flex-basis: %` behavior against indefinite parents.

**Renderer agnosticism is structural, not conventional.** The crate has no dependency on `spawn-render` and no knowledge of textures beyond opaque `u64` handles it never interprets. `spawn-render` depends on `spawn-ui` and consumes its `DrawList`; the arrow points one way only. Input arrives the same way: the caller bridges `spawn-input` into a small per-frame `UiInputState` struct rather than the tree reaching into the input system. This keeps the tree usable headless and testable without a window.

**Text is a measured box in Phase 1.** A `TextMeasure` callback supplies intrinsic text size during the measure pass; that is the entire font story here. `DrawCommand::Text` carries a rect, a `FontId`, and the originating `NodeId` — no glyph runs. Shaping, positioning, and rasterization belong to the renderer and to Phase 2. This split lets layout depend on text metrics without dragging a font stack into the crate.

**Determinism by construction.** Layout output and draw-list order are a pure function of tree, styles, `root_size`, and the measure callback. No order-significant path iterates a hashed container. Arithmetic is left-to-right with no rounding — sub-pixel rects are returned, and snapping is left to the renderer. Identical inputs always yield byte-identical output, which is what makes golden tests viable.

## Architecture

The crate is `std`-only and splits across seven modules, each re-exported at the crate root so callers can write `spawn_ui::UiTree` while explicit module paths remain available.

- **`tree`** — `NodeId` and `UiTree`. Owns slot storage, the free list, the root handle, and the dirty set, all private. Holds the hierarchy operations (`create_node`, `remove_node`, `set_style`, `set_children_order`, the read accessors) and the per-node content setters (`set_text`, `set_image`, `set_font`). Child order is the single source of truth for in-flow order, draw z-order, and hit-test priority simultaneously: a later sibling lays out later, draws on top, and wins a hit test.

- **`style`** — `Style` and its component types: `Dimension` (`Auto`/`Px`/`Percent`), `Size`, `Edges`, `Border`, and the layout enums (`Display`, `FlexDirection`, `JustifyContent`, `AlignItems`). `Style` is `Copy`, which is what the no-heap, dirty-flag model needs. `Percent` is a `[0,1]` fraction, not 0–100. `Display::None` excises a node and its subtree from layout, draw, and hit testing.

- **`layout`** — `compute_layout`, the `layout(node)` rect query, the `TextMeasure` trait, and the dirty-propagation machinery (the last of which is internal). `compute_layout` rejects negative or non-finite `root_size`. The cached rect per node is its border box in root space (absolute, not parent-relative), valid only after a successful layout.

- **`input`** — `UiInputState`, `hit_test`, frame update, event draining, and hover/active queries. `update_input` runs after layout against cached rects and refuses to run while layout is dirty (`InvalidState`). Hit testing walks depth-first in reverse child order, honoring `overflow_clip` ancestors.

- **`draw`** — `DrawList`, `DrawCommand`, the opaque `TextureId`/`FontId` handles, `UiImage`, and `build_draw_list`. Emits commands in pre-order depth-first tree order with strictly nested scissor push/pop pairs.

- **`error`** — `UiError` and `UiResult<T>`.

- **`lib`** — module declarations, crate-level lint config, and the root re-exports, including a re-export of `spawn_input::MouseButton` for caller convenience.

The public surface is, at a high level: construct a tree from a root style; mutate it with hierarchy and style/content setters that return `UiResult`; run `compute_layout` once per frame against a root size and a measure callback; query final rects; feed a per-frame input snapshot through `update_input` and drain the resulting `UiEvent` queue; and emit a `DrawList` into a caller-owned, reused buffer. Events flow only through the drained queue — there are no callbacks. Hover and active state are also directly queryable.

## Constraints

- **Allocation.** The tree owns node storage, allocated on `create_node` and freed on `remove_node`. In steady state — no mutations, same `root_size` — `compute_layout` does zero work and zero allocation, and `build_draw_list` rebuilds into the caller's reused `DrawList` with zero heap allocation because the buffer's capacity is retained across frames (`clear` keeps capacity). The draw list is rebuilt every frame; caching it is out of scope. Error payloads carry `&'static str` context only, so the error path never allocates.
- **Safety.** No `unsafe`, anywhere. No `unwrap`, `expect`, or `panic!` outside test code. Fallible operations return `Option` or `UiResult`; an invalid `NodeId` never panics.
- **Dependencies.** Permitted: `spawn-core` (for `Vec2`, `Rect`, `Color`, error patterns, `ApproxEq`), `spawn-input` (for the re-exported `MouseButton`), and `std`. Forbidden: every external crate, and any dependency on or knowledge of `spawn-render`, GPU resources, or window backends. The dependency on `spawn-render` runs the other direction.
- **Invariants.** UI space is `spawn-core` screen convention: origin top-left, `+x` right, `+y` down, logical pixels, `f32` exclusively. All rects returned by `layout()` are absolute root-space border boxes. Shared types (`Vec2`, `Rect`, `Color`, `MouseButton`) are reused, never redefined; the crate owns `NodeId`, `Style`, `DrawList`, and the event/state types. The root node always exists for the tree's lifetime and cannot be removed. Layout and draw-list ordering remain a deterministic pure function of their inputs. `NodeId` is 8 bytes, asserted at compile time. The public API matches this design exactly — extra public items or omissions require an amendment and re-approval.

## Phase 1 Scope

In scope: the retained `UiTree` of styled nodes; the flexbox-subset `Style`; the single-pass `compute_layout` with per-node root-space rect queries; the `TextMeasure` hook with text treated as a measured layout box; pointer and wheel routing with hit testing, hover/active tracking, and a drained `UiEvent` queue; renderer-agnostic `DrawList` generation with z-ordering and scissor clipping; dirty-flag propagation for incremental layout; and full unit, golden, hit-test, event-sequence, draw-list, and allocation tests.

Deferred to Phase 2: the widget convenience layer — buttons, sliders, text fields, checkboxes, scroll views — which are compositions over this tree, not primitives within it; and glyph shaping, positioning, and rasterization, which the renderer performs using the `text_node` handle to fetch the string. Deferred without a fixed phase, each behind its own future approval: animation and transitions; data binding and reactive diffing; absolute, grid, and wrap layout; focus traversal and keyboard navigation policy; theming; the accessibility tree; and serialization.

The line sits at "structure and geometry, nothing painted, nothing composed." Everything kept is a primitive that higher layers build on; everything deferred either needs a font stack (text rendering), is a composition of the primitives (widgets), or introduces policy the core deliberately stays free of (navigation, theming, animation). Drawing that boundary keeps the core small, deterministic, and dependency-free, which is what lets the editor and renderer build on it with confidence.
