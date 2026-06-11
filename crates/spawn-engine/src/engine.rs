//! The [`Engine`] runtime and the frame pipeline.
//!
//! `Engine` owns the live world, the two schedules, the subsystem handles, the
//! render-proxy store, and the frame clock. [`Engine::tick`] runs exactly one
//! frame; the headless driver calls it directly and the windowed driver calls it
//! from `redraw_requested`. Field declaration order is the teardown order
//! (render backend → input → audio → asset server → world): the asset server's
//! `Drop` joins its IO threads while the world (which may hold asset handles) is
//! still alive.

use std::time::Instant;

use spawn_asset::{AssetServer, AssetServerConfig};
use spawn_audio::{AudioConfig, AudioEngine};
use spawn_ecs::{EcsResult, Schedule, Stage, World};
use spawn_input::InputState;
use spawn_platform::PlatformEvent;
use spawn_render::SurfaceSize;

use crate::audio::AudioCommands;
use crate::config::EngineConfig;
use crate::error::EngineResult;
use crate::input::InputFrame;
use crate::render::{RenderBackend, RenderProxies, RenderProxyStore};
use crate::time::Time;
use crate::ui::{UiSetup, UiUpdate};
use spawn_ui::{Style, UiTree};

/// An exclusive fixed-step hook: `&mut World` work run once per fixed tick (where
/// physics is wired). Receives the current [`Time`] snapshot.
pub(crate) type FixedHook = Box<dyn FnMut(&mut World, &Time) -> EcsResult<()> + Send>;

/// A proxy-extraction routine: reads the world and writes the backend's back
/// buffer at the sync point.
pub(crate) type ExtractFn = Box<dyn FnMut(&World, &mut RenderProxies) + Send>;

/// The frame-delta source: a fixed step (headless, deterministic) or wall-clock
/// (windowed).
pub(crate) enum Clock {
    Fixed(f32),
    Realtime(Option<Instant>),
}

/// The parts an [`App`](crate::App) hands to [`Engine::assemble`].
pub(crate) struct EngineParts {
    pub world: World,
    pub var_stages: Vec<Stage>,
    pub fixed_stages: Vec<Stage>,
    pub startup_stage: Stage,
    pub fixed_hooks: Vec<FixedHook>,
    pub extracts: Vec<ExtractFn>,
    /// Render-setup hooks; consumed by the windowed driver to populate the wgpu
    /// backend's resource registry. The headless path has no renderer and ignores
    /// them.
    pub render_setups: Vec<crate::render::RenderSetup>,
    pub audio_setups: Vec<crate::audio::AudioSetup>,
    pub ui_setups: Vec<UiSetup>,
    pub ui_updates: Vec<UiUpdate>,
    pub config: EngineConfig,
}

/// The live engine runtime.
pub struct Engine {
    backend: Box<dyn RenderBackend>,
    input: InputState,
    audio: AudioEngine,
    assets: AssetServer,
    world: World,
    var_schedule: Schedule,
    fixed_schedule: Schedule,
    fixed_hooks: Vec<FixedHook>,
    extracts: Vec<ExtractFn>,
    ui: Option<UiTree>,
    ui_updates: Vec<UiUpdate>,
    proxies: RenderProxyStore,
    time: Time,
    config: EngineConfig,
    pending_events: Vec<PlatformEvent>,
    accumulator: f32,
    clock: Clock,
    should_exit: bool,
}

impl Engine {
    /// Builds the runtime from an app's parts and a render backend, creating the
    /// subsystems, inserting the `Time` resource, building both schedules, and
    /// running the startup systems once. The variable/fixed schedules are built
    /// after startup so startup-deferred resources/spawns are visible.
    pub(crate) fn assemble(
        parts: EngineParts,
        backend: Box<dyn RenderBackend>,
        clock: Clock,
    ) -> EngineResult<Self> {
        let EngineParts {
            mut world,
            var_stages,
            fixed_stages,
            startup_stage,
            fixed_hooks,
            extracts,
            // Render-setup hooks are consumed by the windowed driver before
            // assemble; the headless path has no renderer to run them against.
            render_setups: _,
            audio_setups,
            ui_setups,
            ui_updates,
            config,
        } = parts;

        let input = InputState::new()?;
        let audio = AudioEngine::new(AudioConfig::default())?;
        let mut assets = AssetServer::new(AssetServerConfig {
            root: ".".into(),
            hot_reload: false,
            ..Default::default()
        })?;

        let time = Time::new(config.fixed_timestep);
        world.insert_resource(time);
        world.insert_resource(InputFrame::snapshot(&input));
        world.insert_resource(AudioCommands::new());

        for setup in audio_setups {
            setup(&mut assets, &mut world)?;
        }

        let ui = if ui_setups.is_empty() {
            None
        } else {
            let mut tree = UiTree::new(Style::default());
            for setup in ui_setups {
                setup(&mut tree)?;
            }
            Some(tree)
        };

        // Startup runs once without an event swap, so first-frame readers still
        // see any events startup produced.
        let mut startup = Schedule::new();
        startup.add_stage(startup_stage);
        startup.build(&world)?;
        startup.run_stages(&mut world)?;

        let mut var_schedule = Schedule::new();
        for stage in var_stages {
            var_schedule.add_stage(stage);
        }
        var_schedule.build(&world)?;

        let mut fixed_schedule = Schedule::new();
        for stage in fixed_stages {
            fixed_schedule.add_stage(stage);
        }
        fixed_schedule.build(&world)?;

        Ok(Self {
            backend,
            input,
            audio,
            assets,
            world,
            var_schedule,
            fixed_schedule,
            fixed_hooks,
            extracts,
            ui,
            ui_updates,
            proxies: RenderProxyStore::new(),
            time,
            config,
            pending_events: Vec::new(),
            accumulator: 0.0,
            clock,
            should_exit: false,
        })
    }

