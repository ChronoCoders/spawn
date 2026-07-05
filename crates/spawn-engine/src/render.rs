//! The frontend/backend boundary: render proxies (backend-owned plain data
//! extracted from the ECS world at the sync point), the [`RenderExecutor`] seam
//! that publishes a filled proxy buffer by ownership (returning a recycled empty
//! one), the [`RenderBackend`] trait, and its two implementations.
//!
//! Render-relevant state crosses the frame boundary only as extracted proxies; a
//! backend never reads live ECS storage during `submit`, and the frontend never
//! touches a buffer it has handed off (Finding 2: single-owner proxies, no shared
//! lock).

use std::sync::Arc;

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
/// by [`WgpuBackend`] (real GPU) and [`HeadlessBackend`] (no GPU).
pub trait RenderBackend {
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
    /// hooks for `reloads` first and compositing `ui` when present. Returns the
    /// recycled (cleared) buffer to extract into next frame plus the frame's
    /// [`RenderReport`]. A per-frame backend error is carried in the report; a
    /// transport failure (render thread gone) is an `Err`.
    fn submit(
        &mut self,
        filled: RenderProxies,
        reloads: &[ReloadEvent],
        ui: Option<&mut UiTree>,
        mode: SyncMode,
    ) -> EngineResult<(RenderProxies, RenderReport)>;
    /// Reconfigures the backend for a new surface size.
    fn resize(&mut self, size: SurfaceSize) -> EngineResult<()>;
    /// Extractions submitted but not yet rendered: `0` for the inline executor.
    fn frames_in_flight(&self) -> u32;
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
        reloads: &[ReloadEvent],
        ui: Option<&mut UiTree>,
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
                .apply_render_reloads(reloads, &mut self.reloads)
            {
                report.error = Some(e);
                filled.reset();
                return Ok((filled, report));
            }
        }
        if let Err(e) = self.backend.submit(&filled, ui) {
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
pub type RenderSetup = Box<dyn FnOnce(&mut Renderer, &mut RenderResources) -> EngineResult<()>>;

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
