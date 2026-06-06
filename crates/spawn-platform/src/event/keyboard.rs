//! Keyboard events and the physical (layout-independent) key code.

use super::ButtonState;

/// A keyboard key state change.
///
/// `key` is the physical key (US-QWERTY position), independent of layout.
/// `repeat` is `true` when the OS generated the event from auto-repeat rather
/// than a fresh press.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyboardEvent {
    pub key: KeyCode,
    pub state: ButtonState,
    pub repeat: bool,
}

/// Physical, layout-independent key identity matching the US-QWERTY position.
///
/// Maps from winit `PhysicalKey::Code(KeyCode)`. `Unknown` covers any physical
/// key with no spawn mapping, so translation is total and never panics. Text and
/// logical-key (layout-dependent) mapping are out of scope for Phase 1.
/// `#[non_exhaustive]` so later phases may extend without a breaking change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum KeyCode {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    F13,
    F14,
    F15,
    F16,
    F17,
    F18,
    F19,
    F20,
    F21,
    F22,
    F23,
    F24,
    Escape,
    Enter,
    Space,
    Tab,
    Backspace,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ShiftLeft,
    ShiftRight,
    ControlLeft,
    ControlRight,
    AltLeft,
    AltRight,
    SuperLeft,
    SuperRight,
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,
    NumpadAdd,
    NumpadSubtract,
    NumpadMultiply,
    NumpadDivide,
    NumpadEnter,
    NumpadDecimal,
    Minus,
    Equal,
    BracketLeft,
    BracketRight,
    Backslash,
    Semicolon,
    Quote,
    Backquote,
    Comma,
    Period,
    Slash,
    CapsLock,
    NumLock,
    ScrollLock,
    PrintScreen,
    Pause,
    Unknown,
}

