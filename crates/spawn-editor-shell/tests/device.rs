//! GPU-gated end-to-end: compose the editor's public pieces (panels, inspector,
//! scene draws, selection + gizmo overlay) into a `RenderScene` and render one
//! frame through the lit + `Overlay2D` graph with no wgpu validation errors.
//!
//! Skips cleanly on a host without an adapter / display server, like the
//! spawn-render device tests (§13 gate).

use std::sync::Mutex;

use spawn_asset::AssetId;
use spawn_core::{Transform3D, Vec2, Vec3};
use spawn_ecs::World;
use spawn_editor::Selection;
use spawn_editor_shell::camera::EditorCamera;
use spawn_editor_shell::gizmo::{gizmo_lines, GizmoMode};
use spawn_editor_shell::panels::Panels;
use spawn_editor_shell::scene::{extract_draws, Renderable};
use spawn_editor_shell::{inspector, overlay, theme::Theme};
use spawn_render::{
    ColorWrite, CompareFn, CullMode, DepthWrite, DrawItem, Font, FontRegistry, Lighting, Material,
    MaterialUniform, Mesh, Overlay, PassDesc, PassKind, RenderError, RenderGraph, RenderResources,
    RenderScene, RenderStateKey, Renderer, RendererConfig, ResourceDesc, ResourceKind,
    ShaderHandle, ShadowConfig, SizeSpec, SurfaceSize, Topology, Vertex,
};
use spawn_ui::{DrawList, FontId, Style, UiTree};

const SIZE: SurfaceSize = SurfaceSize {
    width: 256,
    height: 192,
};
static WINIT_LOCK: Mutex<()> = Mutex::new(());

fn try_renderer() -> Option<(Renderer<'static>, std::sync::MutexGuard<'static, ()>)> {
    let guard = WINIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    use winit::application::ApplicationHandler;
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::platform::pump_events::EventLoopExtPumpEvents;
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

    let mut el = EventLoop::builder().with_any_thread(true).build().ok()?;
    let mut grab = Grab(None);
    let _ = el.pump_app_events(Some(std::time::Duration::from_millis(50)), &mut grab);
    let window: &'static Window = Box::leak(Box::new(grab.0?));
    let renderer = Renderer::new(window, SIZE, RendererConfig::default()).ok()?;
    Some((renderer, guard))
}

fn cube() -> (Vec<Vertex>, Vec<u32>) {
    let v = |p: [f32; 3]| Vertex {
        position: p,
        normal: [0.0, 1.0, 0.0],
        uv: [0.0, 0.0],
    };
    let verts = vec![
        v([-0.5, -0.5, 0.5]),
        v([0.5, -0.5, 0.5]),
        v([0.5, 0.5, 0.5]),
        v([-0.5, 0.5, 0.5]),
    ];
    (verts, vec![0, 1, 2, 0, 2, 3])
}

fn editor_graph(renderer: &Renderer, theme: &Theme) -> RenderGraph {
    let mut g = RenderGraph::new();
    let surface = g.surface();
    let depth = g.primary_depth();
    let res = ShadowConfig::default().resolution;
    let shadow = g.transient(ResourceDesc {
        name: "shadow-map",
        format: renderer.depth_format().to_wgpu(),
        size: SizeSpec::Fixed {
            width: res,
            height: res,
        },
        kind: ResourceKind::Depth,
    });
    g.add_pass(PassDesc {
        name: "shadow",
        kind: PassKind::ShadowDepth,
        reads: Vec::new(),
        color: None,
        depth: Some(DepthWrite {
            target: shadow,
            clear: Some(1.0),
            write: true,
        }),
    });
    g.add_pass(PassDesc {
        name: "lit",
        kind: PassKind::ForwardLit,
        reads: vec![shadow],
        color: Some(ColorWrite {
            target: surface,
            clear: Some(theme.surface_base),
        }),
        depth: Some(DepthWrite {
            target: depth,
            clear: Some(1.0),
            write: true,
        }),
    });
    g.add_pass(PassDesc {
        name: "overlay",
        kind: PassKind::Overlay2D,
        reads: Vec::new(),
        color: Some(ColorWrite {
            target: surface,
            clear: None,
        }),
        depth: None,
    });
    g
}

