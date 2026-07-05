//! The frontend/backend boundary: render proxies (backend-owned plain data
//! extracted from the ECS world at the sync point), the [`RenderExecutor`] seam
//! that publishes a filled proxy buffer by ownership (returning a recycled empty
//! one), the [`RenderBackend`] trait, and its two implementations.
//!
//! Render-relevant state crosses the frame boundary only as extracted proxies; a
//! backend never reads live ECS storage during `submit`, and the frontend never
//! touches a buffer it has handed off (Finding 2: single-owner proxies, no shared
//! lock).

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;

use spawn_asset::{AssetId, ReloadEvent};
use spawn_core::{Color, Mat4};
use spawn_platform::Window;
use spawn_render::{
    AdapterInfo, Camera, ColorWrite, CompiledGraph, DepthWrite, DrawItem, Font, FontRegistry,
    Lighting, Overlay, PassDesc, PassKind, RenderGraph, RenderResources, RenderScene, Renderer,
    RendererConfig, ResourceDesc, ResourceKind, ShadowConfig, SizeSpec, SurfaceSize,
};
use spawn_ui::{DrawList, UiTree};

use crate::error::{EngineError, EngineResult};
use crate::frame::SyncMode;
use crate::ui::DEFAULT_FONT;

/// Extracted camera state: world→view and view→clip, as plain matrices.
#[derive(Debug, Clone, Copy)]
pub struct CameraProxy {
    pub view: Mat4,
    pub projection: Mat4,
}

impl Default for CameraProxy {
    fn default() -> Self {
        Self {
            view: Mat4::IDENTITY,
            projection: Mat4::IDENTITY,
        }
    }
}

/// One extracted renderable: a world transform plus the identities of the mesh
/// and material to draw with. Resource identities (not GPU handles) so the proxy
/// is plain, backend-owned data with no borrow of ECS or GPU state.
#[derive(Debug, Clone, Copy)]
pub struct RenderProxy {
    pub model: Mat4,
    pub mesh: AssetId,
    pub material: AssetId,
}

/// The full extraction for one frame: the active camera, the scene lighting (one
/// directional light + shadow), and the draw list. The `draws` vector is cleared
/// (not freed) each frame and retains capacity, so a steady draw count is
/// allocation-free.
#[derive(Default)]
pub struct RenderProxies {
    pub camera: CameraProxy,
    pub lighting: Lighting,
    pub draws: Vec<RenderProxy>,
}

impl RenderProxies {
    /// Clears the draw list (retaining capacity) and resets the camera and
    /// lighting, readying the buffer for a fresh extraction.
    pub(crate) fn reset(&mut self) {
        self.camera = CameraProxy::default();
        self.lighting = Lighting::default();
        self.draws.clear();
    }
}

/// Consumes extracted proxies and turns them into presented frames. Implemented
/// by [`WgpuBackend`] (real GPU) and [`HeadlessBackend`] (no GPU). `Send` so the
/// backend can be built and owned on the render thread.
pub trait RenderBackend: Send {
    /// Renders one frame from the published proxy buffer. `ui`, when present, is
    /// the engine-owned overlay tree: the backend lays it out and composites it
    /// over the lit scene. The headless backend ignores it.
    fn submit(&mut self, proxies: &RenderProxies, ui: Option<&mut UiTree>) -> EngineResult<()>;
    /// Reconfigures for a new surface size.
    fn resize(&mut self, size: SurfaceSize) -> EngineResult<()>;
    /// The selected GPU adapter's identity for startup logging, or `None` when the
    /// backend has no GPU (headless). Read once during assembly; defaults to
    /// `None` so a GPU-free backend needs no override.
    fn adapter_info(&self) -> Option<AdapterInfo> {
        None
    }
    /// Runs the render-reload hooks against the backend's live renderer and
    /// resource registry when assets reload in place, before the next submit. The
    /// headless backend has no renderer and ignores them (the asset-level swap
    /// still happens and is observable via
    /// [`ReloadEvents`](crate::ReloadEvents)); defaults to a no-op.
    fn apply_render_reloads(
        &mut self,
        _reloads: &[ReloadEvent],
        _hooks: &mut [RenderReload],
    ) -> EngineResult<()> {
        Ok(())
    }
}

