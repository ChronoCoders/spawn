//! Per-frame device input state.
//!
//! [`InputState`] owns keyboard, mouse, touch, and gamepad state. The per-frame
//! contract is: call [`InputState::begin_frame`] once, then
//! [`InputState::process`] zero or more times, then query. `just_pressed` /
//! `just_released` are edge-true only on the frame the edge's event was
//! processed (see the §1.2 convention on each device).

mod gamepad;
mod keyboard;
mod mouse;
mod touch;

pub use gamepad::{Deadzone, GamepadAxis, GamepadButton, GamepadEvent, GamepadId, Gamepads};
pub use keyboard::Keyboard;
pub use mouse::Mouse;
pub use touch::{Touch, TouchId, TouchPhase, TouchPoint};

use spawn_core::Vec2;
use spawn_platform::{ButtonState, MouseEvent, PlatformEvent, WindowEvent};

use crate::error::InputResult;

/// Owner of all device input state for the application lifetime.
///
/// Not `Clone`: frame-edge bits and backend ids must not be duplicated.
#[derive(Debug)]
pub struct InputState {
    keyboard: Keyboard,
    mouse: Mouse,
    touch: Touch,
    gamepads: Gamepads,
}

impl InputState {
    /// Constructs input state and initializes the gamepad backend.
    ///
    /// Returns `Err(InputError::BackendInit)` only on a hard backend failure; a
    /// headless host with no gamepad access still succeeds in no-gamepad mode.
    pub fn new() -> InputResult<Self> {
        Ok(Self {
            keyboard: Keyboard::new(),
            mouse: Mouse::new(),
            touch: Touch::new(),
            gamepads: Gamepads::new()?,
        })
    }

    /// Advances edge state for a new frame: copies held bits into the
    /// previous-frame bits, zeroes mouse delta/wheel, ages out finished touches,
    /// clears gamepad connection events, and drains the gamepad backend. Call
    /// exactly once before `process`.
    pub fn begin_frame(&mut self) {
        self.keyboard.begin_frame();
        self.mouse.begin_frame();
        self.touch.begin_frame();
        self.gamepads.begin_frame();
    }

    /// Applies one platform event to device state. Non-input window events are
    /// ignored except `Focused(false)`, which releases all held keyboard/mouse
    /// buttons (yielding `just_released` next frame).
    pub fn process(&mut self, event: &PlatformEvent) {
        match event {
            PlatformEvent::Keyboard(ev) => {
                self.keyboard.set(ev.key, ev.state == ButtonState::Pressed);
            }
            PlatformEvent::Mouse(ev) => self.process_mouse(ev),
            PlatformEvent::Touch(ev) => self.touch.process(ev),
            PlatformEvent::Window(WindowEvent::Focused(false)) => {
                self.keyboard.release_all();
                self.mouse.release_all();
            }
            PlatformEvent::Window(_) => {}
        }
    }

    fn process_mouse(&mut self, event: &MouseEvent) {
        match *event {
            MouseEvent::Moved { x, y } => {
                self.mouse.set_position(Vec2::new(x as f32, y as f32));
            }
            MouseEvent::Button { button, state } => {
                self.mouse.set_button(button, state == ButtonState::Pressed);
            }
            MouseEvent::Wheel { delta } => self.mouse.add_wheel(delta),
            MouseEvent::Entered | MouseEvent::Left => {}
        }
    }

    pub fn keyboard(&self) -> &Keyboard {
        &self.keyboard
    }

    pub fn mouse(&self) -> &Mouse {
        &self.mouse
    }

    pub fn touch(&self) -> &Touch {
        &self.touch
    }

    pub fn gamepads(&self) -> &Gamepads {
        &self.gamepads
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_core::traits::ApproxEq;
    use spawn_platform::{KeyCode, KeyboardEvent, MouseButton};

    fn key(key: KeyCode, pressed: bool) -> PlatformEvent {
        PlatformEvent::Keyboard(KeyboardEvent {
            key,
            state: if pressed {
                ButtonState::Pressed
            } else {
                ButtonState::Released
            },
            repeat: false,
        })
    }

    fn key_repeat(key: KeyCode) -> PlatformEvent {
        PlatformEvent::Keyboard(KeyboardEvent {
            key,
            state: ButtonState::Pressed,
            repeat: true,
        })
    }

    #[test]
    fn keyboard_press_edge_one_frame() {
        let mut input = InputState::new().expect("init");
        input.begin_frame();
        input.process(&key(KeyCode::Space, true));
        assert!(input.keyboard().is_pressed(KeyCode::Space));
        assert!(input.keyboard().just_pressed(KeyCode::Space));

        input.begin_frame();
        assert!(input.keyboard().is_pressed(KeyCode::Space));
        assert!(!input.keyboard().just_pressed(KeyCode::Space));
    }

    #[test]
    fn keyboard_release_edge_one_frame() {
        let mut input = InputState::new().expect("init");
        input.begin_frame();
        input.process(&key(KeyCode::W, true));
        input.begin_frame();
        input.process(&key(KeyCode::W, false));
        assert!(input.keyboard().just_released(KeyCode::W));
        input.begin_frame();
        assert!(!input.keyboard().just_released(KeyCode::W));
    }

    #[test]
    fn key_repeat_does_not_refire_just_pressed() {
        let mut input = InputState::new().expect("init");
        input.begin_frame();
        input.process(&key(KeyCode::A, true));
        input.begin_frame();
        input.process(&key_repeat(KeyCode::A));
        assert!(input.keyboard().is_pressed(KeyCode::A));
        assert!(!input.keyboard().just_pressed(KeyCode::A));
    }

    #[test]
    fn focus_lost_releases_and_reports_just_released() {
        let mut input = InputState::new().expect("init");
        input.begin_frame();
        input.process(&key(KeyCode::S, true));
        input.begin_frame();
        input.process(&PlatformEvent::Window(WindowEvent::Focused(false)));
        assert!(!input.keyboard().is_pressed(KeyCode::S));
        assert!(input.keyboard().just_released(KeyCode::S));
    }

    #[test]
    fn mouse_button_and_motion() {
        let mut input = InputState::new().expect("init");
        input.begin_frame();
        input.process(&PlatformEvent::Mouse(MouseEvent::Button {
            button: MouseButton::Left,
            state: ButtonState::Pressed,
        }));
        input.process(&PlatformEvent::Mouse(MouseEvent::Moved { x: 4.0, y: 5.0 }));
        assert!(input.mouse().just_pressed(MouseButton::Left));
        assert!(input.mouse().delta().approx_eq_default(Vec2::new(4.0, 5.0)));
    }

    #[test]
    fn window_resize_is_ignored() {
        let mut input = InputState::new().expect("init");
        input.begin_frame();
        input.process(&PlatformEvent::Window(WindowEvent::Resized {
            width: 100,
            height: 100,
        }));
    }
}
