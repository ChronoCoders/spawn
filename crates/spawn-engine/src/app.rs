//! The [`App`] configuration aggregate and the windowed/headless drivers.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use spawn_core::Color;
use spawn_ecs::system::BuildableSystem;
use spawn_ecs::{EcsResult, Event, IntoSystem, Resource, Stage, World};
use spawn_platform::{EventLoop, PlatformApp, PlatformEvent, Window, WindowEvent};
use spawn_render::{RenderResources, Renderer, RendererConfig, SurfaceSize};

use crate::config::EngineConfig;
use crate::engine::{Clock, Engine, EngineParts};
use crate::error::{EngineError, EngineResult};
use crate::frame::ScheduleLabel;
use crate::render::{HeadlessBackend, RenderBackend, RenderProxies, WgpuBackend};
use crate::time::Time;

/// The configuration aggregate: the world, the variable- and fixed-rate
/// schedules, startup work, exclusive fixed hooks, proxy extraction, and the
/// engine configuration. Consumed by [`run`](App::run) / [`run_headless`](App::run_headless).
pub struct App {
    world: World,
    var_stages: Vec<Stage>,
    fixed_stages: Vec<Stage>,
    startup_stage: Stage,
    fixed_hooks: Vec<crate::engine::FixedHook>,
    extracts: Vec<crate::engine::ExtractFn>,
    render_setups: Vec<crate::render::RenderSetup>,
    config: EngineConfig,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

fn labelled_stages() -> Vec<Stage> {
    ScheduleLabel::ALL
        .iter()
        .map(|label| Stage::new(label.name()))
        .collect()
}

impl App {
    /// An empty app with default configuration.
    pub fn new() -> Self {
        Self {
            world: World::new(),
            var_stages: labelled_stages(),
            fixed_stages: labelled_stages(),
            startup_stage: Stage::new("startup"),
            fixed_hooks: Vec::new(),
            extracts: Vec::new(),
            render_setups: Vec::new(),
            config: EngineConfig::default(),
        }
    }

    /// Direct world access for setup (registering components, spawning entities,
    /// inserting resources) before launch.
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// Inserts a startup resource.
    pub fn insert_resource<R: Resource>(&mut self, value: R) -> &mut Self {
        self.world.insert_resource(value);
        self
    }

    /// Registers an event type used by systems.
    pub fn add_event<E: Event>(&mut self) -> &mut Self {
        self.world.init_event::<E>();
        self
    }

    /// Adds a system that runs once, after init, before the first frame.
    pub fn add_startup_system<P, S>(&mut self, system: S) -> &mut Self
    where
        S: IntoSystem<P>,
        S::Sys: BuildableSystem,
    {
        self.startup_stage.add_system(system);
        self
    }

    /// Adds a variable-rate system to the named stage (runs once per frame).
    pub fn add_system<P, S>(&mut self, stage: ScheduleLabel, system: S) -> &mut Self
    where
        S: IntoSystem<P>,
        S::Sys: BuildableSystem,
    {
        self.var_stages[stage.index()].add_system(system);
        self
    }

    /// Adds a fixed-rate system to the named stage (runs once per fixed tick).
    pub fn add_fixed_system<P, S>(&mut self, stage: ScheduleLabel, system: S) -> &mut Self
    where
        S: IntoSystem<P>,
        S::Sys: BuildableSystem,
    {
        self.fixed_stages[stage.index()].add_system(system);
        self
    }

    /// Registers an exclusive `&mut World` fixed-step hook (where physics is
    /// wired). Run once per fixed tick in registration order, after the fixed
    /// schedule.
    pub fn add_fixed_hook<F>(&mut self, hook: F) -> &mut Self
    where
        F: FnMut(&mut World, &Time) -> EcsResult<()> + Send + 'static,
    {
        self.fixed_hooks.push(Box::new(hook));
        self
    }

    /// Registers a proxy-extraction routine, run at the sync point each frame.
    pub fn add_extract<F>(&mut self, extract: F) -> &mut Self
    where
        F: FnMut(&World, &mut RenderProxies) + Send + 'static,
    {
        self.extracts.push(Box::new(extract));
        self
    }

    /// Registers a render-setup hook: builds GPU mesh/material resources from the
    /// renderer and registers them in the [`RenderResources`] registry, run once
    /// when the wgpu backend is created (windowed mode). The headless backend has
    /// no renderer, so these are not run there.
    pub fn add_render_setup<F>(&mut self, setup: F) -> &mut Self
    where
        F: FnOnce(&mut Renderer, &mut RenderResources) -> EngineResult<()> + 'static,
    {
        self.render_setups.push(Box::new(setup));
        self
    }

    /// Replaces the engine configuration.
    pub fn set_config(&mut self, config: EngineConfig) -> &mut Self {
        self.config = config;
        self
    }