/// The backend's per-frame outcome, observable without reading backend state
/// across the executor boundary. `error` carries a per-frame backend failure so
/// the frontend can surface it from `tick`; it is `None` on a rendered frame.
#[derive(Debug, Default)]
pub struct RenderReport {
    /// Monotonic index of the frame the backend last rendered.
    pub frame: u64,
    /// Draw proxies submitted on that frame.
    pub draw_count: usize,
    /// The frame's backend error, when the frame failed.
    pub error: Option<EngineError>,
}

/// The seam between the frame loop and the render backend. Frame publishing goes
/// through an executor instead of calling [`RenderBackend::submit`] inline, so the
/// backend can move to a render thread without changing the loop. Proxy buffers
/// are passed by ownership: the frontend hands over a filled buffer and gets a
/// recycled empty one back, so a buffer is owned by exactly one side at a time.
pub(crate) trait RenderExecutor {
    /// Renders the filled proxy buffer for this frame, running any render-reload
    /// hooks for `reloads` first and compositing the `ui` snapshot when present.
    /// Returns the recycled (cleared) buffer to extract into next frame plus a
    /// [`RenderReport`]. A per-frame backend error is carried in the report; a
    /// transport failure (render thread gone) is an `Err`. Buffers, reloads, and
    /// the UI snapshot are owned so they can cross to the render thread.
    fn submit(
        &mut self,
        filled: RenderProxies,
        reloads: Vec<ReloadEvent>,
        ui: Option<UiTree>,
        mode: SyncMode,
    ) -> EngineResult<(RenderProxies, RenderReport)>;
    /// Reconfigures the backend for a new surface size.
    fn resize(&mut self, size: SurfaceSize) -> EngineResult<()>;
    /// Extractions submitted but not yet rendered: `0` in `Immediate`, `≤1` in
    /// `Pipelined` on the render thread; always `0` for the inline executor.
    fn frames_in_flight(&self) -> u32;
    /// The selected GPU adapter's identity, for startup logging.
    fn adapter_info(&self) -> Option<AdapterInfo>;
    /// Shuts the executor down, joining the render thread. Idempotent; a no-op for
    /// the inline executor.
    fn shutdown(&mut self) -> EngineResult<()>;
}

/// The synchronous executor: owns the backend and renders on the calling thread.
/// `Immediate` and `Pipelined` collapse to the same behavior (render the buffer
/// just extracted), so frames-in-flight is always `0`. This is the headless and
/// single-threaded path, kept bit-reproducible.
pub(crate) struct InlineExecutor {
    backend: Box<dyn RenderBackend>,
    reloads: Vec<RenderReload>,
    frame: u64,
}

impl InlineExecutor {
    pub(crate) fn new(backend: Box<dyn RenderBackend>, reloads: Vec<RenderReload>) -> Self {
        Self {
            backend,
            reloads,
            frame: 0,
        }
    }
}

impl RenderExecutor for InlineExecutor {
    fn submit(
        &mut self,
        mut filled: RenderProxies,
        reloads: Vec<ReloadEvent>,
        mut ui: Option<UiTree>,
        _mode: SyncMode,
    ) -> EngineResult<(RenderProxies, RenderReport)> {
        let mut report = RenderReport {
            frame: self.frame,
            draw_count: filled.draws.len(),
            error: None,
        };
        self.frame += 1;
        if !reloads.is_empty() && !self.reloads.is_empty() {
            if let Err(e) = self
                .backend
                .apply_render_reloads(&reloads, &mut self.reloads)
            {
                report.error = Some(e);
                filled.reset();
                return Ok((filled, report));
            }
        }
        if let Err(e) = self.backend.submit(&filled, ui.as_mut()) {
            report.error = Some(e);
        }
        filled.reset();
        Ok((filled, report))
    }

