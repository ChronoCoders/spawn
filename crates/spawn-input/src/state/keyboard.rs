//! Keyboard device state with per-frame edge tracking.

use spawn_platform::KeyCode;

/// Number of distinct [`KeyCode`] slots tracked. Must cover every non-`Unknown`
/// variant plus `Unknown`.
const KEY_COUNT: usize = 116;

/// Maps a [`KeyCode`] to a dense, stable index in `[0, KEY_COUNT)`.
///
/// `KeyCode` is `#[non_exhaustive]`; this mapping is total and any variant
/// without an explicit slot folds into the `Unknown` slot rather than panicking.
fn key_index(key: KeyCode) -> usize {
    match key {
        KeyCode::A => 0,
        KeyCode::B => 1,
        KeyCode::C => 2,
        KeyCode::D => 3,
        KeyCode::E => 4,
        KeyCode::F => 5,
        KeyCode::G => 6,
        KeyCode::H => 7,
        KeyCode::I => 8,
        KeyCode::J => 9,
        KeyCode::K => 10,
        KeyCode::L => 11,
        KeyCode::M => 12,
        KeyCode::N => 13,
        KeyCode::O => 14,
        KeyCode::P => 15,
        KeyCode::Q => 16,
        KeyCode::R => 17,
        KeyCode::S => 18,
        KeyCode::T => 19,
        KeyCode::U => 20,
        KeyCode::V => 21,
        KeyCode::W => 22,
        KeyCode::X => 23,
        KeyCode::Y => 24,
        KeyCode::Z => 25,
        KeyCode::Digit0 => 26,
        KeyCode::Digit1 => 27,
        KeyCode::Digit2 => 28,
        KeyCode::Digit3 => 29,
        KeyCode::Digit4 => 30,
        KeyCode::Digit5 => 31,
        KeyCode::Digit6 => 32,
        KeyCode::Digit7 => 33,
        KeyCode::Digit8 => 34,
        KeyCode::Digit9 => 35,
        KeyCode::F1 => 36,
        KeyCode::F2 => 37,
        KeyCode::F3 => 38,
        KeyCode::F4 => 39,
        KeyCode::F5 => 40,
        KeyCode::F6 => 41,
        KeyCode::F7 => 42,
        KeyCode::F8 => 43,
        KeyCode::F9 => 44,
        KeyCode::F10 => 45,
        KeyCode::F11 => 46,
        KeyCode::F12 => 47,
        KeyCode::F13 => 48,
        KeyCode::F14 => 49,
        KeyCode::F15 => 50,
        KeyCode::F16 => 51,
        KeyCode::F17 => 52,
        KeyCode::F18 => 53,
        KeyCode::F19 => 54,
        KeyCode::F20 => 55,
        KeyCode::F21 => 56,
        KeyCode::F22 => 57,
        KeyCode::F23 => 58,
        KeyCode::F24 => 59,
        KeyCode::Escape => 60,
        KeyCode::Enter => 61,
        KeyCode::Space => 62,
        KeyCode::Tab => 63,
        KeyCode::Backspace => 64,
        KeyCode::ArrowUp => 65,
        KeyCode::ArrowDown => 66,
        KeyCode::ArrowLeft => 67,
        KeyCode::ArrowRight => 68,
        KeyCode::ShiftLeft => 69,
        KeyCode::ShiftRight => 70,
        KeyCode::ControlLeft => 71,
        KeyCode::ControlRight => 72,
        KeyCode::AltLeft => 73,
        KeyCode::AltRight => 74,
        KeyCode::SuperLeft => 75,
        KeyCode::SuperRight => 76,
        KeyCode::Insert => 77,
        KeyCode::Delete => 78,
        KeyCode::Home => 79,
        KeyCode::End => 80,
        KeyCode::PageUp => 81,
        KeyCode::PageDown => 82,
        KeyCode::Numpad0 => 83,
        KeyCode::Numpad1 => 84,
        KeyCode::Numpad2 => 85,
        KeyCode::Numpad3 => 86,
        KeyCode::Numpad4 => 87,
        KeyCode::Numpad5 => 88,
        KeyCode::Numpad6 => 89,
        KeyCode::Numpad7 => 90,
        KeyCode::Numpad8 => 91,
        KeyCode::Numpad9 => 92,
        KeyCode::NumpadAdd => 93,
        KeyCode::NumpadSubtract => 94,
        KeyCode::NumpadMultiply => 95,
        KeyCode::NumpadDivide => 96,
        KeyCode::NumpadEnter => 97,
        KeyCode::NumpadDecimal => 98,
        KeyCode::Minus => 99,
        KeyCode::Equal => 100,
        KeyCode::BracketLeft => 101,
        KeyCode::BracketRight => 102,
        KeyCode::Backslash => 103,
        KeyCode::Semicolon => 104,
        KeyCode::Quote => 105,
        KeyCode::Backquote => 106,
        KeyCode::Comma => 107,
        KeyCode::Period => 108,
        KeyCode::Slash => 109,
        KeyCode::CapsLock => 110,
        KeyCode::NumLock => 111,
        KeyCode::ScrollLock => 112,
        KeyCode::PrintScreen => 113,
        KeyCode::Pause => 114,
        KeyCode::Unknown => 115,
        _ => 115,
    }
}

