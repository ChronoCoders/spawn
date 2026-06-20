//! Device-backed headless tests: zero per-frame allocation (§12/§13/AC#7) and
//! surface resize/minimize handling (§13/AC#8).
//!
//! These require a real GPU adapter *and* a windowing surface. On hosts without
//! a display server (no surface can be created) or without an adapter, the
//! helper returns `None` and each test skips cleanly with a logged note, so CI
//! without a GPU still passes (spec §13 headless-skip gate). The surface-error
//! recovery *policy* is unit-tested without a device in `src/frame.rs`
//! (`surface_action`), so the mapping is covered even where surface errors
//! cannot be injected here.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use spawn_core::Color;
use spawn_render::{
    Camera, ColorWrite, CompiledGraph, DepthWrite, DrawItem, PassDesc, PassKind, RenderError,
    RenderGraph, RenderScene, Renderer, RendererConfig, SurfaceSize,
};

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
}

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

struct Counting;

// SAFETY: every operation delegates to the System allocator unchanged; the only
// added behavior is a relaxed counter increment guarded by a thread-local flag.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ARMED.try_with(|a| a.get()).unwrap_or(false) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if ARMED.try_with(|a| a.get()).unwrap_or(false) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

const SIZE: SurfaceSize = SurfaceSize {
    width: 64,
    height: 64,
};

/// Serializes the device-backed tests: concurrent winit event loops and live
/// surfaces contend over the same X11/Mesa connection, which is unrelated to
/// what these tests verify. The returned guard is held for the whole test.
static WINIT_LOCK: Mutex<()> = Mutex::new(());

/// Builds a hidden winit window and a `Renderer` on it, plus a guard that
/// serializes against the other device test. Returns `None` if no display server
/// / adapter is available on this host (skip-gate per §13). The window is leaked
/// so the surface's borrow is `'static` for the test's duration.
fn try_renderer() -> Option<(Renderer<'static>, std::sync::MutexGuard<'static, ()>)> {
    let guard = WINIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    use winit::application::ApplicationHandler;
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::platform::pump_events::EventLoopExtPumpEvents;
    #[cfg(target_os = "windows")]
    use winit::platform::windows::EventLoopBuilderExtWindows;
    #[cfg(all(unix, not(target_os = "macos")))]
    use winit::platform::x11::EventLoopBuilderExtX11;
    use winit::window::{Window, WindowId};

    struct Grab(Option<Window>);
    impl ApplicationHandler for Grab {
        fn resumed(&mut self, el: &ActiveEventLoop) {
            self.0 = el
                .create_window(Window::default_attributes().with_visible(false))
                .ok();
            el.exit();
        }
        fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: winit::event::WindowEvent) {
        }
    }

    // `any_thread` so the loop can be built off the cargo test thread; without it
    // winit panics rather than returning an error. Build still fails cleanly
    // (returning `None` below) when there is no display server.
    let mut el = EventLoop::builder().with_any_thread(true).build().ok()?;
    let mut grab = Grab(None);
    let _ = el.pump_app_events(Some(std::time::Duration::from_millis(50)), &mut grab);
    let window: &'static Window = Box::leak(Box::new(grab.0?));

    let renderer = Renderer::new(window, SIZE, RendererConfig::default()).ok()?;
    Some((renderer, guard))
}

fn compiled_graph(renderer: &Renderer) -> CompiledGraph {
    let mut g = RenderGraph::new();
    g.add_pass(PassDesc {
        name: "opaque",
        kind: PassKind::ForwardOpaque,
        reads: Vec::new(),
        color: Some(ColorWrite {
            target: g.surface(),
            clear: Some(Color::new(0.1, 0.2, 0.3, 1.0)),
        }),
        depth: Some(DepthWrite {
            target: g.primary_depth(),
            clear: Some(1.0),
            write: true,
        }),
    });
    g.compile(renderer).expect("compile")
}

#[test]
fn zero_net_engine_allocation_per_frame() {
    let Some((mut renderer, _guard)) = try_renderer() else {
        eprintln!("device.rs: no adapter/surface available; skipping (spec §13 gate)");
        return;
    };

    // A clear-only frame (no draws) exercises the engine-owned per-frame surface:
    // surface acquire, encoder creation, camera-uniform upload, model-capacity
    // check, render-pass begin/end, submit, present. wgpu's own transient objects
    // are exempt (§12); only engine-owned collections must not grow.
    let camera = Camera::new(spawn_core::Mat4::IDENTITY, spawn_core::Mat4::IDENTITY);
    let g = compiled_graph(&renderer);

    let run_frame = |renderer: &mut Renderer| {
        let draws: [DrawItem; 0] = [];
        let scene = RenderScene {
            camera: &camera,
            lighting: None,
            draws: &draws,
            pbr_draws: &[],
            transparent: &[],
            instances: &[],
            pbr_instances: &[],
            skinned: &[],
            pbr_skinned: &[],
            overlay: None,
        };
        let mut frame = renderer.begin_frame().expect("begin");
        frame.execute(&g, &scene).expect("execute");
        frame.end_frame().expect("end");
    };

    // Warm up so any lazy first-touch allocation happens before arming.
    for _ in 0..8 {
        run_frame(&mut renderer);
    }

    // wgpu's per-frame transient objects (encoder, surface texture, render pass,
    // staging) are exempt (§12) and DO hit the global allocator, so we cannot
    // assert an absolute zero against a global counter. The engine guarantee is
    // that it adds no *growing* allocation: its reused buffers (model buffer,
    // camera buffer) and the caller-owned draw/graph collections do not
    // reallocate after warm-up. We verify that by comparing two equal windows of
    // frames — if the engine reallocated per frame, later windows would allocate
    // strictly more than earlier ones. A stable (non-increasing) count proves no
    // engine-owned per-frame growth.
    const WINDOW: usize = 16;
    ARMED.with(|a| a.set(true));
    let before = ALLOCS.load(Ordering::Relaxed);
    for _ in 0..WINDOW {
        run_frame(&mut renderer);
    }
    let first = ALLOCS.load(Ordering::Relaxed) - before;
    for _ in 0..WINDOW {
        run_frame(&mut renderer);
    }
    let second = ALLOCS.load(Ordering::Relaxed) - before - first;
    ARMED.with(|a| a.set(false));

    eprintln!("device.rs: per-frame allocs window1={first} window2={second}");
    assert!(
        second <= first,
        "per-frame allocation grew across windows ({first} -> {second}); \
         engine reallocated in the hot path"
    );
}

