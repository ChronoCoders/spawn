//! The `EditorApp` bespoke windowed driver and the immediate-mode editor frame.
//!
//! The driver owns the renderer, the world, the editor state, and the UI in one
//! place (editor state is not `Send`, and the editor wants immediate-mode UI), so
//! it does not use the engine's frontend/backend proxy split. See
//! `docs/specs/phase-02-spawn-editor-shell.md` §1.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use spawn_core::{Rect, Vec2};
use spawn_ecs::World;
use spawn_editor::EditorState;
use spawn_input::InputState;
use spawn_platform::{
    EventLoop, KeyCode, MouseButton, PlatformApp, PlatformEvent, Window, WindowConfig, WindowEvent,
};
use spawn_render::{
    ColorWrite, CompiledGraph, DepthWrite, DrawItem, Font, FontRegistry, Lighting, Overlay,
    PassDesc, PassKind, RenderGraph, RenderResources, RenderScene, Renderer, RendererConfig,
    ResourceDesc, ResourceKind, ShadowConfig, SizeSpec, SurfaceSize,
};
use spawn_ui::{DrawList, FontId, NodeId, Style, UiEvent, UiTree};

use crate::bridge::{self, PointerTarget};
use crate::camera::EditorCamera;
use crate::error::{ShellError, ShellResult};
use crate::gizmo::GizmoController;
use crate::inspector::{self, FieldRow};
use crate::outliner::Outliner;
use crate::panels::Panels;
use crate::scene;
use crate::theme::Theme;
use crate::toolbar::{Toolbar, ToolbarAction};
use crate::{overlay, playmode, status};

const EDITOR_FONT: FontId = FontId(1);
const FIXED_DT: f32 = 1.0 / 60.0;
const DRAG_SENSITIVITY: f32 = 0.01;

/// A world-setup hook: register components/reflection and spawn the initial scene.
pub type WorldSetup = Box<dyn FnOnce(&mut World)>;
/// A render-setup hook: build GPU mesh/material resources into the registry.
pub type RenderSetup = Box<dyn FnOnce(&mut Renderer, &mut RenderResources) -> ShellResult<()>>;
/// A play-mode step: advance the world by one fixed tick (run only while playing).
pub type PlayStep = Box<dyn FnMut(&mut World, f32)>;