    fn resize(&mut self, size: SurfaceSize) -> EngineResult<()> {
        self.backend.resize(size)
    }

    fn frames_in_flight(&self) -> u32 {
        0
    }

    fn adapter_info(&self) -> Option<AdapterInfo> {
        self.backend.adapter_info()
    }

    fn shutdown(&mut self) -> EngineResult<()> {
        Ok(())
    }
}

/// How [`Engine::assemble`](crate::Engine) obtains its backend: an already-built
/// backend rendered inline, or a builder run on the render thread. The threaded
/// builder is `Send` and runs on the render thread so surface/GPU creation and the
/// render-setup hooks happen where the backend lives.
pub(crate) enum RenderTarget {
    Inline(Box<dyn RenderBackend>),
    Threaded(Box<dyn FnOnce() -> EngineResult<Box<dyn RenderBackend>> + Send>),
}

/// Messages from the frame thread to the render thread. `Submit` is large and
/// sent every frame; boxing it to shrink the enum would add a per-frame heap
/// allocation, defeating the ownership-passing transport, so the size gap is
/// intentional.
#[allow(clippy::large_enum_variant)]
enum RenderMsg {
    Submit {
        proxies: RenderProxies,
        reloads: Vec<ReloadEvent>,
        ui: Option<UiTree>,
    },
    Resize(SurfaceSize),
    Shutdown,
}

/// A rendered frame handed back to the frame thread: the recycled (cleared) buffer
/// and the frame's report.
struct Completion {
    proxies: RenderProxies,
    report: RenderReport,
}

/// The threaded executor: a single OS render thread owns the backend and renders
/// off the frame thread. Proxy buffers cross by ownership over channels (no shared
/// lock); `Immediate` blocks on the completion, `Pipelined` keeps at most one
/// frame in flight via a two-buffer cycle.
pub(crate) struct ThreadedExecutor {
    messages: Sender<RenderMsg>,
    completions: Receiver<EngineResult<Completion>>,
    handle: Option<JoinHandle<()>>,
    adapter: Option<AdapterInfo>,
    spare: Option<RenderProxies>,
    outstanding: u32,
}

impl ThreadedExecutor {
    pub(crate) fn spawn(
        build: Box<dyn FnOnce() -> EngineResult<Box<dyn RenderBackend>> + Send>,
        reloads: Vec<RenderReload>,
    ) -> EngineResult<Self> {
        let (msg_tx, msg_rx) = mpsc::channel::<RenderMsg>();
        let (comp_tx, comp_rx) = mpsc::channel::<EngineResult<Completion>>();
        let (ready_tx, ready_rx) = mpsc::channel::<EngineResult<Option<AdapterInfo>>>();
        let handle = std::thread::Builder::new()
            .name("spawn-render".to_string())
            .spawn(move || render_loop(build, reloads, &msg_rx, &comp_tx, &ready_tx))
            .map_err(|_| EngineError::RenderThread {
                context: "failed to spawn render thread",
            })?;
        let adapter = match ready_rx.recv() {
            Ok(Ok(info)) => info,
            Ok(Err(e)) => {
                let _ = handle.join();
                return Err(e);
            }
            Err(_) => {
                let _ = handle.join();
                return Err(EngineError::RenderThread {
                    context: "render thread exited during startup",
                });
            }
        };
        Ok(Self {
            messages: msg_tx,
            completions: comp_rx,
            handle: Some(handle),
            adapter,
            spare: Some(RenderProxies::default()),
            outstanding: 0,
        })
    }

    fn recv_completion(&mut self) -> EngineResult<Completion> {
        let received = self
            .completions
            .recv()
            .map_err(|_| EngineError::RenderThread {
                context: "render thread disconnected",
            })?;
        self.outstanding = self.outstanding.saturating_sub(1);
        received
    }
}