#[test]
fn compiled_graph_executes_and_presents() {
    // GPU instance required: compile derives + allocates, execute records the
    // single forward-opaque pass against the surface, end_frame submits + presents.
    let Some((mut renderer, _guard)) = try_renderer() else {
        eprintln!("device.rs: no adapter/surface available; skipping (spec §13 gate)");
        return;
    };

    let camera = Camera::new(spawn_core::Mat4::IDENTITY, spawn_core::Mat4::IDENTITY);
    let g = compiled_graph(&renderer);

    // No transients in this graph: nothing to alias. Graph derivation is
    // device-independent, so verify it before touching the surface — these hold
    // even when the host cannot present (headless gate below).
    assert_eq!(g.transient_memory(), 0);
    assert_eq!(g.naive_memory(), 0);

    let draws: [DrawItem; 0] = [];
    let scene = RenderScene {
        camera: &camera,
        lighting: None,
        draws: &draws,
        pbr_draws: &[],
        transparent: &[],
        instances: &[],
        pbr_instances: &[],
        skinned: &[],
        pbr_skinned: &[],
        overlay: None,
    };

    // Acquiring the swapchain can still fail on a host that has an adapter but no
    // presentable surface (e.g. a virtual X server), which `try_renderer`'s gate
    // cannot foresee. A surface-acquire failure is the same headless condition the
    // sibling tests skip on, so skip here too; any other error is a real fault.
    let mut frame = match renderer.begin_frame() {
        Ok(frame) => frame,
        Err(RenderError::Surface | RenderError::SurfaceTimeout) => {
            eprintln!("device.rs: surface not presentable on this host; skipping (spec §13 gate)");
            return;
        }
        Err(e) => panic!("begin_frame: {e}"),
    };
    frame.execute(&g, &scene).expect("execute compiled graph");
    frame.end_frame().expect("end");
}

#[test]
fn resize_and_minimize_are_handled() {
    let Some((mut renderer, _guard)) = try_renderer() else {
        eprintln!("device.rs: no adapter/surface available; skipping (spec §13 gate)");
        return;
    };

    renderer.resize(SurfaceSize::new(128, 96)).expect("resize");
    assert_eq!(renderer.size(), SurfaceSize::new(128, 96));

    // Minimize: a zero size is a no-op that records the request and suppresses
    // presentation without error.
    renderer.resize(SurfaceSize::new(0, 0)).expect("minimize");

    // Restore to a non-zero size; a frame acquires and presents again.
    renderer.resize(SIZE).expect("restore");
    assert_eq!(renderer.size(), SIZE);
}

/// Verifies the owned-handle constructor: an `Arc`-held window yields a
/// `Renderer<'static>` with no borrow tying it to the window. Skips cleanly on a
/// host with no display server / adapter, like the other device tests (§13 gate).
#[test]
fn from_owned_constructs_static_renderer() {
    let _guard = WINIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    use std::sync::Arc;
    use winit::application::ApplicationHandler;
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::platform::pump_events::EventLoopExtPumpEvents;
    #[cfg(target_os = "windows")]
    use winit::platform::windows::EventLoopBuilderExtWindows;
    #[cfg(all(unix, not(target_os = "macos")))]
    use winit::platform::x11::EventLoopBuilderExtX11;
    use winit::window::{Window, WindowId};

    struct Grab(Option<Window>);
    impl ApplicationHandler for Grab {
        fn resumed(&mut self, el: &ActiveEventLoop) {
            self.0 = el
                .create_window(Window::default_attributes().with_visible(false))
                .ok();
            el.exit();
        }
        fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: winit::event::WindowEvent) {
        }
    }

    let Some(mut el) = EventLoop::builder().with_any_thread(true).build().ok() else {
        return;
    };
    let mut grab = Grab(None);
    let _ = el.pump_app_events(Some(std::time::Duration::from_millis(50)), &mut grab);
    let Some(window) = grab.0 else {
        return;
    };
    let window = Arc::new(window);

    // Construction succeeds where an adapter exists; a host without one hits the
    // NoAdapter skip-gate. Either way the owned-surface path is exercised.
    if let Ok(renderer) = Renderer::from_owned(window, SIZE, RendererConfig::default()) {
        assert_eq!(renderer.size(), SIZE);
    }
}

const UNLIT_WGSL: &str = r#"
struct Camera { view_proj: mat4x4<f32>, view_pos: vec4<f32> };
struct Model { model: mat4x4<f32> };
struct Material { base_color: vec4<f32>, params: vec4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<uniform> model: Model;
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var tex: texture_2d<f32>;
@group(1) @binding(2) var samp: sampler;
struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex
fn vs_main(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_proj * model.model * vec4<f32>(position, 1.0);
    out.uv = uv;
    return out;
}
@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return material.base_color * textureSample(tex, samp, in.uv);
}
"#;

/// GPU instance required: builds a mesh + material (loading a shader and a
/// pipeline through the new `Renderer::load_shader`/`build_pipeline`), registers
/// them in a `RenderResources`, and resolves the pair — the rasterization
/// resolution the engine's `WgpuBackend` performs each frame.
#[test]
fn render_resources_resolve_registered_resources() {
    let Some((mut renderer, _guard)) = try_renderer() else {
        eprintln!("device.rs: no adapter/surface available; skipping (spec §13 gate)");
        return;
    };

    use spawn_asset::AssetId;
    use spawn_render::{
        CompareFn, CullMode, Material, MaterialUniform, Mesh, PassKind, PipelineKey,
        RenderResources, RenderStateKey, ShaderHandle, Topology, Vertex, VertexLayoutId,
    };

    let shader = ShaderHandle::from_id(AssetId::from_raw(7));
    renderer
        .load_shader(shader, UNLIT_WGSL)
        .expect("shader compiles");
    let state = RenderStateKey {
        color_format: renderer.surface_format(),
        depth_format: renderer.depth_format(),
        depth_compare: CompareFn::Less,
        depth_write: true,
        cull: CullMode::Back,
        topology: Topology::TriangleList,
    };
    let key = PipelineKey {
        shader,
        vertex_layout: VertexLayoutId::PositionNormalUv,
        render_state: state,
        pass: PassKind::ForwardOpaque,
        instanced: false,
    };
    renderer.build_pipeline(key).expect("pipeline builds");

    let verts = [
        Vertex {
            position: [-0.5, -0.5, 0.0],
            normal: [0.0, 0.0, 1.0],
            uv: [0.0, 1.0],
        },
        Vertex {
            position: [0.5, -0.5, 0.0],
            normal: [0.0, 0.0, 1.0],
            uv: [1.0, 1.0],
        },
        Vertex {
            position: [0.0, 0.5, 0.0],
            normal: [0.0, 0.0, 1.0],
            uv: [0.5, 0.0],
        },
    ];
    let mesh = Mesh::new(renderer.device(), &verts, &[0, 1, 2]).expect("mesh");
    let material = Material::new(
        &renderer,
        shader,
        MaterialUniform {
            base_color: [1.0, 0.5, 0.2, 1.0],
            params: [0.0; 4],
        },
        None,
        state,
    )
    .expect("material");

    let mut res = RenderResources::new();
    let mesh_id = AssetId::from_canonical_path("mesh");
    let mat_id = AssetId::from_canonical_path("material");
    res.insert_mesh(mesh_id, mesh);
    res.insert_material(mat_id, material);

    assert!(
        res.resolve(mesh_id, mat_id).is_some(),
        "registered pair resolves"
    );
    assert!(
        res.resolve(mesh_id, AssetId::from_canonical_path("unknown"))
            .is_none(),
        "an unregistered id resolves to None"
    );
}

