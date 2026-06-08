//! Window configuration, the live window handle, and identifiers.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle,
};

use crate::error::{PlatformError, PlatformResult};

/// How the window occupies the display. `Default` is `Windowed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowMode {
    #[default]
    Windowed,
    /// Borderless window sized to the current monitor (windowed-fullscreen).
    BorderlessFullscreen,
    /// Exclusive video mode on the primary monitor, closest match to the
    /// requested size and refresh rate.
    ExclusiveFullscreen,
}

impl WindowMode {
    const fn to_u8(self) -> u8 {
        match self {
            WindowMode::Windowed => 0,
            WindowMode::BorderlessFullscreen => 1,
            WindowMode::ExclusiveFullscreen => 2,
        }
    }

    const fn from_u8(value: u8) -> Self {
        match value {
            1 => WindowMode::BorderlessFullscreen,
            2 => WindowMode::ExclusiveFullscreen,
            _ => WindowMode::Windowed,
        }
    }
}

/// Cursor confinement mode. `None` releases any grab and is the `Default`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorGrab {
    #[default]
    None,
    /// Cursor cannot leave the window bounds.
    Confined,
    /// Cursor fixed in place, for relative-motion / FPS-camera use.
    Locked,
}

/// Opaque, engine-owned window identifier, stable for the window's lifetime.
///
/// Translated from `winit::window::WindowId`, which is not re-exported. `Hash`
/// so downstream crates can key per-window state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(u64);

impl WindowId {
    #[cfg(test)]
    pub(crate) fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub(crate) fn from_winit(id: winit::window::WindowId) -> Self {
        Self(u64::from(id))
    }
}

/// Configuration used to create the primary window.
///
/// `size` is the requested logical size; it is scaled by the monitor
/// `scale_factor` at creation. `vsync` is an advisory hint forwarded to
/// spawn-render via [`Window::vsync_hint`]; spawn-platform does not own
/// presentation.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowConfig {
    pub title: String,
    pub size: (u32, u32),
    pub min_size: Option<(u32, u32)>,
    pub max_size: Option<(u32, u32)>,
    pub mode: WindowMode,
    pub resizable: bool,
    pub visible: bool,
    pub vsync: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "Spawn".to_string(),
            size: (1280, 720),
            min_size: None,
            max_size: None,
            mode: WindowMode::Windowed,
            resizable: true,
            visible: true,
            vsync: true,
        }
    }
}

impl WindowConfig {
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn with_size(mut self, w: u32, h: u32) -> Self {
        self.size = (w, h);
        self
    }

    pub fn with_min_size(mut self, w: u32, h: u32) -> Self {
        self.min_size = Some((w, h));
        self
    }

    pub fn with_max_size(mut self, w: u32, h: u32) -> Self {
        self.max_size = Some((w, h));
        self
    }

    pub fn with_mode(mut self, mode: WindowMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn with_resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    pub fn with_visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }

    pub fn with_vsync(mut self, vsync: bool) -> Self {
        self.vsync = vsync;
        self
    }
}

/// A live OS window. Created by the run loop; not constructible downstream and
/// not `Clone`. Shared into [`crate::app::PlatformApp`] callbacks — as an owning
/// `Arc<Window>` to `init`, and by reference to the others.
///
/// Implements [`HasWindowHandle`] and [`HasDisplayHandle`] so spawn-render can
/// build a wgpu surface without depending on winit. `Send + Sync` (its
/// interior-mutable `mode`/`exit_requested` state uses atomics, not `Cell`) so an
/// engine can hand it to spawn-render's surface as an `Arc<Window>`.
pub struct Window {
    inner: winit::window::Window,
    vsync: bool,
    mode: AtomicU8,
    exit_requested: AtomicBool,
}

impl Window {
    pub(crate) fn new(inner: winit::window::Window, vsync: bool, mode: WindowMode) -> Self {
        Self {
            inner,
            vsync,
            mode: AtomicU8::new(mode.to_u8()),
            exit_requested: AtomicBool::new(false),
        }
    }

    /// Stable identifier for this window's lifetime.
    pub fn id(&self) -> WindowId {
        WindowId::from_winit(self.inner.id())
    }

    /// Current physical size in pixels.
    pub fn size(&self) -> (u32, u32) {
        let size = self.inner.inner_size();
        (size.width, size.height)
    }

    /// DPI scale factor (physical divided by logical).
    pub fn scale_factor(&self) -> f64 {
        self.inner.scale_factor()
    }

    /// The configured `vsync` value; advisory for spawn-render.
    pub fn vsync_hint(&self) -> bool {
        self.vsync
    }

    pub fn mode(&self) -> WindowMode {
        WindowMode::from_u8(self.mode.load(Ordering::Relaxed))
    }

    /// Sets the window title.
    ///
    /// Returns `Ok(())` on every supported platform; the `PlatformResult`
    /// return preserves a fallible contract for backends that may reject a
    /// title change (currently none do).
    pub fn set_title(&self, title: &str) -> PlatformResult<()> {
        self.inner.set_title(title);
        Ok(())
    }

