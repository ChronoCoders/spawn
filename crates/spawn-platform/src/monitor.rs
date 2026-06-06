//! Monitor metadata and enumeration.
//!
//! Enumeration is a setup-time operation and may allocate (`String`, `Vec`),
//! unlike per-event translation. Sizes and positions are physical pixels.

/// Metadata for a connected monitor.
///
/// `name` is `None` when the platform cannot report one. `refresh_rate_mhz` is
/// `None` unless winit reports a known current video mode. `position` is the
/// monitor's top-left on the virtual desktop.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    pub name: Option<String>,
    pub size: (u32, u32),
    pub position: (i32, i32),
    pub scale_factor: f64,
    pub refresh_rate_mhz: Option<u32>,
    pub video_modes: Vec<VideoMode>,
}

/// A single exclusive-fullscreen-capable video mode. `refresh_rate_mhz` is in
/// milli-hertz, matching winit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VideoMode {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u16,
    pub refresh_rate_mhz: u32,
}

pub(crate) fn translate_video_mode(mode: &winit::monitor::VideoModeHandle) -> VideoMode {
    let size = mode.size();
    VideoMode {
        width: size.width,
        height: size.height,
        bit_depth: mode.bit_depth(),
        refresh_rate_mhz: mode.refresh_rate_millihertz(),
    }
}

pub(crate) fn translate_monitor(handle: &winit::monitor::MonitorHandle) -> MonitorInfo {
    let size = handle.size();
    let position = handle.position();
    MonitorInfo {
        name: handle.name(),
        size: (size.width, size.height),
        position: (position.x, position.y),
        scale_factor: handle.scale_factor(),
        refresh_rate_mhz: handle.refresh_rate_millihertz(),
        video_modes: handle
            .video_modes()
            .map(|m| translate_video_mode(&m))
            .collect(),
    }
}