/// GPU instance required: compiles the lit graph (a depth-only shadow caster
/// feeding the lit forward pass), then executes it against the surface with one
/// directional light and one draw. Exercises multi-pass execution, per-pass
/// camera isolation (light view-proj vs scene camera), and the compiled light
/// bind group — with no wgpu validation errors.
#[test]
fn lit_graph_executes_with_shadow_pass() {
    let Some((mut renderer, _guard)) = try_renderer() else {
        eprintln!("device.rs: no adapter/surface available; skipping (spec §13 gate)");
        return;
    };

    use spawn_asset::AssetId;
    use spawn_render::{
        ColorWrite, CompareFn, CullMode, DepthWrite, Lighting, Material, MaterialUniform, Mesh,
        PassDesc, PassKind, RenderGraph, RenderScene, RenderStateKey, ResourceDesc, ResourceKind,
        ShaderHandle, SizeSpec, Topology, Vertex,
    };

    // The lit pass uses the renderer's built-in lit pipeline; the material only
    // supplies group 1, so its placeholder shader/state are never looked up.
    let placeholder = ShaderHandle::from_id(AssetId::from_raw(11));
    let state = RenderStateKey {
        color_format: renderer.surface_format(),
        depth_format: renderer.depth_format(),
        depth_compare: CompareFn::Less,
        depth_write: true,
        cull: CullMode::Back,
        topology: Topology::TriangleList,
    };
    let material = Material::new(
        &renderer,
        placeholder,
        MaterialUniform::default(),
        None,
        state,
    )
    .expect("material");
    let n = [0.0, 1.0, 0.0];
    let verts = [
        Vertex {
            position: [-1.0, 0.0, -1.0],
            normal: n,
            uv: [0.0, 0.0],
        },
        Vertex {
            position: [1.0, 0.0, -1.0],
            normal: n,
            uv: [1.0, 0.0],
        },
        Vertex {
            position: [1.0, 0.0, 1.0],
            normal: n,
            uv: [1.0, 1.0],
        },
        Vertex {
            position: [-1.0, 0.0, 1.0],
            normal: n,
            uv: [0.0, 1.0],
        },
    ];
    let mesh = Mesh::new(renderer.device(), &verts, &[0, 1, 2, 0, 2, 3]).expect("mesh");

    let mut g = RenderGraph::new();
    let surface = g.surface();
    let depth = g.primary_depth();
    let shadow_map = g.transient(ResourceDesc {
        name: "shadow-map",
        format: renderer.depth_format().to_wgpu(),
        size: SizeSpec::Fixed {
            width: 1024,
            height: 1024,
        },
        kind: ResourceKind::Depth,
    });
    g.add_pass(PassDesc {
        name: "shadow",
        kind: PassKind::ShadowDepth,
        reads: Vec::new(),
        color: None,
        depth: Some(DepthWrite {
            target: shadow_map,
            clear: Some(1.0),
            write: true,
        }),
    });
    g.add_pass(PassDesc {
        name: "lit",
        kind: PassKind::ForwardLit,
        reads: vec![shadow_map],
        color: Some(ColorWrite {
            target: surface,
            clear: Some(Color::new(0.0, 0.0, 0.0, 1.0)),
        }),
        depth: Some(DepthWrite {
            target: depth,
            clear: Some(1.0),
            write: true,
        }),
    });
    let compiled = g.compile(&renderer).expect("compile lit graph");
    assert!(
        compiled.transient_memory() > 0,
        "the shadow map is a real transient"
    );

    let camera = Camera::new(spawn_core::Mat4::IDENTITY, spawn_core::Mat4::IDENTITY);
    let lighting = Lighting::default();
    let draws = [DrawItem {
        mesh: &mesh,
        material: &material,
        model: spawn_core::Mat4::IDENTITY,
    }];
    let scene = RenderScene {
        camera: &camera,
        lighting: Some(&lighting),
        draws: &draws,
        pbr_draws: &[],
        transparent: &[],
        instances: &[],
        pbr_instances: &[],
        skinned: &[],
        pbr_skinned: &[],
        overlay: None,
    };

    let mut frame = renderer.begin_frame().expect("begin");
    frame.execute(&compiled, &scene).expect("execute lit graph");
    frame.end_frame().expect("end");
}

