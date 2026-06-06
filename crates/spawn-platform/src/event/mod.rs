//! Engine-owned event enums and the winit-to-spawn translation layer.
//!
//! All variants are translations of winit events; winit `WindowEvent` /
//! `DeviceEvent` values never escape the crate. Every type here is `Copy` so
//! per-event translation is allocation-free. Positions are physical pixels,
//! origin top-left, `+x` right, `+y` down.

mod keyboard;
mod mouse;
mod touch;

pub use keyboard::{KeyCode, KeyboardEvent};
pub use mouse::{MouseButton, MouseEvent, ScrollDelta};
pub use touch::{TouchEvent, TouchPhase};

/// A translated platform event delivered to `PlatformApp::event`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlatformEvent {
    Window(WindowEvent),
    Keyboard(KeyboardEvent),
    Mouse(MouseEvent),
    Touch(TouchEvent),
}

/// Window-lifecycle and geometry events.
///
/// `Resized` carries physical pixels; `Moved` carries the physical position of
/// the window's top-left on the virtual desktop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowEvent {
    Resized { width: u32, height: u32 },
    ScaleFactorChanged { scale_factor: f64 },
    CloseRequested,
    Focused(bool),
    Occluded(bool),
    Moved { x: i32, y: i32 },
}

/// Pressed/released state shared by keyboard and mouse-button events.
/// `Default` is `Released`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ButtonState {
    Pressed,
    #[default]
    Released,
}

pub(crate) fn translate_element_state(state: winit::event::ElementState) -> ButtonState {
    match state {
        winit::event::ElementState::Pressed => ButtonState::Pressed,
        winit::event::ElementState::Released => ButtonState::Released,
    }
}

/// Translates a winit window event into a `PlatformEvent`.
///
/// Returns `None` for events the run loop consumes directly rather than
/// forwarding (`RedrawRequested`, `ScaleFactorChanged` whose new physical size
/// the loop applies, and events outside Phase 1 scope). `ScaleFactorChanged` is
/// surfaced here as a scale-only notification; the resulting resize arrives as a
/// separate `Resized`.
pub(crate) fn translate_window_event(event: &winit::event::WindowEvent) -> Option<PlatformEvent> {
    use winit::event::WindowEvent as W;

    match event {
        W::Resized(size) => Some(PlatformEvent::Window(WindowEvent::Resized {
            width: size.width,
            height: size.height,
        })),
        W::ScaleFactorChanged { scale_factor, .. } => {
            Some(PlatformEvent::Window(WindowEvent::ScaleFactorChanged {
                scale_factor: *scale_factor,
            }))
        }
        W::CloseRequested => Some(PlatformEvent::Window(WindowEvent::CloseRequested)),
        W::Focused(focused) => Some(PlatformEvent::Window(WindowEvent::Focused(*focused))),
        W::Occluded(occluded) => Some(PlatformEvent::Window(WindowEvent::Occluded(*occluded))),
        W::Moved(pos) => Some(PlatformEvent::Window(WindowEvent::Moved {
            x: pos.x,
            y: pos.y,
        })),
        W::KeyboardInput { event, .. } => Some(PlatformEvent::Keyboard(KeyboardEvent {
            key: keyboard::translate_key(event.physical_key),
            state: translate_element_state(event.state),
            repeat: event.repeat,
        })),
        W::CursorMoved { position, .. } => Some(PlatformEvent::Mouse(MouseEvent::Moved {
            x: position.x,
            y: position.y,
        })),
        W::CursorEntered { .. } => Some(PlatformEvent::Mouse(MouseEvent::Entered)),
        W::CursorLeft { .. } => Some(PlatformEvent::Mouse(MouseEvent::Left)),
        W::MouseInput { state, button, .. } => Some(PlatformEvent::Mouse(MouseEvent::Button {
            button: mouse::translate_button(*button),
            state: translate_element_state(*state),
        })),
        W::MouseWheel { delta, .. } => Some(PlatformEvent::Mouse(MouseEvent::Wheel {
            delta: mouse::translate_scroll(*delta),
        })),
        W::Touch(touch) => Some(PlatformEvent::Touch(touch::translate_touch(*touch))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_copy<T: Copy>() {}

    #[test]
    fn event_types_are_copy() {
        assert_copy::<PlatformEvent>();
        assert_copy::<WindowEvent>();
        assert_copy::<KeyboardEvent>();
        assert_copy::<MouseEvent>();
        assert_copy::<TouchEvent>();
        assert_copy::<ButtonState>();
        assert_copy::<KeyCode>();
        assert_copy::<MouseButton>();
        assert_copy::<ScrollDelta>();
        assert_copy::<TouchPhase>();
    }

    #[test]
    fn button_state_default_is_released() {
        assert_eq!(ButtonState::default(), ButtonState::Released);
    }

    #[test]
    fn element_state_round_trips() {
        assert_eq!(
            translate_element_state(winit::event::ElementState::Pressed),
            ButtonState::Pressed
        );
        assert_eq!(
            translate_element_state(winit::event::ElementState::Released),
            ButtonState::Released
        );
    }

    #[test]
    fn resized_translates() {
        let ev = winit::event::WindowEvent::Resized(winit::dpi::PhysicalSize::new(800, 600));
        assert_eq!(
            translate_window_event(&ev),
            Some(PlatformEvent::Window(WindowEvent::Resized {
                width: 800,
                height: 600
            }))
        );
    }

    #[test]
    fn close_requested_translates() {
        let ev = winit::event::WindowEvent::CloseRequested;
        assert_eq!(
            translate_window_event(&ev),
            Some(PlatformEvent::Window(WindowEvent::CloseRequested))
        );
    }

    #[test]
    fn moved_translates() {
        let ev = winit::event::WindowEvent::Moved(winit::dpi::PhysicalPosition::new(10, -5));
        assert_eq!(
            translate_window_event(&ev),
            Some(PlatformEvent::Window(WindowEvent::Moved { x: 10, y: -5 }))
        );
    }

    #[test]
    fn focused_and_occluded_translate() {
        assert_eq!(
            translate_window_event(&winit::event::WindowEvent::Focused(true)),
            Some(PlatformEvent::Window(WindowEvent::Focused(true)))
        );
        assert_eq!(
            translate_window_event(&winit::event::WindowEvent::Occluded(false)),
            Some(PlatformEvent::Window(WindowEvent::Occluded(false)))
        );
    }

    #[test]
    fn cursor_moved_translates() {
        let ev = winit::event::WindowEvent::CursorMoved {
            device_id: winit::event::DeviceId::dummy(),
            position: winit::dpi::PhysicalPosition::new(12.0, 34.0),
        };
        assert_eq!(
            translate_window_event(&ev),
            Some(PlatformEvent::Mouse(MouseEvent::Moved { x: 12.0, y: 34.0 }))
        );
    }

    #[test]
    fn redraw_is_not_forwarded() {
        assert_eq!(
            translate_window_event(&winit::event::WindowEvent::RedrawRequested),
            None
        );
    }

    #[test]
    fn translation_loop_is_allocation_free() {
        let ev = winit::event::WindowEvent::Resized(winit::dpi::PhysicalSize::new(1, 1));
        let mut last = None;
        for _ in 0..1000 {
            last = translate_window_event(&ev);
        }
        assert!(last.is_some());
    }
}
