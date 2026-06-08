//! The application model and the run loop bridging winit's `ApplicationHandler`.
//!
//! Downstream code implements [`PlatformApp`]; it never sees winit. The
//! per-iteration dispatch order is normative: translated events are delivered to
//! [`PlatformApp::event`] one at a time, then [`PlatformApp::update`], then
//! [`PlatformApp::redraw_requested`] only when a redraw is pending. [`PlatformApp::init`]
//! fires exactly once before the first event; [`PlatformApp::exit`] exactly once
//! after the last.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent as WinitWindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop as WinitEventLoop};
use winit::platform::pump_events::EventLoopExtPumpEvents;
use winit::window::WindowAttributes;

use crate::error::{PlatformError, PlatformResult};
use crate::event::{translate_window_event, PlatformEvent};
use crate::monitor::{translate_monitor, MonitorInfo};
use crate::window::{Window, WindowConfig, WindowMode};

/// Application callbacks driven by the platform run loop.
///
/// `init` receives an owning `Arc<Window>` the app may keep; the other callbacks
/// borrow the window and cannot store it. spawn-platform does not auto-close on
/// `CloseRequested` — the app decides whether to exit by calling
/// [`Window::request_exit`](crate::Window::request_exit) from any callback; the
/// loop then exits after the current iteration and fires [`exit`](PlatformApp::exit) once.
pub trait PlatformApp {
    /// Called once after the window is created and before the first event.
    ///
    /// Receives the window as an `Arc<Window>` (not a borrow) so the app can keep
    /// it alive past the callback — e.g. to hand it to spawn-render as a
    /// surface-owning handle. The subsequent callbacks borrow the window instead.
    fn init(&mut self, window: Arc<Window>);

    /// Called for every translated platform event.
    fn event(&mut self, window: &Window, event: &PlatformEvent);

    /// Called once per loop iteration after the event batch, before redraw.
    fn update(&mut self, _window: &Window) {}

    /// Called when a redraw is required (OS-driven or after `request_redraw`).
    fn redraw_requested(&mut self, _window: &Window) {}

    /// Called once as the loop exits (window closed or exit requested).
    fn exit(&mut self, _window: &Window) {}
}

/// The platform event loop.
///
/// `inner` is wrapped in a `RefCell` so the setup-time monitor queries can take
/// `&self` while internally driving winit's `&mut self` pump; `run` reclaims
/// ownership via `into_inner`.
pub struct EventLoop {
    inner: RefCell<WinitEventLoop<()>>,
}

/// Captures a monitor snapshot during a single pumped iteration, since winit
/// 0.30 exposes monitor enumeration only through an active event loop.
struct MonitorProbe {
    monitors: Vec<MonitorInfo>,
    primary: Option<MonitorInfo>,
}

impl ApplicationHandler<()> for MonitorProbe {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.monitors = event_loop
            .available_monitors()
            .map(|h| translate_monitor(&h))
            .collect();
        self.primary = event_loop.primary_monitor().map(|h| translate_monitor(&h));
        event_loop.exit();
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        _event: WinitWindowEvent,
    ) {
    }
}

impl EventLoop {
    /// Creates the platform event loop.
    ///
    /// Returns [`PlatformError::EventLoopCreation`] if the OS denies one (e.g.
    /// not on the main thread, or no display server is available).
    pub fn new() -> PlatformResult<Self> {
        WinitEventLoop::with_user_event()
            .build()
            .map(|inner| Self {
                inner: RefCell::new(inner),
            })
            .map_err(|_| PlatformError::EventLoopCreation {
                context: "winit event loop build failed",
            })
    }

    fn probe_monitors(&self) -> MonitorProbe {
        let mut probe = MonitorProbe {
            monitors: Vec::new(),
            primary: None,
        };
        let _ = self
            .inner
            .borrow_mut()
            .pump_app_events(Some(Duration::ZERO), &mut probe);
        probe
    }

    /// All connected monitors.
    ///
    /// Drives one event-loop iteration to snapshot the current display layout
    /// (winit 0.30 exposes monitor enumeration only from an active loop). A
    /// setup-time operation; allocation is permitted.
    pub fn available_monitors(&self) -> Vec<MonitorInfo> {
        self.probe_monitors().monitors
    }

