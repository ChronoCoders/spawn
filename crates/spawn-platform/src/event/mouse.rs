//! Mouse events and supporting value types.

use super::ButtonState;

/// A mouse input event.
///
/// `Moved` positions are physical pixels relative to the window, origin
/// top-left, `+x` right, `+y` down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MouseEvent {
    Moved {
        x: f64,
        y: f64,
    },
    Button {
        button: MouseButton,
        state: ButtonState,
    },
    Wheel {
        delta: ScrollDelta,
    },
    Entered,
    Left,
}

/// A mouse button. `Other` carries the raw platform button index for buttons
/// beyond the named set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u16),
}

/// Scroll wheel delta. `Lines` is line-based (notched wheels); `Pixels` is the
/// precise/trackpad delta. Both follow the cursor convention: `+y` is content
/// moving down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScrollDelta {
    Lines { x: f32, y: f32 },
    Pixels { x: f32, y: f32 },
}

pub(crate) fn translate_button(button: winit::event::MouseButton) -> MouseButton {
    match button {
        winit::event::MouseButton::Left => MouseButton::Left,
        winit::event::MouseButton::Right => MouseButton::Right,
        winit::event::MouseButton::Middle => MouseButton::Middle,
        winit::event::MouseButton::Back => MouseButton::Back,
        winit::event::MouseButton::Forward => MouseButton::Forward,
        winit::event::MouseButton::Other(code) => MouseButton::Other(code),
    }
}

pub(crate) fn translate_scroll(delta: winit::event::MouseScrollDelta) -> ScrollDelta {
    match delta {
        winit::event::MouseScrollDelta::LineDelta(x, y) => ScrollDelta::Lines { x, y },
        winit::event::MouseScrollDelta::PixelDelta(pos) => ScrollDelta::Pixels {
            x: pos.x as f32,
            y: pos.y as f32,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_round_trip() {
        assert_eq!(
            translate_button(winit::event::MouseButton::Left),
            MouseButton::Left
        );
        assert_eq!(
            translate_button(winit::event::MouseButton::Right),
            MouseButton::Right
        );
        assert_eq!(
            translate_button(winit::event::MouseButton::Middle),
            MouseButton::Middle
        );
        assert_eq!(
            translate_button(winit::event::MouseButton::Back),
            MouseButton::Back
        );
        assert_eq!(
            translate_button(winit::event::MouseButton::Forward),
            MouseButton::Forward
        );
        assert_eq!(
            translate_button(winit::event::MouseButton::Other(7)),
            MouseButton::Other(7)
        );
    }

    #[test]
    fn line_scroll_maps() {
        assert_eq!(
            translate_scroll(winit::event::MouseScrollDelta::LineDelta(1.0, -2.0)),
            ScrollDelta::Lines { x: 1.0, y: -2.0 }
        );
    }

    #[test]
    fn pixel_scroll_maps() {
        let pos = winit::dpi::PhysicalPosition::new(3.0_f64, 4.0_f64);
        assert_eq!(
            translate_scroll(winit::event::MouseScrollDelta::PixelDelta(pos)),
            ScrollDelta::Pixels { x: 3.0, y: 4.0 }
        );
    }
}