/// Editor configuration: the window, theme, and setup/play hooks the host
/// provides.
pub struct EditorConfig {
    window: WindowConfig,
    theme: Theme,
    world_setup: Option<WorldSetup>,
    render_setup: Option<RenderSetup>,
    play_step: Option<PlayStep>,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl EditorConfig {
    pub fn new() -> Self {
        Self {
            window: WindowConfig::default().with_title("Spawn Editor"),
            theme: Theme::dark(),
            world_setup: None,
            render_setup: None,
            play_step: None,
        }
    }

    pub fn with_window(mut self, window: WindowConfig) -> Self {
        self.window = window;
        self
    }

    pub fn with_theme(mut self, theme: Theme) -> Self {
        self.theme = theme;
        self
    }

    /// Registers components/reflection and spawns the initial scene before launch.
    pub fn with_world_setup(mut self, setup: impl FnOnce(&mut World) + 'static) -> Self {
        self.world_setup = Some(Box::new(setup));
        self
    }

    /// Builds GPU mesh/material resources (run once after the renderer exists).
    pub fn with_render_setup(
        mut self,
        setup: impl FnOnce(&mut Renderer, &mut RenderResources) -> ShellResult<()> + 'static,
    ) -> Self {
        self.render_setup = Some(Box::new(setup));
        self
    }

    /// A per-fixed-tick world step run only while playing.
    pub fn with_play_step(mut self, step: impl FnMut(&mut World, f32) + 'static) -> Self {
        self.play_step = Some(Box::new(step));
        self
    }
}

/// The visual editor application.
pub struct EditorApp;

impl EditorApp {
    /// Runs the editor to a clean shutdown, returning any fatal error.
    pub fn run(config: EditorConfig) -> ShellResult<()> {
        let window_config = config.window.clone();
        let error: Rc<RefCell<Option<ShellError>>> = Rc::new(RefCell::new(None));
        let driver = Driver {
            config: Some(config),
            state: None,
            error: Rc::clone(&error),
        };
        EventLoop::new()?.run(window_config, driver)?;
        match Rc::try_unwrap(error).ok().and_then(RefCell::into_inner) {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}

/// The live editor state assembled once the window (and renderer) exist.
struct LiveEditor {
    renderer: Renderer<'static>,
    fonts: FontRegistry,
    graph: RenderGraph,
    compiled: CompiledGraph,
    resources: RenderResources,
    world: World,
    editor: EditorState,
    ui: UiTree,
    panels: Panels,
    toolbar: Toolbar,
    outliner: Outliner,
    status_node: NodeId,
    measure_font: Font,
    camera: EditorCamera,
    gizmo: GizmoController,
    theme: Theme,
    input: InputState,
    pending: Vec<PlatformEvent>,
    inspector_rows: Vec<FieldRow>,
    inspector_shown: Option<spawn_ecs::Entity>,
    ui_draw_list: DrawList,
    overlay_lines: Vec<spawn_render::LineSegment>,
    last_draw_count: usize,
    viewport_rect: Rect,
    prev_pointer: Vec2,
    show_grid: bool,
    accumulator: f32,
    last_instant: Option<Instant>,
    play_step: Option<PlayStep>,
    size: SurfaceSize,
}

struct Driver {
    config: Option<EditorConfig>,
    state: Option<LiveEditor>,
    error: Rc<RefCell<Option<ShellError>>>,
}

impl Driver {
    fn fail(&self, err: ShellError) {
        *self.error.borrow_mut() = Some(err);
    }
}

impl PlatformApp for Driver {
    fn init(&mut self, window: Arc<Window>) {
        let Some(config) = self.config.take() else {
            return;
        };
        match build_state(window, config) {
            Ok(state) => self.state = Some(state),
            Err(err) => self.fail(err),
        }
    }

    fn event(&mut self, _window: &Window, event: &PlatformEvent) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        match event {
            PlatformEvent::Window(WindowEvent::Resized { width, height }) => {
                if let Err(err) = state.resize(SurfaceSize::new(*width, *height)) {
                    self.fail(err);
                }
            }
            PlatformEvent::Window(WindowEvent::CloseRequested) => {
                _window.request_exit();
            }
            other => state.pending.push(*other),
        }
    }

    fn update(&mut self, window: &Window) {
        window.request_redraw();
    }

    fn redraw_requested(&mut self, window: &Window) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        if let Err(err) = state.frame() {
            self.fail(err);
            window.request_exit();
        }
    }