    /// Runs exactly one frame of the pipeline (clock → input → asset pump →
    /// fixed-step accumulator → variable schedule → event swap → extract → render
    /// → audio). Platform-agnostic and headless-callable.
    pub fn tick(&mut self) -> EngineResult<()> {
        // 1. Clock advance (clamped to bound the spiral of death).
        let dt = self.sample_delta().min(self.config.max_frame_delta);
        self.time.advance_frame(dt);
        self.accumulator += dt;
        self.sync_time();

        // 2. Input: begin_frame before processing this frame's buffered events.
        self.input.begin_frame();
        for event in self.pending_events.drain(..) {
            self.input.process(&event);
        }
        let input_frame = InputFrame::snapshot(&self.input);
        if let Some(mut resource) = self.world.get_resource_mut::<InputFrame>() {
            *resource = input_frame;
        }

        // 3. Asset pump (the single per-frame main-thread sync point).
        let _ = self.assets.apply_loaded();

        // 4. Fixed-step accumulator: fixed schedule then fixed hooks per tick,
        // capped per frame so a stall cannot run unbounded ticks.
        let mut steps = 0u32;
        while self.accumulator >= self.config.fixed_timestep
            && steps < self.config.max_fixed_steps_per_frame
        {
            self.time.advance_fixed_tick();
            self.sync_time();
            self.fixed_schedule.run_stages(&mut self.world)?;
            for hook in &mut self.fixed_hooks {
                hook(&mut self.world, &self.time)?;
            }
            self.accumulator -= self.config.fixed_timestep;
            steps += 1;
        }
        self.time.set_alpha(self.accumulator);
        self.sync_time();

        // 5. Variable schedule, then the single per-frame event swap.
        self.var_schedule.run_stages(&mut self.world)?;
        self.world.update_events();

        // 6. Extract: write the back proxy buffer (the sync point).
        {
            let back = self.proxies.back_mut();
            back.reset();
            for extract in &mut self.extracts {
                extract(&self.world, back);
            }
        }

        // 7. UI: run the overlay updates against the live world. The tree is
        // engine-owned and threaded to the backend (bypassing the proxy buffer);
        // the backend lays it out and composites it (the headless backend ignores it).
        if let Some(tree) = self.ui.as_mut() {
            for update in &mut self.ui_updates {
                update(&self.world, tree)?;
            }
        }

        // 8. Publish per sync mode and render.
        let mode = self.config.sync_mode;
        self.backend
            .submit(self.proxies.read(mode), self.ui.as_mut())?;
        self.proxies.advance(mode);

        // 9. Audio pump.
        if let Some(mut commands) = self.world.get_resource_mut::<AudioCommands>() {
            if !commands.is_empty() {
                for command in commands.drain() {
                    let _ = self
                        .audio
                        .play(&command.source, command.params, &self.assets);
                }
            }
        }
        self.audio.update(dt)?;

        Ok(())
    }

    /// The live world.
    pub fn world(&self) -> &World {
        &self.world
    }

    /// Mutable world access (tests, inspection).
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// The current frame clock.
    pub fn time(&self) -> &Time {
        &self.time
    }

    /// The engine-owned overlay tree, present when any UI setup was registered.
    pub fn ui(&self) -> Option<&UiTree> {
        self.ui.as_ref()
    }

    /// Extractions written but not yet consumed by the backend: `0` in
    /// `Immediate`, `≤1` in `Pipelined`.
    pub fn frames_in_flight(&self) -> u32 {
        self.proxies.in_flight()
    }

    /// Whether a close request or host exit has been received.
    pub fn should_exit(&self) -> bool {
        self.should_exit
    }

    /// Buffers an OS event for the next frame's input drain (windowed driver).
    pub(crate) fn push_event(&mut self, event: PlatformEvent) {
        self.pending_events.push(event);
    }

    /// Forwards a surface resize to the backend (windowed driver).
    pub(crate) fn resize(&mut self, size: SurfaceSize) -> EngineResult<()> {
        self.backend.resize(size)
    }

    /// Marks the engine for shutdown (windowed driver, on close request).
    pub(crate) fn request_exit(&mut self) {
        self.should_exit = true;
    }

    fn sample_delta(&mut self) -> f32 {
        match &mut self.clock {
            Clock::Fixed(dt) => *dt,
            Clock::Realtime(last) => {
                let now = Instant::now();
                let dt = last
                    .map(|prev| now.duration_since(prev).as_secs_f32())
                    .unwrap_or(0.0);
                *last = Some(now);
                dt
            }
        }
    }

    /// Mirrors the authoritative `Time` field into the world resource so systems
    /// read the current value via `Res<Time>`.
    fn sync_time(&self) {
        if let Some(mut resource) = self.world.get_resource_mut::<Time>() {
            *resource = self.time;
        }
    }
}