/// Translates a winit physical key into the engine `KeyCode`.
///
/// Total: any key without a spawn mapping (including non-`Code` variants such as
/// `PhysicalKey::Unidentified`) maps to `KeyCode::Unknown`.
pub(crate) fn translate_key(key: winit::keyboard::PhysicalKey) -> KeyCode {
    use winit::keyboard::KeyCode as W;
    use winit::keyboard::PhysicalKey;

    let code = match key {
        PhysicalKey::Code(code) => code,
        PhysicalKey::Unidentified(_) => return KeyCode::Unknown,
    };

    match code {
        W::KeyA => KeyCode::A,
        W::KeyB => KeyCode::B,
        W::KeyC => KeyCode::C,
        W::KeyD => KeyCode::D,
        W::KeyE => KeyCode::E,
        W::KeyF => KeyCode::F,
        W::KeyG => KeyCode::G,
        W::KeyH => KeyCode::H,
        W::KeyI => KeyCode::I,
        W::KeyJ => KeyCode::J,
        W::KeyK => KeyCode::K,
        W::KeyL => KeyCode::L,
        W::KeyM => KeyCode::M,
        W::KeyN => KeyCode::N,
        W::KeyO => KeyCode::O,
        W::KeyP => KeyCode::P,
        W::KeyQ => KeyCode::Q,
        W::KeyR => KeyCode::R,
        W::KeyS => KeyCode::S,
        W::KeyT => KeyCode::T,
        W::KeyU => KeyCode::U,
        W::KeyV => KeyCode::V,
        W::KeyW => KeyCode::W,
        W::KeyX => KeyCode::X,
        W::KeyY => KeyCode::Y,
        W::KeyZ => KeyCode::Z,
        W::Digit0 => KeyCode::Digit0,
        W::Digit1 => KeyCode::Digit1,
        W::Digit2 => KeyCode::Digit2,
        W::Digit3 => KeyCode::Digit3,
        W::Digit4 => KeyCode::Digit4,
        W::Digit5 => KeyCode::Digit5,
        W::Digit6 => KeyCode::Digit6,
        W::Digit7 => KeyCode::Digit7,
        W::Digit8 => KeyCode::Digit8,
        W::Digit9 => KeyCode::Digit9,
        W::F1 => KeyCode::F1,
        W::F2 => KeyCode::F2,
        W::F3 => KeyCode::F3,
        W::F4 => KeyCode::F4,
        W::F5 => KeyCode::F5,
        W::F6 => KeyCode::F6,
        W::F7 => KeyCode::F7,
        W::F8 => KeyCode::F8,
        W::F9 => KeyCode::F9,
        W::F10 => KeyCode::F10,
        W::F11 => KeyCode::F11,
        W::F12 => KeyCode::F12,
        W::F13 => KeyCode::F13,
        W::F14 => KeyCode::F14,
        W::F15 => KeyCode::F15,
        W::F16 => KeyCode::F16,
        W::F17 => KeyCode::F17,
        W::F18 => KeyCode::F18,
        W::F19 => KeyCode::F19,
        W::F20 => KeyCode::F20,
        W::F21 => KeyCode::F21,
        W::F22 => KeyCode::F22,
        W::F23 => KeyCode::F23,
        W::F24 => KeyCode::F24,
        W::Escape => KeyCode::Escape,
        W::Enter => KeyCode::Enter,
        W::Space => KeyCode::Space,
        W::Tab => KeyCode::Tab,
        W::Backspace => KeyCode::Backspace,
        W::ArrowUp => KeyCode::ArrowUp,
        W::ArrowDown => KeyCode::ArrowDown,
        W::ArrowLeft => KeyCode::ArrowLeft,
        W::ArrowRight => KeyCode::ArrowRight,
        W::ShiftLeft => KeyCode::ShiftLeft,
        W::ShiftRight => KeyCode::ShiftRight,
        W::ControlLeft => KeyCode::ControlLeft,
        W::ControlRight => KeyCode::ControlRight,
        W::AltLeft => KeyCode::AltLeft,
        W::AltRight => KeyCode::AltRight,
        W::SuperLeft => KeyCode::SuperLeft,
        W::SuperRight => KeyCode::SuperRight,
        W::Insert => KeyCode::Insert,
        W::Delete => KeyCode::Delete,
        W::Home => KeyCode::Home,
        W::End => KeyCode::End,
        W::PageUp => KeyCode::PageUp,
        W::PageDown => KeyCode::PageDown,
        W::Numpad0 => KeyCode::Numpad0,
        W::Numpad1 => KeyCode::Numpad1,
        W::Numpad2 => KeyCode::Numpad2,
        W::Numpad3 => KeyCode::Numpad3,
        W::Numpad4 => KeyCode::Numpad4,
        W::Numpad5 => KeyCode::Numpad5,
        W::Numpad6 => KeyCode::Numpad6,
        W::Numpad7 => KeyCode::Numpad7,
        W::Numpad8 => KeyCode::Numpad8,
        W::Numpad9 => KeyCode::Numpad9,
        W::NumpadAdd => KeyCode::NumpadAdd,
        W::NumpadSubtract => KeyCode::NumpadSubtract,
        W::NumpadMultiply => KeyCode::NumpadMultiply,
        W::NumpadDivide => KeyCode::NumpadDivide,
        W::NumpadEnter => KeyCode::NumpadEnter,
        W::NumpadDecimal => KeyCode::NumpadDecimal,
        W::Minus => KeyCode::Minus,
        W::Equal => KeyCode::Equal,
        W::BracketLeft => KeyCode::BracketLeft,
        W::BracketRight => KeyCode::BracketRight,
        W::Backslash => KeyCode::Backslash,
        W::Semicolon => KeyCode::Semicolon,
        W::Quote => KeyCode::Quote,
        W::Backquote => KeyCode::Backquote,
        W::Comma => KeyCode::Comma,
        W::Period => KeyCode::Period,
        W::Slash => KeyCode::Slash,
        W::CapsLock => KeyCode::CapsLock,
        W::NumLock => KeyCode::NumLock,
        W::ScrollLock => KeyCode::ScrollLock,
        W::PrintScreen => KeyCode::PrintScreen,
        W::Pause => KeyCode::Pause,
        _ => KeyCode::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::{KeyCode as W, NativeKeyCode, PhysicalKey};

    fn code(c: W) -> KeyCode {
        translate_key(PhysicalKey::Code(c))
    }

    #[test]
    fn letters_map() {
        assert_eq!(code(W::KeyA), KeyCode::A);
        assert_eq!(code(W::KeyZ), KeyCode::Z);
    }

    #[test]
    fn digits_map() {
        assert_eq!(code(W::Digit0), KeyCode::Digit0);
        assert_eq!(code(W::Digit9), KeyCode::Digit9);
    }

    #[test]
    fn function_keys_map() {
        assert_eq!(code(W::F1), KeyCode::F1);
        assert_eq!(code(W::F12), KeyCode::F12);
        assert_eq!(code(W::F24), KeyCode::F24);
    }

    #[test]
    fn modifiers_map() {
        assert_eq!(code(W::ShiftLeft), KeyCode::ShiftLeft);
        assert_eq!(code(W::ControlRight), KeyCode::ControlRight);
        assert_eq!(code(W::SuperLeft), KeyCode::SuperLeft);
    }

    #[test]
    fn arrows_map() {
        assert_eq!(code(W::ArrowUp), KeyCode::ArrowUp);
        assert_eq!(code(W::ArrowLeft), KeyCode::ArrowLeft);
    }

    #[test]
    fn numpad_maps() {
        assert_eq!(code(W::Numpad0), KeyCode::Numpad0);
        assert_eq!(code(W::NumpadEnter), KeyCode::NumpadEnter);
        assert_eq!(code(W::NumpadDecimal), KeyCode::NumpadDecimal);
    }

    #[test]
    fn punctuation_maps() {
        assert_eq!(code(W::Minus), KeyCode::Minus);
        assert_eq!(code(W::Slash), KeyCode::Slash);
        assert_eq!(code(W::Backquote), KeyCode::Backquote);
    }

    #[test]
    fn locks_and_system_keys_map() {
        assert_eq!(code(W::CapsLock), KeyCode::CapsLock);
        assert_eq!(code(W::PrintScreen), KeyCode::PrintScreen);
        assert_eq!(code(W::Pause), KeyCode::Pause);
    }

    #[test]
    fn unmapped_code_is_unknown() {
        // `Fn` has no spawn mapping in Phase 1.
        assert_eq!(code(W::Fn), KeyCode::Unknown);
    }

    #[test]
    fn unidentified_is_unknown() {
        let key = PhysicalKey::Unidentified(NativeKeyCode::Unidentified);
        assert_eq!(translate_key(key), KeyCode::Unknown);
    }
}
