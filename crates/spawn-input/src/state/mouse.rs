//! Mouse device state with per-frame edge tracking and accumulated motion.

use spawn_core::Vec2;
use spawn_platform::{MouseButton, ScrollDelta};

/// Number of named mouse buttons (`Left`, `Right`, `Middle`, `Back`, `Forward`).
const NAMED_BUTTONS: usize = 5;
/// Fixed capacity for distinct `MouseButton::Other(code)` slots.
const OTHER_CAPACITY: usize = 8;
/// Nominal pixels per scrolled line, used to convert pixel-precise scroll
/// deltas into the line units that `wheel()` reports (§1.3). Trackpads and
/// high-resolution wheels deliver `ScrollDelta::Pixels`; dividing by this
/// constant keeps `wheel()` in a single, consistent line-based unit.
const PIXELS_PER_LINE: f32 = 16.0;

fn named_index(button: MouseButton) -> Option<usize> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Right => Some(1),
        MouseButton::Middle => Some(2),
        MouseButton::Back => Some(3),
        MouseButton::Forward => Some(4),
        MouseButton::Other(_) => None,
    }
}

/// Cursor position, accumulated motion/scroll, and per-button edge state.
///
/// `delta` and `wheel` accumulate over a frame and reset on `begin_frame`.
/// Button edges follow the §1.2 convention.
#[derive(Debug, Clone, Copy)]
pub struct Mouse {
    held: [bool; NAMED_BUTTONS],
    held_last_frame: [bool; NAMED_BUTTONS],
    other_codes: [u16; OTHER_CAPACITY],
    other_used: [bool; OTHER_CAPACITY],
    other_held: [bool; OTHER_CAPACITY],
    other_held_last_frame: [bool; OTHER_CAPACITY],
    position: Vec2,
    delta: Vec2,
    wheel: Vec2,
}

impl Mouse {
    pub(crate) fn new() -> Self {
        Self {
            held: [false; NAMED_BUTTONS],
            held_last_frame: [false; NAMED_BUTTONS],
            other_codes: [0; OTHER_CAPACITY],
            other_used: [false; OTHER_CAPACITY],
            other_held: [false; OTHER_CAPACITY],
            other_held_last_frame: [false; OTHER_CAPACITY],
            position: Vec2::ZERO,
            delta: Vec2::ZERO,
            wheel: Vec2::ZERO,
        }
    }

    pub(crate) fn begin_frame(&mut self) {
        self.held_last_frame = self.held;
        self.other_held_last_frame = self.other_held;
        self.delta = Vec2::ZERO;
        self.wheel = Vec2::ZERO;
    }

    /// Resolves the fixed-table slot for an `Other` code, claiming a free slot on
    /// first sight. Returns `None` if the table is full; such an event is dropped.
    fn other_slot(&mut self, code: u16) -> Option<usize> {
        for i in 0..OTHER_CAPACITY {
            if self.other_used[i] && self.other_codes[i] == code {
                return Some(i);
            }
        }
        for i in 0..OTHER_CAPACITY {
            if !self.other_used[i] {
                self.other_used[i] = true;
                self.other_codes[i] = code;
                return Some(i);
            }
        }
        None
    }

    pub(crate) fn set_button(&mut self, button: MouseButton, pressed: bool) {
        match named_index(button) {
            Some(i) => self.held[i] = pressed,
            None => {
                if let MouseButton::Other(code) = button {
                    if let Some(i) = self.other_slot(code) {
                        self.other_held[i] = pressed;
                    }
                }
            }
        }
    }

    pub(crate) fn set_position(&mut self, pos: Vec2) {
        self.delta += pos - self.position;
        self.position = pos;
    }

    pub(crate) fn add_wheel(&mut self, delta: ScrollDelta) {
        let (x, y) = match delta {
            ScrollDelta::Lines { x, y } => (x, y),
            ScrollDelta::Pixels { x, y } => (x / PIXELS_PER_LINE, y / PIXELS_PER_LINE),
        };
        self.wheel += Vec2::new(x, y);
    }

    pub(crate) fn release_all(&mut self) {
        self.held = [false; NAMED_BUTTONS];
        self.other_held = [false; OTHER_CAPACITY];
    }

