//! The frontend/backend boundary: render proxies (backend-owned plain data
//! extracted from the ECS world at the sync point), the double-buffered proxy
//! store that bounds frames-in-flight, the [`RenderBackend`] trait, and its two
//! implementations.
//!
//! Render-relevant state crosses the frame boundary only as extracted proxies; a
//! backend never reads live ECS storage during `submit`, and the frontend never
//! touches a published buffer (Finding 2: thread-owned proxies, no shared lock).

use std::sync::Arc;

use spawn_asset::AssetId;
use spawn_core::{Color, Mat4};
use spawn_platform::Window;
use spawn_render::{
    Camera, ColorWrite, CompiledGraph, DepthWrite, DrawItem, Lighting, PassDesc, PassKind,
    RenderGraph, RenderResources, RenderScene, Renderer, RendererConfig, ResourceDesc,
    ResourceKind, ShadowConfig, SizeSpec, SurfaceSize,
};

use crate::error::EngineResult;
use crate::frame::SyncMode;

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
    /// Renders one frame from the published proxy buffer.
    fn submit(&mut self, proxies: &RenderProxies) -> EngineResult<()>;
    /// Reconfigures for a new surface size.
    fn resize(&mut self, size: SurfaceSize) -> EngineResult<()>;
}

/// The engine-private double buffer. Two `RenderProxies` and an alternating
/// cursor make the frontend→backend lag structurally bounded: the backend can
/// never read more than one frame behind the frontend.
pub(crate) struct RenderProxyStore {
    buffers: [RenderProxies; 2],
    current: usize,
    in_flight: u32,
}

impl RenderProxyStore {
    pub(crate) fn new() -> Self {
        Self {
            buffers: [RenderProxies::default(), RenderProxies::default()],
            current: 0,
            in_flight: 0,
        }
    }

    /// The buffer the frontend extracts into this frame.
    pub(crate) fn back_mut(&mut self) -> &mut RenderProxies {
        &mut self.buffers[self.current]
    }

    /// The buffer the backend reads this frame: the just-extracted one in
    /// `Immediate`, the previous frame's in `Pipelined`.
    pub(crate) fn read(&self, mode: SyncMode) -> &RenderProxies {
        let index = match mode {
            SyncMode::Immediate => self.current,
            SyncMode::Pipelined => 1 - self.current,
        };
        &self.buffers[index]
    }

    /// Advances to the next frame's buffer and records frames-in-flight (`0` in
    /// `Immediate`, `1` in `Pipelined`).
    pub(crate) fn advance(&mut self, mode: SyncMode) {
        self.in_flight = match mode {
            SyncMode::Immediate => 0,
            SyncMode::Pipelined => 1,
        };
        self.current = 1 - self.current;
    }

    pub(crate) fn in_flight(&self) -> u32 {
        self.in_flight
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
    fn submit(&mut self, proxies: &RenderProxies) -> EngineResult<()> {
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
}

/// An app-provided render-setup routine: builds GPU `Mesh`/`Material` resources
/// from the renderer and registers them in the registry, run once at backend
/// construction (after the renderer exists). Headless mode has no renderer and
/// does not run these.
pub type RenderSetup = Box<dyn FnOnce(&mut Renderer, &mut RenderResources) -> EngineResult<()>>;

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
        let compiled = graph.compile(&renderer)?;
        Ok(Self {
            renderer,
            graph,
            compiled,
            resources,
        })
    }
}

impl RenderBackend for WgpuBackend {
    fn submit(&mut self, proxies: &RenderProxies) -> EngineResult<()> {
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
        let scene = RenderScene {
            camera: &camera,
            lighting: Some(&proxies.lighting),
            draws: &draws,
            overlay: None,
        };
        let mut frame = self.renderer.begin_frame()?;
        frame.execute(&self.compiled, &scene)?;
        frame.end_frame()?;
        Ok(())
    }

    fn resize(&mut self, size: SurfaceSize) -> EngineResult<()> {
        self.renderer.resize(size)?;
        self.compiled.resize(&self.graph, &self.renderer)?;
        Ok(())
    }
}