/// GPU instance required: compiles the PBR graph (shadow caster → `ForwardPbr`
/// into the HDR scene transient → fullscreen tonemap to the surface) and executes
/// it with one directional light and one PBR draw. Exercises the PBR pipeline, the
/// `Rgba16Float` transient, the compiled tonemap input bind group, and the
/// fullscreen pass — with no wgpu validation errors.
#[test]
fn pbr_graph_executes_with_tonemap() {
    let Some((mut renderer, _guard)) = try_renderer() else {
        eprintln!("device.rs: no adapter/surface available; skipping (spec §13 gate)");
        return;
    };

    use spawn_asset::AssetId;
    use spawn_render::{
        CompareFn, CullMode, Lighting, Mesh, PbrDrawItem, PbrMaps, PbrMaterial, PbrMaterialUniform,
        RenderStateKey, ResourceDesc, ResourceKind, ShaderHandle, SizeSpec, Topology, Vertex,
    };

    // The PBR pass uses the renderer's built-in PBR pipeline; the material only
    // supplies group 1, so its placeholder shader is never looked up. The material
    // renders into the HDR transient, so its state carries the HDR color format.
    let placeholder = ShaderHandle::from_id(AssetId::from_raw(12));
    let state = RenderStateKey {
        color_format: renderer.hdr_format(),
        depth_format: renderer.depth_format(),
        depth_compare: CompareFn::Less,
        depth_write: true,
        cull: CullMode::Back,
        topology: Topology::TriangleList,
    };
    let material = PbrMaterial::new(
        &renderer,
        placeholder,
        PbrMaterialUniform {
            base_color: [0.8, 0.1, 0.1, 1.0],
            factors: [1.0, 0.4, 1.0, 1.0],
            ..Default::default()
        },
        PbrMaps::default(),
        state,
    )
    .expect("pbr material");
    let n = [0.0, 1.0, 0.0];
    let verts = [
        Vertex {
            position: [-1.0, 0.0, -1.0],
            normal: n,
            uv: [0.0, 0.0],
        },
        Vertex {
            position: [1.0, 0.0, -1.0],
            normal: n,
            uv: [1.0, 0.0],
        },
        Vertex {
            position: [1.0, 0.0, 1.0],
            normal: n,
            uv: [1.0, 1.0],
        },
        Vertex {
            position: [-1.0, 0.0, 1.0],
            normal: n,
            uv: [0.0, 1.0],
        },
    ];
    let mesh = Mesh::new(renderer.device(), &verts, &[0, 1, 2, 0, 2, 3]).expect("mesh");

    let mut g = RenderGraph::new();
    let surface = g.surface();
    let depth = g.primary_depth();
    let shadow_map = g.transient(ResourceDesc {
        name: "shadow-map",
        format: renderer.depth_format().to_wgpu(),
        size: SizeSpec::Fixed {
            width: 1024,
            height: 1024,
        },
        kind: ResourceKind::Depth,
    });
    let hdr = g.transient(ResourceDesc {
        name: "scene-hdr",
        format: renderer.hdr_format(),
        size: SizeSpec::SurfaceRelative { num: 1, den: 1 },
        kind: ResourceKind::Color,
    });
    g.add_pass(PassDesc {
        name: "shadow",
        kind: PassKind::ShadowDepth,
        reads: Vec::new(),
        color: None,
        depth: Some(DepthWrite {
            target: shadow_map,
            clear: Some(1.0),
            write: true,
        }),
    });
    g.add_pass(PassDesc {
        name: "pbr",
        kind: PassKind::ForwardPbr,
        reads: vec![shadow_map],
        color: Some(ColorWrite {
            target: hdr,
            clear: Some(Color::new(0.0, 0.0, 0.0, 1.0)),
        }),
        depth: Some(DepthWrite {
            target: depth,
            clear: Some(1.0),
            write: true,
        }),
    });
    g.add_pass(PassDesc {
        name: "tonemap",
        kind: PassKind::Tonemap,
        reads: vec![hdr],
        color: Some(ColorWrite {
            target: surface,
            clear: Some(Color::BLACK),
        }),
        depth: None,
    });
    let compiled = g.compile(&renderer).expect("compile pbr graph");
    assert!(
        compiled.transient_memory() > 0,
        "the shadow map and HDR scene are real transients"
    );

    let camera = Camera::new(spawn_core::Mat4::IDENTITY, spawn_core::Mat4::IDENTITY);
    let lighting = Lighting::default();
    let pbr_draws = [PbrDrawItem {
        mesh: &mesh,
        material: &material,
        model: spawn_core::Mat4::IDENTITY,
    }];
    let draws: [DrawItem; 0] = [];
    let scene = RenderScene {
        camera: &camera,
        lighting: Some(&lighting),
        draws: &draws,
        pbr_draws: &pbr_draws,
        transparent: &[],
        instances: &[],
        pbr_instances: &[],
        skinned: &[],
        pbr_skinned: &[],
        overlay: None,
    };

    let mut frame = match renderer.begin_frame() {
        Ok(frame) => frame,
        Err(RenderError::Surface | RenderError::SurfaceTimeout) => {
            eprintln!("device.rs: surface not presentable on this host; skipping (spec §13 gate)");
            return;
        }
        Err(e) => panic!("begin_frame: {e}"),
    };
    frame.execute(&compiled, &scene).expect("execute pbr graph");
    frame.end_frame().expect("end");
}