    fn resolve(&self, button: MouseButton) -> Option<(bool, bool)> {
        match named_index(button) {
            Some(i) => Some((self.held[i], self.held_last_frame[i])),
            None => {
                if let MouseButton::Other(code) = button {
                    for i in 0..OTHER_CAPACITY {
                        if self.other_used[i] && self.other_codes[i] == code {
                            return Some((self.other_held[i], self.other_held_last_frame[i]));
                        }
                    }
                }
                None
            }
        }
    }

    /// `true` while the button is held.
    pub fn is_pressed(&self, button: MouseButton) -> bool {
        self.resolve(button).map(|(h, _)| h).unwrap_or(false)
    }

    /// `true` only on the frame the button transitioned up to down.
    pub fn just_pressed(&self, button: MouseButton) -> bool {
        self.resolve(button)
            .map(|(h, last)| h && !last)
            .unwrap_or(false)
    }

    /// `true` only on the frame the button transitioned down to up.
    pub fn just_released(&self, button: MouseButton) -> bool {
        self.resolve(button)
            .map(|(h, last)| !h && last)
            .unwrap_or(false)
    }

    /// Cursor position in logical window pixels, origin top-left.
    pub fn position(&self) -> Vec2 {
        self.position
    }

    /// Motion accumulated this frame; reset on `begin_frame`.
    pub fn delta(&self) -> Vec2 {
        self.delta
    }

    /// Scroll accumulated this frame (`x` horizontal, `y` vertical, in lines);
    /// reset on `begin_frame`.
    pub fn wheel(&self) -> Vec2 {
        self.wheel
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_core::traits::ApproxEq;

    #[test]
    fn named_button_edges() {
        let mut m = Mouse::new();
        m.begin_frame();
        m.set_button(MouseButton::Left, true);
        assert!(m.is_pressed(MouseButton::Left));
        assert!(m.just_pressed(MouseButton::Left));
        m.begin_frame();
        assert!(!m.just_pressed(MouseButton::Left));
        m.set_button(MouseButton::Left, false);
        assert!(m.just_released(MouseButton::Left));
    }

    #[test]
    fn other_button_tracked() {
        let mut m = Mouse::new();
        m.begin_frame();
        m.set_button(MouseButton::Other(9), true);
        assert!(m.is_pressed(MouseButton::Other(9)));
        assert!(m.just_pressed(MouseButton::Other(9)));
        assert!(!m.is_pressed(MouseButton::Other(10)));
    }

    #[test]
    fn delta_accumulates_and_resets() {
        let mut m = Mouse::new();
        m.set_position(Vec2::new(10.0, 10.0));
        m.set_position(Vec2::new(13.0, 14.0));
        assert!(m.delta().approx_eq_default(Vec2::new(13.0, 14.0)));
        assert!(m.position().approx_eq_default(Vec2::new(13.0, 14.0)));
        m.begin_frame();
        assert!(m.delta().approx_eq_default(Vec2::ZERO));
    }

    #[test]
    fn wheel_accumulates_and_resets() {
        let mut m = Mouse::new();
        m.add_wheel(ScrollDelta::Lines { x: 1.0, y: 2.0 });
        m.add_wheel(ScrollDelta::Lines { x: 0.0, y: 1.0 });
        assert!(m.wheel().approx_eq_default(Vec2::new(1.0, 3.0)));
        m.begin_frame();
        assert!(m.wheel().approx_eq_default(Vec2::ZERO));
    }

    #[test]
    fn wheel_pixels_convert_to_lines() {
        let mut m = Mouse::new();
        m.add_wheel(ScrollDelta::Pixels {
            x: PIXELS_PER_LINE,
            y: PIXELS_PER_LINE * 2.0,
        });
        assert!(m.wheel().approx_eq_default(Vec2::new(1.0, 2.0)));
        // Pixel and line deltas accumulate in the same (line) unit.
        m.add_wheel(ScrollDelta::Lines { x: 0.0, y: 1.0 });
        assert!(m.wheel().approx_eq_default(Vec2::new(1.0, 3.0)));
    }
}