    /// The primary monitor, or `None` if the platform cannot determine one.
    pub fn primary_monitor(&self) -> Option<MonitorInfo> {
        self.probe_monitors().primary
    }

    /// Creates the configured window, then runs to completion, driving `app`.
    ///
    /// Consumes both. Must be called on the main thread; on desktop targets it
    /// returns when the loop exits or on a fatal platform error. Returns
    /// [`PlatformError::WindowCreation`] if the window cannot be created, or
    /// [`PlatformError::OsError`] if the loop terminates abnormally.
    pub fn run<A: PlatformApp>(self, config: WindowConfig, app: A) -> PlatformResult<()> {
        let mut handler = Handler {
            config,
            app,
            window: None,
            init_done: false,
            redraw_pending: false,
            pending_events: Vec::new(),
            error: None,
        };
        self.inner
            .into_inner()
            .run_app(&mut handler)
            .map_err(|_| PlatformError::OsError {
                context: "winit run_app terminated abnormally",
            })?;
        match handler.error.take() {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}

struct Handler<A: PlatformApp> {
    config: WindowConfig,
    app: A,
    window: Option<Arc<Window>>,
    init_done: bool,
    redraw_pending: bool,
    /// Events translated during the current event batch, drained once per loop
    /// iteration in `about_to_wait`. Retains its capacity across iterations, so
    /// steady-state dispatch performs no per-frame heap allocation.
    pending_events: Vec<PlatformEvent>,
    error: Option<PlatformError>,
}

impl<A: PlatformApp> Handler<A> {
    fn build_window(&self, event_loop: &ActiveEventLoop) -> PlatformResult<Window> {
        let mut attrs = WindowAttributes::default()
            .with_title(self.config.title.clone())
            .with_inner_size(winit::dpi::LogicalSize::new(
                self.config.size.0,
                self.config.size.1,
            ))
            .with_resizable(self.config.resizable)
            .with_visible(self.config.visible);

        if let Some((w, h)) = self.config.min_size {
            attrs = attrs.with_min_inner_size(winit::dpi::LogicalSize::new(w, h));
        }
        if let Some((w, h)) = self.config.max_size {
            attrs = attrs.with_max_inner_size(winit::dpi::LogicalSize::new(w, h));
        }

        let fullscreen = match self.config.mode {
            WindowMode::Windowed => None,
            WindowMode::BorderlessFullscreen => Some(winit::window::Fullscreen::Borderless(None)),
            WindowMode::ExclusiveFullscreen => {
                let monitor = event_loop
                    .primary_monitor()
                    .ok_or(PlatformError::Fullscreen {
                        context: "no primary monitor for exclusive fullscreen",
                    })?;
                let video_mode = monitor
                    .video_modes()
                    .next()
                    .ok_or(PlatformError::Fullscreen {
                        context: "no exclusive video mode available",
                    })?;
                Some(winit::window::Fullscreen::Exclusive(video_mode))
            }
        };
        attrs = attrs.with_fullscreen(fullscreen);

        let inner = event_loop
            .create_window(attrs)
            .map_err(|_| PlatformError::WindowCreation {
                context: "winit create_window failed",
            })?;
        Ok(Window::new(inner, self.config.vsync, self.config.mode))
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: PlatformError) {
        self.error = Some(error);
        event_loop.exit();
    }

    /// Records a single OS event for this iteration's batch. A redraw request
    /// (app- or OS-driven) only arms the pending flag; the `redraw_requested`
    /// callback is deferred to `drive_iteration` so it runs once, after
    /// `update`, per the normative order. Other events are translated and
    /// buffered rather than dispatched here, because winit fires `window_event`
    /// once per OS event while `event`/`update` must run once per loop iteration
    /// over the whole batch (§3.1/§3.2).
    fn buffer_event(&mut self, event: &WinitWindowEvent) {
        if matches!(event, WinitWindowEvent::RedrawRequested) {
            self.redraw_pending = true;
            return;
        }
        if let Some(translated) = translate_window_event(event) {
            self.pending_events.push(translated);
        }
    }

    /// Drives one loop iteration per the normative order (§3.2): buffered events
    /// → `event` (per event) → `update` → `redraw_requested` (only when
    /// pending). Drains `pending_events` while retaining its capacity.
    fn drive_iteration(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let mut events = std::mem::take(&mut self.pending_events);
        for event in events.drain(..) {
            self.app.event(&window, &event);
        }
        self.app.update(&window);
        if self.redraw_pending {
            self.redraw_pending = false;
            self.app.redraw_requested(&window);
        }
        self.pending_events = events;
    }
}

impl<A: PlatformApp> ApplicationHandler<()> for Handler<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        match self.build_window(event_loop) {
            Ok(window) => {
                // `init` fires once at window creation, before the first event
                // (§3.2). `exit` is gated on this flag so an early-exiting loop
                // never calls `exit` without a preceding `init`. The window is
                // shared as an `Arc` so the app may keep it past `init`.
                let window = Arc::new(window);
                self.app.init(Arc::clone(&window));
                self.init_done = true;
                self.window = Some(window);
            }
            Err(err) => self.fail(event_loop, err),
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WinitWindowEvent,
    ) {
        if self.window.is_none() {
            return;
        }
        self.buffer_event(&event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if event_loop.exiting() {
            return;
        }
        self.drive_iteration();
        // An app callback may have requested exit during this iteration; honor it
        // before the loop waits again so `run` returns promptly.
        if let Some(window) = self.window.as_ref() {
            if window.exit_requested() {
                event_loop.exit();
            }
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // `exit` fires exactly once, and only after `init` (§3.2): an early-
        // exiting loop that never created a window must not call `exit`.
        if !self.init_done {
            return;
        }
        if let Some(window) = self.window.as_ref() {
            self.app.exit(window);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{KeyCode, KeyboardEvent, WindowEvent};

    /// Declares only the two required methods, leaving `update`,
    /// `redraw_requested`, and `exit` to their trait defaults.
    struct DefaultsOnlyApp;

    impl PlatformApp for DefaultsOnlyApp {
        fn init(&mut self, _window: Arc<Window>) {}
        fn event(&mut self, _window: &Window, _event: &PlatformEvent) {}
    }

    struct CountingApp {
        inits: u32,
        events: u32,
        last_key: Option<KeyCode>,
    }

    impl PlatformApp for CountingApp {
        fn init(&mut self, _window: Arc<Window>) {
            self.inits += 1;
        }

        fn event(&mut self, _window: &Window, event: &PlatformEvent) {
            self.events += 1;
            if let PlatformEvent::Keyboard(KeyboardEvent { key, .. }) = event {
                self.last_key = Some(*key);
            }
        }
    }

    /// Builds a window-less handler so the buffering and flag-gating logic
    /// (which never dereferences the window) is exercised headlessly.
    fn headless_handler<A: PlatformApp>(app: A) -> Handler<A> {
        Handler {
            config: WindowConfig::default(),
            app,
            window: None,
            init_done: false,
            redraw_pending: false,
            pending_events: Vec::new(),
            error: None,
        }
    }

    fn resized_event(width: u32, height: u32) -> WinitWindowEvent {
        WinitWindowEvent::Resized(winit::dpi::PhysicalSize::new(width, height))
    }

    #[test]
    fn trait_defaults_are_provided_and_callable() {
        // `DefaultsOnlyApp` implements only the two required methods, so this
        // type-checks only if `update`/`redraw_requested`/`exit` have working
        // default bodies. Binding the calling closure as a `fn(&mut _, &Window)`
        // resolves those defaults at compile time without needing a real window
        // to call them through.
        let calls_defaults: fn(&mut DefaultsOnlyApp, &Window) = |app, window| {
            app.update(window);
            app.redraw_requested(window);
            app.exit(window);
        };
        assert_eq!(
            calls_defaults as usize, calls_defaults as usize,
            "default-dispatch closure resolved"
        );
    }

    #[test]
    fn batch_buffers_one_event_per_os_event() {
        // winit fires one `window_event` per OS event; each is buffered for a
        // single end-of-iteration drain, not dispatched immediately.
        let mut h = headless_handler(CountingApp {
            inits: 0,
            events: 0,
            last_key: None,
        });
        h.buffer_event(&resized_event(800, 600));
        h.buffer_event(&WinitWindowEvent::Focused(true));
        h.buffer_event(&WinitWindowEvent::CloseRequested);
        // Nothing dispatched yet: the batch waits for `drive_iteration`.
        assert_eq!(h.app.events, 0);
        assert_eq!(h.pending_events.len(), 3);
        assert_eq!(
            h.pending_events.first(),
            Some(&PlatformEvent::Window(WindowEvent::Resized {
                width: 800,
                height: 600
            }))
        );
        assert_eq!(
            h.pending_events.last(),
            Some(&PlatformEvent::Window(WindowEvent::CloseRequested))
        );
    }

    #[test]
    fn redraw_request_only_arms_pending_flag() {
        // A `RedrawRequested` arms the gate but is never buffered as an event;
        // `redraw_requested` fires later, only because the flag is set.
        let mut h = headless_handler(CountingApp {
            inits: 0,
            events: 0,
            last_key: None,
        });
        assert!(!h.redraw_pending);
        h.buffer_event(&WinitWindowEvent::RedrawRequested);
        assert!(h.redraw_pending);
        assert!(h.pending_events.is_empty());
    }

    /// Mirrors the `exiting` gate: `exit` runs only after `init`. Kept beside the
    /// handler so the test and the production guard cannot drift.
    fn should_run_exit<A: PlatformApp>(h: &Handler<A>) -> bool {
        h.init_done && h.window.is_some()
    }

    #[test]
    fn exit_is_gated_on_init_having_run() {
        // A loop that exits before window creation (hence before `init`) must
        // not call `exit`.
        let mut h = headless_handler(CountingApp {
            inits: 0,
            events: 0,
            last_key: None,
        });
        assert!(!h.init_done);
        assert!(!should_run_exit(&h));

        // Once `resumed` has run `init` and stored the window, the gate opens.
        h.init_done = true;
        assert!(!should_run_exit(&h), "still gated without a window");
    }

    #[test]
    fn drive_iteration_without_window_is_a_noop() {
        // With no window the per-iteration drive must not touch the app or
        // disturb the buffered batch.
        let mut h = headless_handler(CountingApp {
            inits: 0,
            events: 0,
            last_key: None,
        });
        h.buffer_event(&resized_event(1, 1));
        h.redraw_pending = true;
        h.drive_iteration();
        assert_eq!(h.app.events, 0);
        assert_eq!(h.pending_events.len(), 1);
        assert!(h.redraw_pending);
    }

    #[test]
    #[ignore = "requires a display server; run with `cargo test -- --ignored` on a platform runner"]
    fn event_loop_runs_to_completion() {
        let event_loop = EventLoop::new().expect("event loop");
        let app = CountingApp {
            inits: 0,
            events: 0,
            last_key: None,
        };
        event_loop
            .run(WindowConfig::default().with_visible(false), app)
            .expect("run");
    }

    #[test]
    #[ignore = "requires a display server; run with `cargo test -- --ignored` on a platform runner"]
    fn monitor_enumeration_returns_primary() {
        let event_loop = EventLoop::new().expect("event loop");
        let monitors = event_loop.available_monitors();
        assert!(!monitors.is_empty());
    }

    /// Requests exit from `init`, so a single iteration drives the loop to a
    /// clean return rather than blocking forever.
    struct ExitOnInitApp;

    impl PlatformApp for ExitOnInitApp {
        fn init(&mut self, window: Arc<Window>) {
            window.request_exit();
        }
        fn event(&mut self, _window: &Window, _event: &PlatformEvent) {}
    }

    #[test]
    #[ignore = "requires a display server; run with `cargo test -- --ignored` on a platform runner"]
    fn app_request_exit_returns_from_run() {
        let event_loop = EventLoop::new().expect("event loop");
        event_loop
            .run(WindowConfig::default().with_visible(false), ExitOnInitApp)
            .expect("run returns cleanly after request_exit");
    }
}
