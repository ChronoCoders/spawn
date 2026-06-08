# spawn-render

`spawn-render` is the GPU rendering layer of the Spawn engine. It turns mesh, material, texture, and camera data into presented frames against a window surface, driving everything through `wgpu` so the engine stays portable across Vulkan, Metal, and DX12 without touching any native API directly. It exists to give higher layers (editor, in-game UI, debug overlays) a stable, allocation-disciplined rendering surface with explicit resource lifetimes and predictable per-frame cost, rather than scattering GPU calls throughout the codebase.

## Design Decisions

`wgpu` is the sole graphics backend. No raw Vulkan/Metal/DX12 call exists anywhere in the crate, and no competing backend crate is permitted. This trades a small amount of low-level control for a single portable abstraction that already handles device negotiation, command submission, and GPU synchronization. Backend selection is wgpu's concern, not the engine's.

The crate contains zero `unsafe`. Casting vertex and uniform structs to byte slices for upload runs exclusively through `bytemuck`, using `Pod`/`Zeroable` derives plus `cast_slice`. Hand-rolled transmutes were rejected outright: every `#[repr(C)]` upload type is plain-old-data, so `bytemuck` covers the need without sacrificing the safe-Rust guarantee.

GPU synchronization is delegated to wgpu for this phase. The render graph is deliberately a "graph-lite": an ordered list of passes with author-declared inputs and outputs that get validated for correctness, but the crate inserts no barriers itself. Declared resource I/O exists so pass ordering and dependencies are visible and checkable, not because the engine schedules GPU work. A full automatic graph with barrier inference was considered and pushed out — it would add substantial machinery for no benefit while wgpu still owns sync.

Pipeline state objects and shader modules are built once and cached, never per frame. Render pipelines live in a cache keyed by `(shader, vertex layout, render state)`; shaders compile at load through a shader store. A draw that references an uncached pipeline is a programmer error surfaced as `PipelineNotCached`, not a silent on-demand build — building inside the frame loop would reintroduce exactly the latency spikes the cache exists to eliminate.

The crate holds no global state. All GPU state lives on a `Renderer`, one per window/surface. `Device` and `Queue` sit behind `Arc` so resource-construction helpers can be handed out without borrowing the entire renderer; no other field is shared. This keeps ownership legible and avoids the lifetime hazards that come from exposing raw device handles.

Coordinate conventions are fixed and normative: right-handed, column-major matrices applied as `M * v`, depth range `[0, 1]`. Camera matrices come from `spawn-core`'s `perspective_rh`/`orthographic_rh`/`look_at_rh` builders, so projection conventions match the rest of the engine rather than drifting per crate.

## Architecture

The crate splits into focused modules, each re-exported at the crate root so downstream code never reaches into submodules:

- **renderer** — the `Renderer` and `RendererConfig`. Owns the wgpu instance, adapter, device, queue, surface, surface configuration, depth texture, pipeline cache, shader store, created-once bind-group layouts, and the reused per-frame recording collections. Construction negotiates instance/adapter/device/queue and configures the surface. `Renderer::new` borrows the window for the surface lifetime `'w`; `Renderer::from_owned` takes an `Arc<W: HasWindowHandleSet>` so the surface *owns* the window and the renderer is `Renderer<'static>` — the storable form a long-lived engine wrapper needs without a borrow tying it to the window. Accessors hand out the `Arc<Device>`/`Arc<Queue>` pair and expose the cache, shader store, and bind-group layouts for the setup/load path.
- **frame** — `FrameContext`, the borrowed handle alive for exactly one frame. Covers surface-texture acquisition, encoder creation, pass recording, submission, and present, plus surface-loss recovery.
- **graph** — `RenderGraph`, `RenderPassDesc`, and pass-description types. Owns graph construction and validation.
- **passes::forward_opaque** — the single Phase 1 pass kind. Defines `RenderScene` and `DrawItem` and records opaque draws with depth test on and no blending.
- **pipeline** — `PipelineKey`, `RenderStateKey`, `PipelineCache`, and the shader store. The cache is the only place a `wgpu::RenderPipeline` is constructed; the store is the only place a `wgpu::ShaderModule` is compiled.
- **mesh** — the fixed `Vertex` type with its `const` attribute layout, and `Mesh` (vertex/index buffers plus index metadata).
- **material** — `Material` and `MaterialUniform`. A material owns its uniform buffer and bind group and carries the `PipelineKey` used to look up its pipeline.
- **texture** — `Texture` and `SamplerConfig`, including RGBA8 and asset-payload constructors.
- **camera** — `Camera` and `CameraUniform`, with perspective/orthographic builders and view-projection math.
- **format** — `SurfaceSize`, `DepthFormat`, and the wgpu enum re-exports (`TextureFormat`, `PresentMode`, `PowerPreference`, `FilterMode`, `AddressMode`, `CompareFn`, `CullMode`, `Topology`) so downstream crates do not pull in wgpu for these types.
- **error** — `RenderError`, `RenderResult`, and `SourceLocation`.