/// Currently-held and previous-frame key state.
///
/// Edge queries follow the §1.2 convention: `begin_frame` copies `held` into
/// `held_last_frame`, so `just_pressed`/`just_released` are edge-true only on
/// the frame the transition's event was processed.
#[derive(Debug)]
pub struct Keyboard {
    held: [bool; KEY_COUNT],
    held_last_frame: [bool; KEY_COUNT],
}

impl Keyboard {
    pub(crate) fn new() -> Self {
        Self {
            held: [false; KEY_COUNT],
            held_last_frame: [false; KEY_COUNT],
        }
    }

    /// Copies current held bits into the previous-frame bits. Called by
    /// [`InputState::begin_frame`](crate::InputState::begin_frame).
    pub(crate) fn begin_frame(&mut self) {
        self.held_last_frame = self.held;
    }

    pub(crate) fn set(&mut self, key: KeyCode, pressed: bool) {
        self.held[key_index(key)] = pressed;
    }

    /// Releases all keys (used on focus loss). Does not touch the previous-frame
    /// bits, so released keys report `just_released` next frame.
    pub(crate) fn release_all(&mut self) {
        self.held = [false; KEY_COUNT];
    }

    /// `true` while the key is held.
    pub fn is_pressed(&self, key: KeyCode) -> bool {
        self.held[key_index(key)]
    }

    /// `true` only on the frame the key transitioned from up to down (not held
    /// last frame, held this frame). OS auto-repeat does not re-trigger this.
    pub fn just_pressed(&self, key: KeyCode) -> bool {
        let i = key_index(key);
        self.held[i] && !self.held_last_frame[i]
    }

    /// `true` only on the frame the key transitioned from down to up.
    pub fn just_released(&self, key: KeyCode) -> bool {
        let i = key_index(key);
        !self.held[i] && self.held_last_frame[i]
    }

    /// Iterates currently-held keys without allocating.
    pub fn pressed_keys(&self) -> impl Iterator<Item = KeyCode> + '_ {
        ALL_KEYS
            .iter()
            .copied()
            .filter(move |&k| self.held[key_index(k)])
    }
}