impl RenderExecutor for ThreadedExecutor {
    fn submit(
        &mut self,
        filled: RenderProxies,
        reloads: Vec<ReloadEvent>,
        ui: Option<UiTree>,
        mode: SyncMode,
    ) -> EngineResult<(RenderProxies, RenderReport)> {
        self.messages
            .send(RenderMsg::Submit {
                proxies: filled,
                reloads,
                ui,
            })
            .map_err(|_| EngineError::RenderThread {
                context: "render thread disconnected on submit",
            })?;
        self.outstanding += 1;
        // `Immediate` blocks for this frame's completion (zero in flight).
        // `Pipelined` returns the previous frame's completion once two are in
        // flight, or a spare buffer while priming, keeping at most one in flight.
        if mode == SyncMode::Immediate || self.outstanding >= 2 {
            let completion = self.recv_completion()?;
            Ok((completion.proxies, completion.report))
        } else if let Some(spare) = self.spare.take() {
            Ok((spare, RenderReport::default()))
        } else {
            let completion = self.recv_completion()?;
            Ok((completion.proxies, completion.report))
        }
    }

    fn resize(&mut self, size: SurfaceSize) -> EngineResult<()> {
        self.messages
            .send(RenderMsg::Resize(size))
            .map_err(|_| EngineError::RenderThread {
                context: "render thread disconnected on resize",
            })
    }

    fn frames_in_flight(&self) -> u32 {
        self.outstanding
    }

    fn adapter_info(&self) -> Option<AdapterInfo> {
        self.adapter.clone()
    }

    fn shutdown(&mut self) -> EngineResult<()> {
        if self.handle.is_none() {
            return Ok(());
        }
        let _ = self.messages.send(RenderMsg::Shutdown);
        while self.outstanding > 0 {
            match self.completions.recv() {
                Ok(_) => self.outstanding -= 1,
                Err(_) => break,
            }
        }
        match self.handle.take() {
            Some(handle) => handle.join().map_err(|_| EngineError::RenderThread {
                context: "render thread panicked",
            }),
            None => Ok(()),
        }
    }
}

impl Drop for ThreadedExecutor {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// The render thread: builds the backend, reports its adapter identity, then
/// renders each `Submit` and hands the recycled buffer back until `Shutdown` or a
/// disconnect. A per-frame backend error rides back in the report; a fatal build
/// or resize error ends the thread and surfaces on the next completion receive.
fn render_loop(
    build: Box<dyn FnOnce() -> EngineResult<Box<dyn RenderBackend>> + Send>,
    mut reloads: Vec<RenderReload>,
    messages: &Receiver<RenderMsg>,
    completions: &Sender<EngineResult<Completion>>,
    ready: &Sender<EngineResult<Option<AdapterInfo>>>,
) {
    let mut backend = match build() {
        Ok(backend) => backend,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    if ready.send(Ok(backend.adapter_info())).is_err() {
        return;
    }
    let mut frame: u64 = 0;
    while let Ok(message) = messages.recv() {
        match message {
            RenderMsg::Submit {
                mut proxies,
                reloads: events,
                mut ui,
            } => {
                let mut report = RenderReport {
                    frame,
                    draw_count: proxies.draws.len(),
                    error: None,
                };
                frame += 1;
                if !events.is_empty() && !reloads.is_empty() {
                    if let Err(e) = backend.apply_render_reloads(&events, &mut reloads) {
                        report.error = Some(e);
                    }
                }
                if report.error.is_none() {
                    if let Err(e) = backend.submit(&proxies, ui.as_mut()) {
                        report.error = Some(e);
                    }
                }
                proxies.reset();
                if completions
                    .send(Ok(Completion { proxies, report }))
                    .is_err()
                {
                    break;
                }
            }
            RenderMsg::Resize(size) => {
                if backend.resize(size).is_err() {
                    break;
                }
            }
            RenderMsg::Shutdown => break,
        }
    }
}

/// A GPU-free backend: validates the proxy buffer and records the frame and draw
/// count, presenting nothing. Drives the headless example and the integration
/// tests so the full frontend/backend split is exercised without a display
/// server.
#[derive(Debug, Default)]
pub struct HeadlessBackend {
    frame: u64,
    last_draw_count: usize,
}

impl HeadlessBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of draw proxies submitted on the most recent frame.
    pub fn last_draw_count(&self) -> usize {
        self.last_draw_count
    }

    /// Total frames submitted.
    pub fn frame_count(&self) -> u64 {
        self.frame
    }
}

impl RenderBackend for HeadlessBackend {
    fn submit(&mut self, proxies: &RenderProxies, _ui: Option<&mut UiTree>) -> EngineResult<()> {
        self.last_draw_count = proxies.draws.len();
        self.frame += 1;
        Ok(())
    }