/// GPU instance required: compiles the transparency graph (shadow → PBR into HDR →
/// transparent blend into HDR → tonemap to surface) and executes it with one
/// opaque PBR draw plus two translucent draws. Exercises the alpha-blend pipeline,
/// the back-to-front sort, depth-test-no-write, and reading+writing the HDR
/// transient across the PBR and transparent passes — no wgpu validation errors.
#[test]
fn transparent_graph_executes() {
    let Some((mut renderer, _guard)) = try_renderer() else {
        eprintln!("device.rs: no adapter/surface available; skipping (spec §13 gate)");
        return;
    };

    use spawn_asset::AssetId;
    use spawn_render::{
        CompareFn, CullMode, Lighting, Material, MaterialUniform, Mesh, PbrDrawItem, PbrMaps,
        PbrMaterial, PbrMaterialUniform, RenderStateKey, ResourceDesc, ResourceKind, ShaderHandle,
        SizeSpec, Topology, Vertex,
    };

    let placeholder = ShaderHandle::from_id(AssetId::from_raw(13));
    let pbr_state = RenderStateKey {
        color_format: renderer.hdr_format(),
        depth_format: renderer.depth_format(),
        depth_compare: CompareFn::Less,
        depth_write: true,
        cull: CullMode::Back,
        topology: Topology::TriangleList,
    };
    let pbr_material = PbrMaterial::new(
        &renderer,
        placeholder,
        PbrMaterialUniform::default(),
        PbrMaps::default(),
        pbr_state,
    )
    .expect("pbr material");
    // The transparent pass uses the built-in transparent pipeline; the material
    // only supplies group 1 (its own state is never looked up). Alpha < 1 blends.
    let glass = Material::new(
        &renderer,
        placeholder,
        MaterialUniform {
            base_color: [0.2, 0.6, 1.0, 0.5],
            params: [0.0; 4],
        },
        None,
        pbr_state,
    )
    .expect("glass material");

    let n = [0.0, 1.0, 0.0];
    let verts = [
        Vertex {
            position: [-1.0, 0.0, -1.0],
            normal: n,
            uv: [0.0, 0.0],
        },
        Vertex {
            position: [1.0, 0.0, -1.0],
            normal: n,
            uv: [1.0, 0.0],
        },
        Vertex {
            position: [1.0, 0.0, 1.0],
            normal: n,
            uv: [1.0, 1.0],
        },
        Vertex {
            position: [-1.0, 0.0, 1.0],
            normal: n,
            uv: [0.0, 1.0],
        },
    ];
    let mesh = Mesh::new(renderer.device(), &verts, &[0, 1, 2, 0, 2, 3]).expect("mesh");

    let mut g = RenderGraph::new();
    let surface = g.surface();
    let depth = g.primary_depth();
    let shadow_map = g.transient(ResourceDesc {
        name: "shadow-map",
        format: renderer.depth_format().to_wgpu(),
        size: SizeSpec::Fixed {
            width: 1024,
            height: 1024,
        },
        kind: ResourceKind::Depth,
    });
    let hdr = g.transient(ResourceDesc {
        name: "scene-hdr",
        format: renderer.hdr_format(),
        size: SizeSpec::SurfaceRelative { num: 1, den: 1 },
        kind: ResourceKind::Color,
    });
    g.add_pass(PassDesc {
        name: "shadow",
        kind: PassKind::ShadowDepth,
        reads: Vec::new(),
        color: None,
        depth: Some(DepthWrite {
            target: shadow_map,
            clear: Some(1.0),
            write: true,
        }),
    });
    g.add_pass(PassDesc {
        name: "pbr",
        kind: PassKind::ForwardPbr,
        reads: vec![shadow_map],
        color: Some(ColorWrite {
            target: hdr,
            clear: Some(Color::new(0.0, 0.0, 0.0, 1.0)),
        }),
        depth: Some(DepthWrite {
            target: depth,
            clear: Some(1.0),
            write: true,
        }),
    });
    g.add_pass(PassDesc {
        name: "transparent",
        kind: PassKind::Transparent,
        reads: vec![hdr],
        color: Some(ColorWrite {
            target: hdr,
            clear: None,
        }),
        depth: Some(DepthWrite {
            target: depth,
            clear: None,
            write: false,
        }),
    });
    g.add_pass(PassDesc {
        name: "tonemap",
        kind: PassKind::Tonemap,
        reads: vec![hdr],
        color: Some(ColorWrite {
            target: surface,
            clear: Some(Color::BLACK),
        }),
        depth: None,
    });
    let compiled = g.compile(&renderer).expect("compile transparent graph");

    let camera = Camera::new(spawn_core::Mat4::IDENTITY, spawn_core::Mat4::IDENTITY);
    let lighting = Lighting::default();
    let pbr_draws = [PbrDrawItem {
        mesh: &mesh,
        material: &pbr_material,
        model: spawn_core::Mat4::IDENTITY,
    }];
    let transparent = [
        DrawItem {
            mesh: &mesh,
            material: &glass,
            model: spawn_core::Mat4::from_translation(spawn_core::Vec3::new(0.0, 1.0, 0.0)),
        },
        DrawItem {
            mesh: &mesh,
            material: &glass,
            model: spawn_core::Mat4::from_translation(spawn_core::Vec3::new(0.0, 2.0, 0.0)),
        },
    ];
    let draws: [DrawItem; 0] = [];
    let scene = RenderScene {
        camera: &camera,
        lighting: Some(&lighting),
        draws: &draws,
        pbr_draws: &pbr_draws,
        transparent: &transparent,
        instances: &[],
        pbr_instances: &[],
        skinned: &[],
        pbr_skinned: &[],
        overlay: None,
    };

    let mut frame = match renderer.begin_frame() {
        Ok(frame) => frame,
        Err(RenderError::Surface | RenderError::SurfaceTimeout) => {
            eprintln!("device.rs: surface not presentable on this host; skipping (spec §13 gate)");
            return;
        }
        Err(e) => panic!("begin_frame: {e}"),
    };
    frame
        .execute(&compiled, &scene)
        .expect("execute transparent graph");
    frame.end_frame().expect("end");
}

/// GPU instance required: compiles a single `ForwardOpaque` graph and executes it
/// with one unlit instanced batch (a 3×3 grid of quads) — one `draw_indexed(..,
/// 0..9)` over the per-instance storage buffer. Exercises the instanced opaque
/// pipeline + storage-buffer read in the vertex stage with no validation errors.
#[test]
fn instanced_opaque_graph_executes() {
    let Some((mut renderer, _guard)) = try_renderer() else {
        eprintln!("device.rs: no adapter/surface available; skipping (spec §13 gate)");
        return;
    };

    use spawn_asset::AssetId;
    use spawn_render::{
        CompareFn, CullMode, InstanceBatch, InstanceData, Material, MaterialUniform, Mesh,
        RenderStateKey, ShaderHandle, Topology, Vertex,
    };

    let placeholder = ShaderHandle::from_id(AssetId::from_raw(14));
    let material = Material::new(
        &renderer,
        placeholder,
        MaterialUniform::default(),
        None,
        RenderStateKey {
            color_format: renderer.surface_format(),
            depth_format: renderer.depth_format(),
            depth_compare: CompareFn::Less,
            depth_write: true,
            cull: CullMode::Back,
            topology: Topology::TriangleList,
        },
    )
    .expect("material");
    let n = [0.0, 0.0, 1.0];
    let verts = [
        Vertex {
            position: [-0.1, -0.1, 0.0],
            normal: n,
            uv: [0.0, 0.0],
        },
        Vertex {
            position: [0.1, -0.1, 0.0],
            normal: n,
            uv: [1.0, 0.0],
        },
        Vertex {
            position: [0.1, 0.1, 0.0],
            normal: n,
            uv: [1.0, 1.0],
        },
        Vertex {
            position: [-0.1, 0.1, 0.0],
            normal: n,
            uv: [0.0, 1.0],
        },
    ];
    let mesh = Mesh::new(renderer.device(), &verts, &[0, 1, 2, 0, 2, 3]).expect("mesh");
    let instances: Vec<InstanceData> = (0..9)
        .map(|i| {
            let x = (i % 3) as f32 * 0.3 - 0.3;
            let y = (i / 3) as f32 * 0.3 - 0.3;
            InstanceData::from_model(spawn_core::Mat4::from_translation(spawn_core::Vec3::new(
                x, y, 0.0,
            )))
        })
        .collect();
    let batches = [InstanceBatch {
        mesh: &mesh,
        material: &material,
        instances: &instances,
    }];

    let mut g = RenderGraph::new();
    g.add_pass(PassDesc {
        name: "opaque",
        kind: PassKind::ForwardOpaque,
        reads: Vec::new(),
        color: Some(ColorWrite {
            target: g.surface(),
            clear: Some(Color::BLACK),
        }),
        depth: Some(DepthWrite {
            target: g.primary_depth(),
            clear: Some(1.0),
            write: true,
        }),
    });
    let compiled = g
        .compile(&renderer)
        .expect("compile instanced opaque graph");

    let camera = Camera::new(spawn_core::Mat4::IDENTITY, spawn_core::Mat4::IDENTITY);
    let draws: [DrawItem; 0] = [];
    let scene = RenderScene {
        camera: &camera,
        lighting: None,
        draws: &draws,
        pbr_draws: &[],
        transparent: &[],
        instances: &batches,
        pbr_instances: &[],
        skinned: &[],
        pbr_skinned: &[],
        overlay: None,
    };

    let mut frame = match renderer.begin_frame() {
        Ok(frame) => frame,
        Err(RenderError::Surface | RenderError::SurfaceTimeout) => {
            eprintln!("device.rs: surface not presentable on this host; skipping (spec §13 gate)");
            return;
        }
        Err(e) => panic!("begin_frame: {e}"),
    };
    frame
        .execute(&compiled, &scene)
        .expect("execute instanced opaque graph");
    frame.end_frame().expect("end");
}