    fn exit(&mut self, _window: &Window) {
        self.state = None;
    }
}

fn build_state(window: Arc<Window>, config: EditorConfig) -> ShellResult<LiveEditor> {
    let (w, h) = window.size();
    let size = SurfaceSize::new(w.max(1), h.max(1));
    let mut renderer = Renderer::from_owned(window, size, RendererConfig::default())?;

    let mut resources = RenderResources::new();
    if let Some(setup) = config.render_setup {
        setup(&mut renderer, &mut resources)?;
    }
    let mut fonts = FontRegistry::new();
    let font = Font::embedded_monospace(&renderer, 8.0)?;
    fonts.insert(&renderer, EDITOR_FONT, font);
    let measure_font = Font::embedded_monospace(&renderer, 8.0)?;

    let mut world = World::new();
    world.register::<spawn_core::Transform3D>();
    world.register::<scene::Renderable>();
    if let Some(setup) = config.world_setup {
        setup(&mut world);
    }

    let theme = config.theme;
    let mut ui = UiTree::new(Style::default());
    let panels = Panels::build(&mut ui, &theme)?;
    let toolbar = Toolbar::build(&mut ui, panels.toolbar, EDITOR_FONT, &theme)?;
    let outliner = Outliner::rebuild(
        &mut ui,
        &world,
        &EditorState::new().selection,
        panels.outliner,
        EDITOR_FONT,
        &theme,
    )?;
    let status_node = status::build(&mut ui, panels.status, EDITOR_FONT, &theme)?;

    let graph = build_graph(&renderer, theme);
    let compiled = graph.compile(&renderer)?;

    Ok(LiveEditor {
        renderer,
        fonts,
        graph,
        compiled,
        resources,
        world,
        editor: EditorState::new(),
        ui,
        panels,
        toolbar,
        outliner,
        status_node,
        measure_font,
        camera: EditorCamera::default(),
        gizmo: GizmoController::default(),
        theme,
        input: InputState::new()?,
        pending: Vec::new(),
        inspector_rows: Vec::new(),
        inspector_shown: None,
        ui_draw_list: DrawList::default(),
        overlay_lines: Vec::new(),
        last_draw_count: 0,
        viewport_rect: Rect::new(Vec2::ZERO, Vec2::new(size.width as f32, size.height as f32)),
        prev_pointer: Vec2::ZERO,
        show_grid: true,
        accumulator: 0.0,
        last_instant: None,
        play_step: config.play_step,
        size,
    })
}

/// Builds the lit (shadow + forward) graph plus the overlay pass.
fn build_graph(renderer: &Renderer, theme: Theme) -> RenderGraph {
    let mut graph = RenderGraph::new();
    let surface = graph.surface();
    let depth = graph.primary_depth();
    let res = ShadowConfig::default().resolution;
    let shadow_map = graph.transient(ResourceDesc {
        name: "shadow-map",
        format: renderer.depth_format().to_wgpu(),
        size: SizeSpec::Fixed {
            width: res,
            height: res,
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
            clear: Some(theme.surface_base),
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
    graph
}

impl LiveEditor {
    fn resize(&mut self, size: SurfaceSize) -> ShellResult<()> {
        self.size = size;
        if size.is_zero() {
            return Ok(());
        }
        self.renderer.resize(size)?;
        self.compiled.resize(&self.graph, &self.renderer)?;
        Ok(())
    }

    fn frame(&mut self) -> ShellResult<()> {
        self.tick_logic()?;
        self.render()
    }

    /// All non-GPU per-frame work: input, interaction, simulation, panel refresh,
    /// layout, and overlay assembly.
    fn tick_logic(&mut self) -> ShellResult<()> {
        let dt = self.sample_dt();

        self.input.begin_frame();
        for event in self.pending.drain(..) {
            self.input.process(&event);
        }
        let ui_input = bridge::ui_input(&self.input);
        let pointer = ui_input.pointer;

        self.ui.update_input(&ui_input)?;
        let mut events: Vec<UiEvent> = Vec::new();
        self.ui.drain_events(&mut events)?;
        self.handle_toolbar(&events)?;
        self.handle_outliner(&events);

        let target = bridge::pointer_target(self.viewport_rect, pointer);
        if target == PointerTarget::Viewport {
            self.handle_viewport(pointer)?;
        } else {
            // Ensure a half-finished gizmo drag is committed if the pointer left
            // the viewport mid-gesture.
            if !self.input.mouse().is_pressed(MouseButton::Left) && self.gizmo.is_dragging() {
                self.gizmo.end(&mut self.world, &mut self.editor.commands)?;
            }
        }

        // Inspector edits (suppressed during play).
        if playmode::edits_allowed(&self.editor) {
            self.handle_inspector(&events, pointer)?;
        }

        if self.editor.is_playing() {
            self.run_simulation(dt);
        }

        // Rebuild panels on a selection / displayed-entity change.
        let primary = self.editor.selection.primary();
        if primary != self.inspector_shown {
            self.rebuild_inspector(primary)?;
            self.rebuild_outliner()?;
            self.inspector_shown = primary;
        }
        if let Some(entity) = primary {
            inspector::refresh_values(&mut self.ui, &self.world, entity, &self.inspector_rows);
        }
        self.toolbar.refresh(
            &mut self.ui,
            self.gizmo.mode(),
            self.editor.is_playing(),
            &self.theme,
        )?;
        let status_text = status::text(
            &self.world,
            self.gizmo.mode(),
            self.last_draw_count,
            self.editor.is_playing(),
        );
        self.ui.set_text(self.status_node, Some(status_text))?;

        let size = Vec2::new(
            self.size.width.max(1) as f32,
            self.size.height.max(1) as f32,
        );
        self.ui.compute_layout(size, &mut self.measure_font)?;
        self.viewport_rect = self.panels.viewport_rect(&self.ui, size);

        // Overlay line assembly: grid + selection + gizmo handles.
        overlay::assemble(
            &self.world,
            &self.editor.selection,
            &self.theme,
            self.show_grid,
            &mut self.overlay_lines,
        );
        if let Some(anchor) = self.gizmo_anchor() {
            crate::gizmo::gizmo_lines(
                self.gizmo.mode(),
                anchor,
                self.handle_len(anchor),
                self.gizmo.active_axis(),
                self.theme.accent,
                &mut self.overlay_lines,
            );
        }
        self.prev_pointer = pointer;
        Ok(())
    }

    fn handle_toolbar(&mut self, events: &[UiEvent]) -> ShellResult<()> {
        match self.toolbar.action(events) {
            Some(ToolbarAction::TogglePlay) => {
                playmode::toggle(&mut self.editor, &mut self.world)?;
                self.inspector_shown = None; // force a rebuild after a restore
            }
            Some(ToolbarAction::SetMode(mode)) => self.gizmo.set_mode(mode),
            Some(ToolbarAction::Undo) => {
                let _ = self.editor.commands.undo(&mut self.world);
                self.inspector_shown = None;
            }
            Some(ToolbarAction::Redo) => {
                let _ = self.editor.commands.redo(&mut self.world);
                self.inspector_shown = None;
            }
            None => {}
        }
        Ok(())
    }

    fn handle_outliner(&mut self, events: &[UiEvent]) {
        if let Some(entity) = self.outliner.clicked(events) {
            self.editor.selection.select(entity);
        }
    }

    fn handle_viewport(&mut self, pointer: Vec2) -> ShellResult<()> {
        let mouse = self.input.mouse();
        let aspect = (self.viewport_rect.width() / self.viewport_rect.height().max(1.0)).max(1e-3);
        let delta = mouse.delta();
        let wheel = mouse.wheel();
        let left_down = mouse.is_pressed(MouseButton::Left);
        let left_pressed = mouse.just_pressed(MouseButton::Left);
        let left_released = mouse.just_released(MouseButton::Left);
        let middle_down = mouse.is_pressed(MouseButton::Middle);
        let shift = self.input.keyboard().is_pressed(KeyCode::ShiftLeft)
            || self.input.keyboard().is_pressed(KeyCode::ShiftRight);
        let escape = self.input.keyboard().just_pressed(KeyCode::Escape);

        if middle_down && shift {
            self.camera.pan(delta);
        } else if middle_down {
            self.camera.orbit(delta);
        }
        if wheel.y != 0.0 {
            self.camera.dolly(wheel.y);
        }

        let ray = self.camera.ray(pointer, self.viewport_rect, aspect)?;

        if escape && self.gizmo.is_dragging() {
            self.gizmo
                .abort(&mut self.world, &mut self.editor.commands)?;
            return Ok(());
        }

        if !playmode::edits_allowed(&self.editor) {
            return Ok(());
        }

        if left_pressed {
            // Grab a gizmo handle if the press is on one; otherwise the press
            // becomes an entity pick on release (handled below).
            if let Some(anchor) = self.gizmo_anchor() {
                let handle_len = self.handle_len(anchor);
                if let Some(entity) = self.editor.selection.primary() {
                    self.gizmo
                        .begin(ray, anchor, handle_len, entity, &mut self.editor.commands);
                }
            }
        } else if left_down && self.gizmo.is_dragging() {
            if let Some(anchor) = self.gizmo_anchor() {
                self.gizmo
                    .update(ray, anchor, &mut self.world, &mut self.editor.commands)?;
            }
        } else if left_released {
            if self.gizmo.is_dragging() {
                self.gizmo.end(&mut self.world, &mut self.editor.commands)?;
            } else {
                match crate::picking::pick_entity(&self.world, ray) {
                    Some(entity) => {
                        self.editor.selection.select(entity);
                    }
                    None => {
                        self.editor.selection.clear();
                    }
                }
            }
        }
        Ok(())
    }

    fn handle_inspector(&mut self, events: &[UiEvent], pointer: Vec2) -> ShellResult<()> {
        let Some(entity) = self.editor.selection.primary() else {
            return Ok(());
        };
        // Snapshot the rows so the loop does not borrow self while mutating it.
        let rows = self.inspector_rows.clone();
        for row in rows {
            match row.kind {
                spawn_ecs::FieldKind::Bool => {
                    if spawn_ui::Checkbox::toggled(row.widget, events) {
                        inspector::apply_bool(
                            &mut self.editor.commands,
                            &mut self.world,
                            row,
                            entity,
                        )?;
                    }
                }
                _ => {
                    if let Some(dx) = spawn_ui::DragValue::drag_delta(
                        &self.ui,
                        row.widget,
                        self.prev_pointer,
                        pointer,
                    ) {
                        if dx != 0.0 {
                            inspector::apply_scalar_delta(
                                &mut self.editor.commands,
                                &mut self.world,
                                row,
                                entity,
                                dx * DRAG_SENSITIVITY,
                            )?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn rebuild_inspector(&mut self, primary: Option<spawn_ecs::Entity>) -> ShellResult<()> {
        self.inspector_rows = match primary {
            Some(entity) => inspector::build_rows(
                &mut self.ui,
                &self.world,
                entity,
                self.panels.inspector,
                EDITOR_FONT,
                &self.theme,
            )?,
            None => {
                crate::util::clear_children(&mut self.ui, self.panels.inspector)?;
                Vec::new()
            }
        };
        Ok(())
    }

    fn rebuild_outliner(&mut self) -> ShellResult<()> {
        self.outliner = Outliner::rebuild(
            &mut self.ui,
            &self.world,
            &self.editor.selection,
            self.panels.outliner,
            EDITOR_FONT,
            &self.theme,
        )?;
        Ok(())
    }

    fn run_simulation(&mut self, dt: f32) {
        if let Some(step) = self.play_step.as_mut() {
            self.accumulator += dt;
            let mut budget = 8;
            while self.accumulator >= FIXED_DT && budget > 0 {
                step(&mut self.world, FIXED_DT);
                self.accumulator -= FIXED_DT;
                budget -= 1;
            }
        }
    }

    /// The gizmo anchor (the primary selection's translation), if any.
    fn gizmo_anchor(&self) -> Option<spawn_core::Vec3> {
        let entity = self.editor.selection.primary()?;
        self.world
            .get::<spawn_core::Transform3D>(entity)
            .map(|t| t.translation)
    }

    /// A screen-constant-ish handle length: a fraction of the distance from the
    /// camera eye to the anchor.
    fn handle_len(&self, anchor: spawn_core::Vec3) -> f32 {
        ((self.camera.eye() - anchor).length() * 0.18).max(0.3)
    }

    fn render(&mut self) -> ShellResult<()> {
        if self.size.is_zero() {
            return Ok(());
        }
        // Build the draw list and scene geometry (disjoint field borrows). The
        // `draws` Vec borrows `self.resources`, so it is a per-frame local (the
        // same self-referential exception the engine's backend takes).
        self.ui.build_draw_list(&mut self.ui_draw_list)?;
        let mut draws: Vec<DrawItem> = Vec::new();
        scene::extract_draws(&self.world, &self.resources, &mut draws);
        self.last_draw_count = draws.len();

        let aspect = (self.viewport_rect.width() / self.viewport_rect.height().max(1.0)).max(1e-3);
        let camera = self.camera.camera(aspect)?;
        let lighting = Lighting::default();
        let overlay = Overlay {
            tree: &self.ui,
            draw_list: &self.ui_draw_list,
            fonts: &self.fonts,
            lines: &self.overlay_lines,
        };
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
            overlay: Some(overlay),
        };

        match self.renderer.begin_frame() {
            Ok(mut frame) => {
                frame.execute(&self.compiled, &scene)?;
                frame.end_frame()?;
                Ok(())
            }
            Err(spawn_render::RenderError::Surface | spawn_render::RenderError::SurfaceTimeout) => {
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    fn sample_dt(&mut self) -> f32 {
        let now = Instant::now();
        let dt = self
            .last_instant
            .map(|prev| now.duration_since(prev).as_secs_f32())
            .unwrap_or(0.0)
            .min(0.25);
        self.last_instant = Some(now);
        dt
    }
}