    fn resize(&mut self, _size: SurfaceSize) -> EngineResult<()> {
        Ok(())
    }
}

/// The real wgpu backend: owns a surface-owning [`Renderer<'static>`] (built from
/// the platform window via [`Renderer::from_owned`]) and a compiled lit
/// [`RenderGraph`] (a depth-only shadow pass feeding a lit forward pass).
///
/// `submit` extracts the camera proxy into a [`Camera`], threads the lighting
/// proxy through, resolves each draw proxy to its GPU mesh/material through the
/// [`RenderResources`] registry (skipping unregistered ids), and runs the frame
/// lifecycle. The registry is populated by app render-setup hooks at construction.
pub struct WgpuBackend {
    renderer: Renderer<'static>,
    graph: RenderGraph,
    compiled: CompiledGraph,
    resources: RenderResources,
    fonts: FontRegistry,
    measure_font: Font,
    ui_draw_list: DrawList,
    size: SurfaceSize,
}

/// An app-provided render-setup routine: builds GPU `Mesh`/`Material` resources
/// from the renderer and registers them in the registry, run once at backend
/// construction (after the renderer exists). Headless mode has no renderer and
/// does not run these.
pub type RenderSetup =
    Box<dyn FnOnce(&mut Renderer, &mut RenderResources) -> EngineResult<()> + Send>;

/// An app-provided render-reload hook: rebuilds GPU `Mesh`/`Material` resources in
/// the registry when a watched asset reloads in place. Run on the render backend
/// after the asset pump reports reloads, before the next submit. `FnMut` so it
/// persists across reloads; `Send` so it can run on the render thread. Headless
/// mode has no renderer and does not run these.
pub type RenderReload =
    Box<dyn FnMut(&[ReloadEvent], &mut Renderer, &mut RenderResources) -> EngineResult<()> + Send>;

impl WgpuBackend {
    /// Builds the backend from an owned window handle and an initial surface
    /// size, with `clear_color` as the per-frame surface clear. Compiles a single
    /// forward-opaque pass targeting the surface, then runs the render-setup
    /// hooks to populate the resource registry.
    pub fn new(
        window: Arc<Window>,
        size: SurfaceSize,
        config: RendererConfig,
        clear_color: Color,
        setups: Vec<RenderSetup>,
    ) -> EngineResult<Self> {
        let mut renderer = Renderer::from_owned(window, size, config)?;
        // `renderer` and `resources` are locals here, so a setup hook can borrow
        // `&mut renderer` (to build pipelines) and `&mut resources` together
        // without a field-borrow clash.
        let mut resources = RenderResources::new();
        for setup in setups {
            setup(&mut renderer, &mut resources)?;
        }
        let mut fonts = FontRegistry::new();
        fonts.insert(
            &renderer,
            DEFAULT_FONT,
            Font::embedded_monospace(&renderer, 8.0)?,
        );
        let measure_font = Font::embedded_monospace(&renderer, 8.0)?;
        // The standard engine graph is lit: a depth-only shadow caster writes the
        // shadow map, then the lit forward pass reads it and shades the surface.
        // The derived order places the shadow pass first (the lit pass reads its
        // output). The shadow map is a fixed-size transient in the engine's depth
        // format so the built-in shadow pipeline and the texture agree.
        let mut graph = RenderGraph::new();
        let surface = graph.surface();
        let depth = graph.primary_depth();
        let shadow_resolution = ShadowConfig::default().resolution;
        let shadow_map = graph.transient(ResourceDesc {
            name: "shadow-map",
            format: renderer.depth_format().to_wgpu(),
            size: SizeSpec::Fixed {
                width: shadow_resolution,
                height: shadow_resolution,
            },
            kind: ResourceKind::Depth,
        });
        graph.add_pass(PassDesc {
            name: "shadow-depth",
            kind: PassKind::ShadowDepth,
            reads: Vec::new(),
            color: None,
            depth: Some(DepthWrite {
                target: shadow_map,
                clear: Some(1.0),
                write: true,
            }),
        });
        graph.add_pass(PassDesc {
            name: "forward-lit",
            kind: PassKind::ForwardLit,
            reads: vec![shadow_map],
            color: Some(ColorWrite {
                target: surface,
                clear: Some(clear_color),
            }),
            depth: Some(DepthWrite {
                target: depth,
                clear: Some(1.0),
                write: true,
            }),
        });
        graph.add_pass(PassDesc {
            name: "overlay",
            kind: PassKind::Overlay2D,
            reads: Vec::new(),
            color: Some(ColorWrite {
                target: surface,
                clear: None,
            }),
            depth: None,
        });
        let compiled = graph.compile(&renderer)?;
        Ok(Self {
            renderer,
            graph,
            compiled,
            resources,
            fonts,
            measure_font,
            ui_draw_list: DrawList::default(),
            size,
        })
    }
}

impl RenderBackend for WgpuBackend {
    fn submit(&mut self, proxies: &RenderProxies, ui: Option<&mut UiTree>) -> EngineResult<()> {
        let camera = Camera::new(proxies.camera.view, proxies.camera.projection);
        // Resolve each proxy to its GPU mesh/material; an unregistered id is
        // skipped. The draw list borrows the registry, so it is built per frame
        // (a reused buffer would be self-referential with `resources`).
        let draws: Vec<DrawItem> = proxies
            .draws
            .iter()
            .filter_map(|p| {
                self.resources
                    .resolve(p.mesh, p.material)
                    .map(|(mesh, material)| DrawItem {
                        mesh,
                        material,
                        model: p.model,
                    })
            })
            .collect();
        let overlay = match ui {
            Some(tree) => {
                let extent = spawn_core::Vec2::new(
                    self.size.width.max(1) as f32,
                    self.size.height.max(1) as f32,
                );
                tree.compute_layout(extent, &mut self.measure_font)?;
                tree.build_draw_list(&mut self.ui_draw_list)?;
                Some(Overlay {
                    tree,
                    draw_list: &self.ui_draw_list,
                    fonts: &self.fonts,
                    lines: &[],
                })
            }
            None => None,
        };
        let scene = RenderScene {
            camera: &camera,
            lighting: Some(&proxies.lighting),
            draws: &draws,
            pbr_draws: &[],
            transparent: &[],
            instances: &[],
            pbr_instances: &[],
            skinned: &[],
            pbr_skinned: &[],
            overlay,
        };
        let mut frame = self.renderer.begin_frame()?;
        frame.execute(&self.compiled, &scene)?;
        frame.end_frame()?;
        Ok(())
    }

    fn resize(&mut self, size: SurfaceSize) -> EngineResult<()> {
        self.size = size;
        self.renderer.resize(size)?;
        self.compiled.resize(&self.graph, &self.renderer)?;
        Ok(())
    }

    fn adapter_info(&self) -> Option<AdapterInfo> {
        Some(self.renderer.adapter_info())
    }

    fn apply_render_reloads(
        &mut self,
        reloads: &[ReloadEvent],
        hooks: &mut [RenderReload],
    ) -> EngineResult<()> {
        for hook in hooks.iter_mut() {
            hook(reloads, &mut self.renderer, &mut self.resources)?;
        }
        Ok(())
    }
}