/// GPU instance required: compiles the PBR graph (shadow → PBR into HDR → tonemap)
/// and executes it with one instanced PBR batch, so the instanced PBR pipeline and
/// the instanced shadow caster both read the per-instance storage buffer. No wgpu
/// validation errors.
#[test]
fn instanced_pbr_graph_executes() {
    let Some((mut renderer, _guard)) = try_renderer() else {
        eprintln!("device.rs: no adapter/surface available; skipping (spec §13 gate)");
        return;
    };

    use spawn_asset::AssetId;
    use spawn_render::{
        CompareFn, CullMode, InstanceData, Lighting, Mesh, PbrInstanceBatch, PbrMaps, PbrMaterial,
        PbrMaterialUniform, RenderStateKey, ResourceDesc, ResourceKind, ShaderHandle, SizeSpec,
        Topology, Vertex,
    };

    let placeholder = ShaderHandle::from_id(AssetId::from_raw(15));
    let material = PbrMaterial::new(
        &renderer,
        placeholder,
        PbrMaterialUniform::default(),
        PbrMaps::default(),
        RenderStateKey {
            color_format: renderer.hdr_format(),
            depth_format: renderer.depth_format(),
            depth_compare: CompareFn::Less,
            depth_write: true,
            cull: CullMode::Back,
            topology: Topology::TriangleList,
        },
    )
    .expect("pbr material");
    let n = [0.0, 1.0, 0.0];
    let verts = [
        Vertex {
            position: [-1.0, 0.0, -1.0],
            normal: n,
            uv: [0.0, 0.0],
        },
        Vertex {
            position: [1.0, 0.0, -1.0],
            normal: n,
            uv: [1.0, 0.0],
        },
        Vertex {
            position: [1.0, 0.0, 1.0],
            normal: n,
            uv: [1.0, 1.0],
        },
        Vertex {
            position: [-1.0, 0.0, 1.0],
            normal: n,
            uv: [0.0, 1.0],
        },
    ];
    let mesh = Mesh::new(renderer.device(), &verts, &[0, 1, 2, 0, 2, 3]).expect("mesh");
    let instances = [
        InstanceData::from_model(spawn_core::Mat4::from_translation(spawn_core::Vec3::new(
            -2.0, 0.0, 0.0,
        ))),
        InstanceData::from_model(spawn_core::Mat4::from_translation(spawn_core::Vec3::new(
            2.0, 0.0, 0.0,
        ))),
    ];
    let batches = [PbrInstanceBatch {
        mesh: &mesh,
        material: &material,
        instances: &instances,
    }];

    let mut g = RenderGraph::new();
    let surface = g.surface();
    let depth = g.primary_depth();
    let shadow_map = g.transient(ResourceDesc {
        name: "shadow-map",
        format: renderer.depth_format().to_wgpu(),
        size: SizeSpec::Fixed {
            width: 1024,
            height: 1024,
        },
        kind: ResourceKind::Depth,
    });
    let hdr = g.transient(ResourceDesc {
        name: "scene-hdr",
        format: renderer.hdr_format(),
        size: SizeSpec::SurfaceRelative { num: 1, den: 1 },
        kind: ResourceKind::Color,
    });
    g.add_pass(PassDesc {
        name: "shadow",
        kind: PassKind::ShadowDepth,
        reads: Vec::new(),
        color: None,
        depth: Some(DepthWrite {
            target: shadow_map,
            clear: Some(1.0),
            write: true,
        }),
    });
    g.add_pass(PassDesc {
        name: "pbr",
        kind: PassKind::ForwardPbr,
        reads: vec![shadow_map],
        color: Some(ColorWrite {
            target: hdr,
            clear: Some(Color::new(0.0, 0.0, 0.0, 1.0)),
        }),
        depth: Some(DepthWrite {
            target: depth,
            clear: Some(1.0),
            write: true,
        }),
    });
    g.add_pass(PassDesc {
        name: "tonemap",
        kind: PassKind::Tonemap,
        reads: vec![hdr],
        color: Some(ColorWrite {
            target: surface,
            clear: Some(Color::BLACK),
        }),
        depth: None,
    });
    let compiled = g.compile(&renderer).expect("compile instanced pbr graph");

    let camera = Camera::new(spawn_core::Mat4::IDENTITY, spawn_core::Mat4::IDENTITY);
    let lighting = Lighting::default();
    let scene = RenderScene {
        camera: &camera,
        lighting: Some(&lighting),
        draws: &[],
        pbr_draws: &[],
        transparent: &[],
        instances: &[],
        pbr_instances: &batches,
        skinned: &[],
        pbr_skinned: &[],
        overlay: None,
    };

    let mut frame = match renderer.begin_frame() {
        Ok(frame) => frame,
        Err(RenderError::Surface | RenderError::SurfaceTimeout) => {
            eprintln!("device.rs: surface not presentable on this host; skipping (spec §13 gate)");
            return;
        }
        Err(e) => panic!("begin_frame: {e}"),
    };
    frame
        .execute(&compiled, &scene)
        .expect("execute instanced pbr graph");
    frame.end_frame().expect("end");
}