    /// Runs the windowed driver: creates the platform event loop and runs the
    /// full loop to a clean shutdown, returning when the loop exits.
    pub fn run(self) -> EngineResult<()> {
        if self.config.fixed_timestep <= 0.0 {
            return Err(EngineError::InvalidConfig {
                reason: "fixed_timestep must be > 0",
            });
        }
        let window_config = self.config.window.clone();
        let parts = self.into_parts();
        let error: Rc<RefCell<Option<EngineError>>> = Rc::new(RefCell::new(None));
        let driver = WindowedDriver {
            parts: Some(parts),
            engine: None,
            error: Rc::clone(&error),
        };
        let event_loop = EventLoop::new()?;
        event_loop.run(window_config, driver)?;
        match Rc::try_unwrap(error).ok().and_then(RefCell::into_inner) {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    /// Runs the headless driver: no window/surface/GPU; ticks `frames` times (or
    /// until exit), then tears down. Fully deterministic (each tick advances by
    /// exactly `fixed_timestep`).
    pub fn run_headless(self, frames: u64) -> EngineResult<()> {
        let mut engine = self.build_headless()?;
        for _ in 0..frames {
            if engine.should_exit() {
                break;
            }
            engine.tick()?;
        }
        Ok(())
    }

    /// Builds a headless [`Engine`] (with a [`HeadlessBackend`]) without running
    /// it, so a caller can drive [`Engine::tick`] and inspect state frame by
    /// frame. The deterministic clock advances each tick by `fixed_timestep`
    /// (one fixed step per frame).
    pub fn build_headless(self) -> EngineResult<Engine> {
        let frame_dt = self.config.fixed_timestep;
        self.build_headless_with(frame_dt, Box::new(HeadlessBackend::new()))
    }

    /// Like [`build_headless`](App::build_headless) but with an explicit per-tick
    /// frame delta and a caller-supplied render backend. A `frame_dt` larger than
    /// `fixed_timestep` runs multiple fixed steps per frame (decoupling sim rate
    /// from render rate); a recording backend lets a caller inspect what was
    /// submitted.
    pub fn build_headless_with(
        self,
        frame_dt: f32,
        backend: Box<dyn RenderBackend>,
    ) -> EngineResult<Engine> {
        if self.config.fixed_timestep <= 0.0 {
            return Err(EngineError::InvalidConfig {
                reason: "fixed_timestep must be > 0",
            });
        }
        let parts = self.into_parts();
        Engine::assemble(parts, backend, Clock::Fixed(frame_dt))
    }

    fn into_parts(self) -> EngineParts {
        EngineParts {
            world: self.world,
            var_stages: self.var_stages,
            fixed_stages: self.fixed_stages,
            startup_stage: self.startup_stage,
            fixed_hooks: self.fixed_hooks,
            extracts: self.extracts,
            render_setups: self.render_setups,
            config: self.config,
        }
    }
}

/// The `PlatformApp` adapter for the windowed driver. The engine can only be
/// built once the window exists (the wgpu backend needs it), so the engine is
/// created in `init`; a tick error or build failure is stored and surfaced from
/// [`App::run`] after the loop exits.
struct WindowedDriver {
    parts: Option<EngineParts>,
    engine: Option<Engine>,
    error: Rc<RefCell<Option<EngineError>>>,
}

impl PlatformApp for WindowedDriver {
    fn init(&mut self, window: Arc<Window>) {
        let Some(mut parts) = self.parts.take() else {
            return;
        };
        let setups = std::mem::take(&mut parts.render_setups);
        let (w, h) = window.size();
        let size = SurfaceSize::new(w.max(1), h.max(1));
        let backend: Box<dyn RenderBackend> = match WgpuBackend::new(
            window,
            size,
            RendererConfig::default(),
            Color::BLACK,
            setups,
        ) {
            Ok(backend) => Box::new(backend),
            Err(err) => {
                *self.error.borrow_mut() = Some(err);
                return;
            }
        };
        match Engine::assemble(parts, backend, Clock::Realtime(None)) {
            Ok(engine) => self.engine = Some(engine),
            Err(err) => *self.error.borrow_mut() = Some(err),
        }
    }

    fn event(&mut self, _window: &Window, event: &PlatformEvent) {
        let Some(engine) = self.engine.as_mut() else {
            return;
        };
        match event {
            PlatformEvent::Window(WindowEvent::Resized { width, height }) => {
                if let Err(err) = engine.resize(SurfaceSize::new(*width, *height)) {
                    *self.error.borrow_mut() = Some(err);
                    engine.request_exit();
                }
            }
            PlatformEvent::Window(WindowEvent::CloseRequested) => engine.request_exit(),
            other => engine.push_event(*other),
        }
    }

    fn update(&mut self, window: &Window) {
        match self.engine.as_ref() {
            Some(engine) if engine.should_exit() => window.request_exit(),
            Some(_) => window.request_redraw(),
            // The engine failed to build in `init`; exit so `run` returns the error.
            None => window.request_exit(),
        }
    }

    fn redraw_requested(&mut self, window: &Window) {
        if let Some(engine) = self.engine.as_mut() {
            if let Err(err) = engine.tick() {
                *self.error.borrow_mut() = Some(err);
                engine.request_exit();
                window.request_exit();
            }
        }
    }

    fn exit(&mut self, _window: &Window) {
        // Drops the engine, running teardown in field order.
        self.engine = None;
    }
}