#[test]
fn editor_frame_composes_and_renders() {
    let Some((mut renderer, _guard)) = try_renderer() else {
        eprintln!("editor-shell: no adapter/surface; skipping (spec §13 gate)");
        return;
    };
    let theme = Theme::dark();

    // World: one reflected, renderable cube.
    let mut world = World::new();
    world.register_reflect::<Transform3D>();
    world.register::<Renderable>();
    let cube_id = AssetId::from_raw(1);
    let mat_id = AssetId::from_raw(2);
    let entity = world.spawn_with((
        Transform3D::from_translation(Vec3::ZERO),
        Renderable {
            mesh: cube_id,
            material: mat_id,
        },
    ));
    let mut selection = Selection::new();
    selection.select(entity);

    // GPU resources.
    let (verts, indices) = cube();
    let mesh = Mesh::new(renderer.device(), &verts, &indices).expect("mesh");
    let material = Material::new(
        &renderer,
        ShaderHandle::from_id(AssetId::from_raw(100)),
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
    let mut resources = RenderResources::new();
    resources.insert_mesh(cube_id, mesh);
    resources.insert_material(mat_id, material);

    let mut fonts = FontRegistry::new();
    fonts.insert(
        &renderer,
        FontId(1),
        Font::embedded_monospace(&renderer, 8.0).unwrap(),
    );
    let mut measure = Font::embedded_monospace(&renderer, 8.0).unwrap();

    // UI: panels + an inspector for the selection (exercises text + the overlay).
    let mut ui = UiTree::new(Style::default());
    let panels = Panels::build(&mut ui, &theme).unwrap();
    let _rows = inspector::build_rows(&mut ui, &world, entity, panels.inspector, FontId(1), &theme)
        .unwrap();
    ui.compute_layout(
        Vec2::new(SIZE.width as f32, SIZE.height as f32),
        &mut measure,
    )
    .unwrap();
    let mut draw_list = DrawList::default();
    ui.build_draw_list(&mut draw_list).unwrap();

    // Overlay lines: grid + selection + gizmo.
    let mut lines = Vec::new();
    overlay::assemble(&world, &selection, &theme, true, &mut lines);
    gizmo_lines(
        GizmoMode::Translate,
        Vec3::ZERO,
        2.0,
        None,
        theme.accent,
        &mut lines,
    );

    // Scene draws.
    let mut draws: Vec<DrawItem> = Vec::new();
    extract_draws(&world, &resources, &mut draws);
    assert_eq!(draws.len(), 1, "the registered cube resolves to one draw");

    let camera = EditorCamera::default()
        .camera(SIZE.width as f32 / SIZE.height as f32)
        .unwrap();
    let lighting = Lighting::default();
    let scene = RenderScene {
        camera: &camera,
        lighting: Some(&lighting),
        draws: &draws,
        pbr_draws: &[],
        transparent: &[],
        overlay: Some(Overlay {
            tree: &ui,
            draw_list: &draw_list,
            fonts: &fonts,
            lines: &lines,
        }),
    };

    let graph = editor_graph(&renderer, &theme);
    let compiled = graph.compile(&renderer).expect("compile editor graph");

    let mut frame = match renderer.begin_frame() {
        Ok(f) => f,
        Err(RenderError::Surface | RenderError::SurfaceTimeout) => {
            eprintln!("editor-shell: surface not presentable; skipping (spec §13 gate)");
            return;
        }
        Err(e) => panic!("begin_frame: {e}"),
    };
    frame
        .execute(&compiled, &scene)
        .expect("execute editor frame");
    frame.end_frame().expect("end");
}