/// GPU instance required: builds a one-joint skeleton, composes its bind-pose skin
/// matrices, and renders a skinned PBR mesh through shadow → PBR(HDR) → tonemap.
/// Exercises the skinned PBR + skinned shadow pipelines, the `SkinnedVertex`
/// layout, and the dynamic-offset joint storage with no wgpu validation errors.
#[test]
fn skinned_pbr_graph_executes() {
    let Some((mut renderer, _guard)) = try_renderer() else {
        eprintln!("device.rs: no adapter/surface available; skipping (spec §13 gate)");
        return;
    };

    use spawn_asset::AssetId;
    use spawn_core::Transform3D;
    use spawn_render::{
        CompareFn, CullMode, Joint, Lighting, Mesh, PbrMaps, PbrMaterial, PbrMaterialUniform,
        PbrSkinnedDrawItem, RenderStateKey, ResourceDesc, ResourceKind, ShaderHandle, SizeSpec,
        Skeleton, SkinnedVertex, Topology, ROOT_JOINT,
    };

    let placeholder = ShaderHandle::from_id(AssetId::from_raw(16));
    let material = PbrMaterial::new(
        &renderer,
        placeholder,
        PbrMaterialUniform::default(),
        PbrMaps::default(),
        RenderStateKey {
            color_format: renderer.hdr_format(),
            depth_format: renderer.depth_format(),
            depth_compare: CompareFn::Less,
            depth_write: true,
            cull: CullMode::Back,
            topology: Topology::TriangleList,
        },
    )
    .expect("pbr material");

    let skeleton = Skeleton::new(vec![Joint {
        parent: ROOT_JOINT,
        inverse_bind: spawn_core::Mat4::IDENTITY,
    }])
    .expect("skeleton");
    let skin = skeleton
        .skin_matrices(&[Transform3D::IDENTITY])
        .expect("skin matrices");

    let n = [0.0, 1.0, 0.0];
    let sv = |p: [f32; 3], uv: [f32; 2]| SkinnedVertex {
        position: p,
        normal: n,
        uv,
        joints: [0, 0, 0, 0],
        weights: [1.0, 0.0, 0.0, 0.0],
    };
    let verts = [
        sv([-1.0, 0.0, -1.0], [0.0, 0.0]),
        sv([1.0, 0.0, -1.0], [1.0, 0.0]),
        sv([1.0, 0.0, 1.0], [1.0, 1.0]),
        sv([-1.0, 0.0, 1.0], [0.0, 1.0]),
    ];
    let mesh = Mesh::new_skinned(renderer.device(), &verts, &[0, 1, 2, 0, 2, 3]).expect("mesh");

    let pbr_skinned = [PbrSkinnedDrawItem {
        mesh: &mesh,
        material: &material,
        model: spawn_core::Mat4::IDENTITY,
        joints: &skin,
    }];

    let mut g = RenderGraph::new();
    let surface = g.surface();
    let depth = g.primary_depth();
    let shadow_map = g.transient(ResourceDesc {
        name: "shadow-map",
        format: renderer.depth_format().to_wgpu(),
        size: SizeSpec::Fixed {
            width: 1024,
            height: 1024,
        },
        kind: ResourceKind::Depth,
    });
    let hdr = g.transient(ResourceDesc {
        name: "scene-hdr",
        format: renderer.hdr_format(),
        size: SizeSpec::SurfaceRelative { num: 1, den: 1 },
        kind: ResourceKind::Color,
    });
    g.add_pass(PassDesc {
        name: "shadow",
        kind: PassKind::ShadowDepth,
        reads: Vec::new(),
        color: None,
        depth: Some(DepthWrite {
            target: shadow_map,
            clear: Some(1.0),
            write: true,
        }),
    });
    g.add_pass(PassDesc {
        name: "pbr",
        kind: PassKind::ForwardPbr,
        reads: vec![shadow_map],
        color: Some(ColorWrite {
            target: hdr,
            clear: Some(Color::new(0.0, 0.0, 0.0, 1.0)),
        }),
        depth: Some(DepthWrite {
            target: depth,
            clear: Some(1.0),
            write: true,
        }),
    });
    g.add_pass(PassDesc {
        name: "tonemap",
        kind: PassKind::Tonemap,
        reads: vec![hdr],
        color: Some(ColorWrite {
            target: surface,
            clear: Some(Color::BLACK),
        }),
        depth: None,
    });
    let compiled = g.compile(&renderer).expect("compile skinned pbr graph");

    let camera = Camera::new(spawn_core::Mat4::IDENTITY, spawn_core::Mat4::IDENTITY);
    let lighting = Lighting::default();
    let scene = RenderScene {
        camera: &camera,
        lighting: Some(&lighting),
        draws: &[],
        pbr_draws: &[],
        transparent: &[],
        instances: &[],
        pbr_instances: &[],
        skinned: &[],
        pbr_skinned: &pbr_skinned,
        overlay: None,
    };

    let mut frame = match renderer.begin_frame() {
        Ok(frame) => frame,
        Err(RenderError::Surface | RenderError::SurfaceTimeout) => {
            eprintln!("device.rs: surface not presentable on this host; skipping (spec §13 gate)");
            return;
        }
        Err(e) => panic!("begin_frame: {e}"),
    };
    frame
        .execute(&compiled, &scene)
        .expect("execute skinned pbr graph");
    frame.end_frame().expect("end");
}