    /// Switches between windowed, borderless, and exclusive fullscreen.
    ///
    /// Returns [`PlatformError::Fullscreen`] if exclusive fullscreen is
    /// requested but no exclusive video mode is available on the primary
    /// monitor.
    pub fn set_mode(&self, mode: WindowMode) -> PlatformResult<()> {
        let fullscreen = match mode {
            WindowMode::Windowed => None,
            WindowMode::BorderlessFullscreen => Some(winit::window::Fullscreen::Borderless(None)),
            WindowMode::ExclusiveFullscreen => {
                let monitor = self
                    .inner
                    .current_monitor()
                    .ok_or(PlatformError::Fullscreen {
                        context: "no monitor for exclusive fullscreen",
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
        self.inner.set_fullscreen(fullscreen);
        self.mode.store(mode.to_u8(), Ordering::Relaxed);
        Ok(())
    }

    pub fn set_visible(&self, visible: bool) {
        self.inner.set_visible(visible);
    }

    /// Schedules a `redraw_requested` callback on the next loop iteration.
    pub fn request_redraw(&self) {
        self.inner.request_redraw();
    }

    /// Requests that the run loop exit after the current iteration completes.
    ///
    /// spawn-platform does not auto-close on `CloseRequested`; an app calls this
    /// to shut down cleanly. [`PlatformApp::exit`](crate::PlatformApp::exit) then
    /// fires exactly once before [`EventLoop::run`](crate::EventLoop::run)
    /// returns. Idempotent: repeated calls are equivalent to one.
    pub fn request_exit(&self) {
        self.exit_requested.store(true, Ordering::Relaxed);
    }

    /// Whether [`request_exit`](Window::request_exit) has been called; checked by
    /// the run loop once per iteration to decide whether to exit.
    pub(crate) fn exit_requested(&self) -> bool {
        self.exit_requested.load(Ordering::Relaxed)
    }

    pub fn set_cursor_visible(&self, visible: bool) {
        self.inner.set_cursor_visible(visible);
    }

    /// Confines or locks the cursor. `CursorGrab::None` releases any grab.
    ///
    /// Returns [`PlatformError::CursorGrab`] if neither the requested mode nor a
    /// platform-supported fallback is available.
    pub fn set_cursor_grab(&self, grab: CursorGrab) -> PlatformResult<()> {
        let primary = match grab {
            CursorGrab::None => winit::window::CursorGrabMode::None,
            CursorGrab::Confined => winit::window::CursorGrabMode::Confined,
            CursorGrab::Locked => winit::window::CursorGrabMode::Locked,
        };
        if self.inner.set_cursor_grab(primary).is_ok() {
            return Ok(());
        }
        if grab == CursorGrab::None {
            return Err(PlatformError::CursorGrab {
                context: "failed to release cursor grab",
            });
        }
        let fallback = match grab {
            CursorGrab::Confined => winit::window::CursorGrabMode::Locked,
            CursorGrab::Locked => winit::window::CursorGrabMode::Confined,
            CursorGrab::None => winit::window::CursorGrabMode::None,
        };
        self.inner
            .set_cursor_grab(fallback)
            .map_err(|_| PlatformError::CursorGrab {
                context: "cursor grab not supported on this platform",
            })
    }
}

impl HasWindowHandle for Window {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        self.inner.window_handle()
    }
}

impl HasDisplayHandle for Window {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        self.inner.display_handle()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default_matches_spec() {
        let c = WindowConfig::default();
        assert_eq!(c.title, "Spawn");
        assert_eq!(c.size, (1280, 720));
        assert_eq!(c.min_size, None);
        assert_eq!(c.max_size, None);
        assert_eq!(c.mode, WindowMode::Windowed);
        assert!(c.resizable);
        assert!(c.visible);
        assert!(c.vsync);
    }

    #[test]
    fn builder_chains() {
        let c = WindowConfig::default()
            .with_title("Game")
            .with_size(640, 480)
            .with_min_size(320, 240)
            .with_max_size(1920, 1080)
            .with_mode(WindowMode::BorderlessFullscreen)
            .with_resizable(false)
            .with_visible(false)
            .with_vsync(false);
        assert_eq!(c.title, "Game");
        assert_eq!(c.size, (640, 480));
        assert_eq!(c.min_size, Some((320, 240)));
        assert_eq!(c.max_size, Some((1920, 1080)));
        assert_eq!(c.mode, WindowMode::BorderlessFullscreen);
        assert!(!c.resizable);
        assert!(!c.visible);
        assert!(!c.vsync);
    }

    #[test]
    fn mode_and_grab_defaults() {
        assert_eq!(WindowMode::default(), WindowMode::Windowed);
        assert_eq!(CursorGrab::default(), CursorGrab::None);
    }

    #[test]
    fn window_mode_u8_roundtrips() {
        for mode in [
            WindowMode::Windowed,
            WindowMode::BorderlessFullscreen,
            WindowMode::ExclusiveFullscreen,
        ] {
            assert_eq!(WindowMode::from_u8(mode.to_u8()), mode);
        }
    }

    #[test]
    fn window_is_send_and_sync() {
        // The whole point of the atomic interior state: `Window` must be
        // shareable as `Arc<Window>` for spawn-render's owned surface.
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Window>();
    }

    #[test]
    fn window_id_is_hashable_and_eq() {
        use std::collections::HashSet;
        let a = WindowId::from_raw(1);
        let b = WindowId::from_raw(1);
        let c = WindowId::from_raw(2);
        assert_eq!(a, b);
        assert_ne!(a, c);
        let mut set = HashSet::new();
        set.insert(a);
        assert!(set.contains(&b));
        assert!(!set.contains(&c));
    }
}