The public API shape: a caller constructs a `Renderer` from a window handle and size, loads shaders into the store and builds pipelines into the cache during setup, creates `Mesh`/`Material`/`Texture`/`Camera` resources via constructors that take `&Renderer` (or the device/queue Arcs), assembles and validates a `RenderGraph` once, and then per frame calls `begin_frame` to get a `FrameContext`, `execute` against a graph and a `RenderScene`, and `end_frame` to submit and present. Resource handles wrap their backing wgpu objects and free them on drop with no manual teardown.

## Constraints

- **Allocation:** No heap memory the engine owns is allocated between `begin_frame` and `end_frame`. Draw-gathering collections are cleared (not reallocated) at frame start and retain capacity across frames; no `Box`/`Vec`/`HashMap` growth and no per-draw string formatting occur in the loop. Uniform updates go through `queue.write_buffer` into pre-sized buffers — never per-frame buffer creation. Transient wgpu objects the API forces (encoder, acquired surface texture, render pass) are not engine heap allocations and are exempt; any collection the engine owns must be reused.
- **Safety:** Zero `unsafe` in the crate. No transmutes. Byte casting goes only through `bytemuck`. No `unwrap`/`expect`/`panic!` outside test code — every fallible operation returns `RenderResult<T>`.
- **Dependencies:** May depend on `spawn-core`, `spawn-platform`, `spawn-asset`, `wgpu`, and `bytemuck`. May not depend on any non-wgpu graphics backend crate, and may not make raw Vulkan/Metal/DX12 calls.
- **Pipelines and shaders:** A `wgpu::RenderPipeline` is constructed only in `PipelineCache::get_or_create`, only on a cache miss, only at startup or asset load. A `wgpu::ShaderModule` is compiled only in the shader store at load. Neither happens inside the frame loop.
- **State and submission:** No global state — all GPU state lives on a `Renderer`, one per surface. Exactly one `queue.submit` and one `present` per `end_frame`. Redundant consecutive pipeline/camera-group binds are skipped by tracking the last-bound key in a reused field.
- **Surface recovery:** A `Lost`/`Outdated` surface is recovered by reconfiguring and retrying once; it never crashes the renderer. A zero-size resize is a suppressed no-op (minimized window) and no present happens until a non-zero size returns.
- **Graph validity:** `execute` refuses to record against a graph that is not currently validated. The graph carries a validated flag set by a successful `validate()` and cleared by any mutation, closing the gap where a post-validation edit or a forgotten validation call could reach execution and clobber the singleton camera/model uniforms.

## Phase 1 Scope

In scope: renderer init (instance/adapter/device/queue), surface configuration and resize with surface-loss recovery, the `begin_frame`/`execute`/`end_frame` lifecycle, the explicit ordered render-graph-lite with declared and validated I/O, a single forward opaque pass, the fixed position/normal/uv vertex layout, `Mesh`, `Material` (shader handle plus bind group plus uniform block), `Texture` (2D, sRGB and linear, configurable sampler, single mip level), `Camera` (perspective and orthographic with a view-projection uniform), the keyed pipeline cache, clear color, the `RenderError`/`RenderResult` surface, and unit plus headless tests.

Explicitly deferred to Phase 2: lighting (a group-2 light-uniform bind group, a `ForwardLit` pass kind, and matching error variants), and a full automatic render graph with barrier inference. Group 2 stays unused in Phase 1 and the forward pass is unlit — base color times texture only. The error enum is `#[non_exhaustive]` precisely so lighting variants can be added without breaking callers; nothing else in the Phase 1 API anticipates lighting.

Deferred without a fixed phase, each requiring its own approved design before work begins: shadows, transparency and blending beyond the opaque pass, post-processing, compute passes, instancing, skinning, multi-viewport, MSAA, mipmap generation, texture streaming, and any raw backend access.

The line sits here because Phase 1 targets a correct, allocation-disciplined opaque pipeline that proves the resource-lifetime model, the pipeline cache, and the frame lifecycle end to end. The fixed single-pass, single-vertex-layout, single-pass-kind restrictions are enforced rather than merely assumed — `validate` rejects offscreen color targets and multi-pass graphs because the per-frame camera and model uniform buffers are singletons whose contents would be overwritten across multiple recorded passes before the single submission. Future-proofing that costs nothing now is kept (the `VertexLayoutId` enum and `PipelineKey` carry room to grow); machinery that earns its keep only once lighting or offscreen rendering lands is held back.