/// Every tracked key in index order; used for non-allocating iteration.
const ALL_KEYS: [KeyCode; KEY_COUNT] = [
    KeyCode::A,
    KeyCode::B,
    KeyCode::C,
    KeyCode::D,
    KeyCode::E,
    KeyCode::F,
    KeyCode::G,
    KeyCode::H,
    KeyCode::I,
    KeyCode::J,
    KeyCode::K,
    KeyCode::L,
    KeyCode::M,
    KeyCode::N,
    KeyCode::O,
    KeyCode::P,
    KeyCode::Q,
    KeyCode::R,
    KeyCode::S,
    KeyCode::T,
    KeyCode::U,
    KeyCode::V,
    KeyCode::W,
    KeyCode::X,
    KeyCode::Y,
    KeyCode::Z,
    KeyCode::Digit0,
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
    KeyCode::Digit8,
    KeyCode::Digit9,
    KeyCode::F1,
    KeyCode::F2,
    KeyCode::F3,
    KeyCode::F4,
    KeyCode::F5,
    KeyCode::F6,
    KeyCode::F7,
    KeyCode::F8,
    KeyCode::F9,
    KeyCode::F10,
    KeyCode::F11,
    KeyCode::F12,
    KeyCode::F13,
    KeyCode::F14,
    KeyCode::F15,
    KeyCode::F16,
    KeyCode::F17,
    KeyCode::F18,
    KeyCode::F19,
    KeyCode::F20,
    KeyCode::F21,
    KeyCode::F22,
    KeyCode::F23,
    KeyCode::F24,
    KeyCode::Escape,
    KeyCode::Enter,
    KeyCode::Space,
    KeyCode::Tab,
    KeyCode::Backspace,
    KeyCode::ArrowUp,
    KeyCode::ArrowDown,
    KeyCode::ArrowLeft,
    KeyCode::ArrowRight,
    KeyCode::ShiftLeft,
    KeyCode::ShiftRight,
    KeyCode::ControlLeft,
    KeyCode::ControlRight,
    KeyCode::AltLeft,
    KeyCode::AltRight,
    KeyCode::SuperLeft,
    KeyCode::SuperRight,
    KeyCode::Insert,
    KeyCode::Delete,
    KeyCode::Home,
    KeyCode::End,
    KeyCode::PageUp,
    KeyCode::PageDown,
    KeyCode::Numpad0,
    KeyCode::Numpad1,
    KeyCode::Numpad2,
    KeyCode::Numpad3,
    KeyCode::Numpad4,
    KeyCode::Numpad5,
    KeyCode::Numpad6,
    KeyCode::Numpad7,
    KeyCode::Numpad8,
    KeyCode::Numpad9,
    KeyCode::NumpadAdd,
    KeyCode::NumpadSubtract,
    KeyCode::NumpadMultiply,
    KeyCode::NumpadDivide,
    KeyCode::NumpadEnter,
    KeyCode::NumpadDecimal,
    KeyCode::Minus,
    KeyCode::Equal,
    KeyCode::BracketLeft,
    KeyCode::BracketRight,
    KeyCode::Backslash,
    KeyCode::Semicolon,
    KeyCode::Quote,
    KeyCode::Backquote,
    KeyCode::Comma,
    KeyCode::Period,
    KeyCode::Slash,
    KeyCode::CapsLock,
    KeyCode::NumLock,
    KeyCode::ScrollLock,
    KeyCode::PrintScreen,
    KeyCode::Pause,
    KeyCode::Unknown,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices_are_unique_and_in_range() {
        let mut seen = [false; KEY_COUNT];
        for (expected, &k) in ALL_KEYS.iter().enumerate() {
            let i = key_index(k);
            assert_eq!(i, expected);
            assert!(!seen[i]);
            seen[i] = true;
        }
        assert!(seen.iter().all(|&s| s));
    }

    #[test]
    fn press_release_edges() {
        let mut kb = Keyboard::new();
        kb.begin_frame();
        kb.set(KeyCode::Space, true);
        assert!(kb.is_pressed(KeyCode::Space));
        assert!(kb.just_pressed(KeyCode::Space));
        assert!(!kb.just_released(KeyCode::Space));

        kb.begin_frame();
        assert!(kb.is_pressed(KeyCode::Space));
        assert!(!kb.just_pressed(KeyCode::Space));

        kb.set(KeyCode::Space, false);
        assert!(!kb.is_pressed(KeyCode::Space));
        kb.begin_frame();
        kb.set(KeyCode::Space, false);
        assert!(!kb.just_released(KeyCode::Space));
    }

    #[test]
    fn release_all_yields_just_released_next_frame() {
        let mut kb = Keyboard::new();
        kb.set(KeyCode::W, true);
        kb.begin_frame();
        kb.release_all();
        assert!(kb.just_released(KeyCode::W));
    }

    #[test]
    fn pressed_keys_iterates_held() {
        let mut kb = Keyboard::new();
        kb.set(KeyCode::A, true);
        kb.set(KeyCode::Z, true);
        let mut keys: Vec<KeyCode> = kb.pressed_keys().collect();
        keys.sort_by_key(|k| key_index(*k));
        assert_eq!(keys, vec![KeyCode::A, KeyCode::Z]);
    }
}