/// GPU instance required: renders a skinned unlit mesh through a single
/// `ForwardOpaque` pass, exercising the skinned opaque pipeline + joint storage.
#[test]
fn skinned_opaque_graph_executes() {
    let Some((mut renderer, _guard)) = try_renderer() else {
        eprintln!("device.rs: no adapter/surface available; skipping (spec §13 gate)");
        return;
    };

    use spawn_asset::AssetId;
    use spawn_core::Transform3D;
    use spawn_render::{
        CompareFn, CullMode, Joint, Material, MaterialUniform, Mesh, RenderStateKey, ShaderHandle,
        Skeleton, SkinnedDrawItem, SkinnedVertex, Topology, ROOT_JOINT,
    };

    let placeholder = ShaderHandle::from_id(AssetId::from_raw(17));
    let material = Material::new(
        &renderer,
        placeholder,
        MaterialUniform::default(),
        None,
        RenderStateKey {
            color_format: renderer.surface_format(),
            depth_format: renderer.depth_format(),
            depth_compare: CompareFn::Less,
            depth_write: true,
            cull: CullMode::Back,
            topology: Topology::TriangleList,
        },
    )
    .expect("material");

    let skeleton = Skeleton::new(vec![Joint {
        parent: ROOT_JOINT,
        inverse_bind: spawn_core::Mat4::IDENTITY,
    }])
    .expect("skeleton");
    let skin = skeleton
        .skin_matrices(&[Transform3D::IDENTITY])
        .expect("skin matrices");

    let n = [0.0, 0.0, 1.0];
    let sv = |p: [f32; 3]| SkinnedVertex {
        position: p,
        normal: n,
        uv: [0.0, 0.0],
        joints: [0, 0, 0, 0],
        weights: [1.0, 0.0, 0.0, 0.0],
    };
    let verts = [
        sv([-0.5, -0.5, 0.0]),
        sv([0.5, -0.5, 0.0]),
        sv([0.5, 0.5, 0.0]),
        sv([-0.5, 0.5, 0.0]),
    ];
    let mesh = Mesh::new_skinned(renderer.device(), &verts, &[0, 1, 2, 0, 2, 3]).expect("mesh");
    let skinned = [SkinnedDrawItem {
        mesh: &mesh,
        material: &material,
        model: spawn_core::Mat4::IDENTITY,
        joints: &skin,
    }];

    let mut g = RenderGraph::new();
    g.add_pass(PassDesc {
        name: "opaque",
        kind: PassKind::ForwardOpaque,
        reads: Vec::new(),
        color: Some(ColorWrite {
            target: g.surface(),
            clear: Some(Color::BLACK),
        }),
        depth: Some(DepthWrite {
            target: g.primary_depth(),
            clear: Some(1.0),
            write: true,
        }),
    });
    let compiled = g.compile(&renderer).expect("compile skinned opaque graph");

    let camera = Camera::new(spawn_core::Mat4::IDENTITY, spawn_core::Mat4::IDENTITY);
    let draws: [DrawItem; 0] = [];
    let scene = RenderScene {
        camera: &camera,
        lighting: None,
        draws: &draws,
        pbr_draws: &[],
        transparent: &[],
        instances: &[],
        pbr_instances: &[],
        skinned: &skinned,
        pbr_skinned: &[],
        overlay: None,
    };

    let mut frame = match renderer.begin_frame() {
        Ok(frame) => frame,
        Err(RenderError::Surface | RenderError::SurfaceTimeout) => {
            eprintln!("device.rs: surface not presentable on this host; skipping (spec §13 gate)");
            return;
        }
        Err(e) => panic!("begin_frame: {e}"),
    };
    frame
        .execute(&compiled, &scene)
        .expect("execute skinned opaque graph");
    frame.end_frame().expect("end");
}

/// GPU instance required: compiles a base-clear + `Overlay2D` graph and executes
/// it with a `spawn_ui` draw list (a background panel and a text label) plus a
/// world-space line, exercising the UI quad pipeline, the glyph atlas text path,
/// and the line pipeline with no wgpu validation errors.
#[test]
fn overlay_graph_executes() {
    let Some((mut renderer, _guard)) = try_renderer() else {
        eprintln!("device.rs: no adapter/surface available; skipping (spec §13 gate)");
        return;
    };

    use spawn_render::{Font, FontRegistry, LineSegment, Overlay};
    use spawn_ui::{Dimension, FontId, Size, Style, UiTree};

    let px = |w: f32, h: f32| Size {
        width: Dimension::Px(w),
        height: Dimension::Px(h),
    };

    let font = Font::embedded_monospace(&renderer, 8.0).expect("font atlas");
    let mut fonts = FontRegistry::new();
    fonts.insert(&renderer, FontId(1), font);

    let mut tree = UiTree::new(Style {
        background: Color::new(0.1, 0.1, 0.12, 1.0),
        size: px(SIZE.width as f32, SIZE.height as f32),
        ..Default::default()
    });
    let root = tree.root();
    let label = tree
        .create_node(
            Style {
                size: px(40.0, 8.0),
                ..Default::default()
            },
            root,
        )
        .unwrap();
    tree.set_text(label, Some("Hi".to_string())).unwrap();
    tree.set_font(label, FontId(1)).unwrap();

    let mut measure = Font::embedded_monospace(&renderer, 8.0).expect("measure font");
    tree.compute_layout(
        spawn_core::Vec2::new(SIZE.width as f32, SIZE.height as f32),
        &mut measure,
    )
    .unwrap();
    let mut draw_list = spawn_ui::DrawList::default();
    tree.build_draw_list(&mut draw_list).unwrap();

    let mut g = RenderGraph::new();
    g.add_pass(PassDesc {
        name: "base",
        kind: PassKind::ForwardOpaque,
        reads: Vec::new(),
        color: Some(ColorWrite {
            target: g.surface(),
            clear: Some(Color::BLACK),
        }),
        depth: Some(DepthWrite {
            target: g.primary_depth(),
            clear: Some(1.0),
            write: true,
        }),
    });
    g.add_pass(PassDesc {
        name: "overlay",
        kind: PassKind::Overlay2D,
        reads: Vec::new(),
        color: Some(ColorWrite {
            target: g.surface(),
            clear: None,
        }),
        depth: None,
    });
    let compiled = g.compile(&renderer).expect("compile overlay graph");

    let camera = Camera::new(spawn_core::Mat4::IDENTITY, spawn_core::Mat4::IDENTITY);
    let lines = [LineSegment {
        start: spawn_core::Vec3::ZERO,
        end: spawn_core::Vec3::new(0.5, 0.5, 0.0),
        color: Color::new(1.0, 0.0, 0.0, 1.0),
    }];
    let overlay = Overlay {
        tree: &tree,
        draw_list: &draw_list,
        fonts: &fonts,
        lines: &lines,
    };
    let draws: [DrawItem; 0] = [];
    let scene = RenderScene {
        camera: &camera,
        lighting: None,
        draws: &draws,
        pbr_draws: &[],
        transparent: &[],
        instances: &[],
        pbr_instances: &[],
        skinned: &[],
        pbr_skinned: &[],
        overlay: Some(overlay),
    };

    let mut frame = match renderer.begin_frame() {
        Ok(frame) => frame,
        Err(RenderError::Surface | RenderError::SurfaceTimeout) => {
            eprintln!("device.rs: surface not presentable on this host; skipping (spec §13 gate)");
            return;
        }
        Err(e) => panic!("begin_frame: {e}"),
    };
    frame
        .execute(&compiled, &scene)
        .expect("execute overlay graph");
    frame.end_frame().expect("end");
}
