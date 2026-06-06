//! Gamepad state, deadzone handling, and the `gilrs` backend glue.
//!
//! `gilrs` is owned entirely by this module; no `gilrs` type appears in any
//! public signature. Backend events are translated into the internal
//! [`GamepadInput`] seam, which both the live `gilrs` drain and the tests feed,
//! so edge/deadzone logic is exercised without a physical device.

use crate::error::InputResult;

/// Opaque, stable identity of a connected gamepad slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GamepadId(u8);

impl GamepadId {
    #[cfg(test)]
    pub(crate) fn from_index(i: u8) -> Self {
        Self(i)
    }

    #[cfg(not(test))]
    fn from_index(i: u8) -> Self {
        Self(i)
    }

    fn index(self) -> usize {
        self.0 as usize
    }
}

/// Digital gamepad inputs, identified by physical position (Xbox layout names).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GamepadButton {
    South,
    East,
    North,
    West,
    LeftBumper,
    RightBumper,
    LeftTrigger,
    RightTrigger,
    Select,
    Start,
    Mode,
    LeftStick,
    RightStick,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
}

/// Number of distinct [`GamepadButton`] variants.
const BUTTON_COUNT: usize = 17;

fn button_index(button: GamepadButton) -> usize {
    match button {
        GamepadButton::South => 0,
        GamepadButton::East => 1,
        GamepadButton::North => 2,
        GamepadButton::West => 3,
        GamepadButton::LeftBumper => 4,
        GamepadButton::RightBumper => 5,
        GamepadButton::LeftTrigger => 6,
        GamepadButton::RightTrigger => 7,
        GamepadButton::Select => 8,
        GamepadButton::Start => 9,
        GamepadButton::Mode => 10,
        GamepadButton::LeftStick => 11,
        GamepadButton::RightStick => 12,
        GamepadButton::DPadUp => 13,
        GamepadButton::DPadDown => 14,
        GamepadButton::DPadLeft => 15,
        GamepadButton::DPadRight => 16,
    }
}

/// Analog gamepad axes. Sticks are `[-1, 1]`; triggers are `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GamepadAxis {
    LeftStickX,
    LeftStickY,
    RightStickX,
    RightStickY,
    LeftTrigger,
    RightTrigger,
}

/// Number of distinct [`GamepadAxis`] variants.
const AXIS_COUNT: usize = 6;

fn axis_index(axis: GamepadAxis) -> usize {
    match axis {
        GamepadAxis::LeftStickX => 0,
        GamepadAxis::LeftStickY => 1,
        GamepadAxis::RightStickX => 2,
        GamepadAxis::RightStickY => 3,
        GamepadAxis::LeftTrigger => 4,
        GamepadAxis::RightTrigger => 5,
    }
}

/// Connect/disconnect transition surfaced for one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GamepadEvent {
    Connected(GamepadId),
    Disconnected(GamepadId),
}

/// Radial single-axis deadzone parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Deadzone {
    pub inner: f32,
    pub outer: f32,
}

impl Default for Deadzone {
    fn default() -> Self {
        Self {
            inner: 0.15,
            outer: 1.0,
        }
    }
}

impl Deadzone {
    /// Applies a radial-scaled deadzone to a single axis value.
    ///
    /// `|raw| <= inner` yields `0.0`; otherwise the magnitude is rescaled from
    /// `[inner, outer]` to `[0, 1]`, sign-preserving and clamped to `1.0`:
    /// `sign(raw) * clamp((|raw| - inner) / (outer - inner), 0, 1)`.
    pub fn apply(self, raw: f32) -> f32 {
        let mag = raw.abs();
        if mag <= self.inner {
            return 0.0;
        }
        let span = self.outer - self.inner;
        if span <= 0.0 {
            return raw.signum();
        }
        let scaled = ((mag - self.inner) / span).clamp(0.0, 1.0);
        raw.signum() * scaled
    }
}

/// Internal backend-agnostic gamepad event. Both the `gilrs` drain and tests
/// produce these; the rest of the module is unaware of `gilrs`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum GamepadInput {
    Connected(GamepadId),
    Disconnected(GamepadId),
    Button(GamepadId, GamepadButton, bool),
    Axis(GamepadId, GamepadAxis, f32),
}

#[derive(Debug, Clone, Copy)]
struct PadState {
    connected: bool,
    held: [bool; BUTTON_COUNT],
    held_last_frame: [bool; BUTTON_COUNT],
    raw_axes: [f32; AXIS_COUNT],
}

impl PadState {
    const fn new() -> Self {
        Self {
            connected: false,
            held: [false; BUTTON_COUNT],
            held_last_frame: [false; BUTTON_COUNT],
            raw_axes: [0.0; AXIS_COUNT],
        }
    }
}

/// Fixed-capacity gamepad state, polled once per frame from the backend.
pub struct Gamepads {
    pads: [PadState; Gamepads::MAX_GAMEPADS],
    deadzones: [Deadzone; AXIS_COUNT],
    events: Vec<GamepadEvent>,
    backend: Backend,
}

impl core::fmt::Debug for Gamepads {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Gamepads")
            .field("connected", &self.connected().collect::<Vec<_>>())
            .field("events", &self.events)
            .finish_non_exhaustive()
    }
}

impl Gamepads {
    /// Maximum simultaneously-tracked gamepads.
    pub const MAX_GAMEPADS: usize = 8;

    fn default_deadzones() -> [Deadzone; AXIS_COUNT] {
        let stick = Deadzone::default();
        let trigger = Deadzone {
            inner: 0.0,
            outer: 1.0,
        };
        let mut dz = [stick; AXIS_COUNT];
        dz[axis_index(GamepadAxis::LeftTrigger)] = trigger;
        dz[axis_index(GamepadAxis::RightTrigger)] = trigger;
        dz
    }

    /// Constructs gamepad state, initializing the backend.
    ///
    /// Returns `Err(InputError::BackendInit)` only if the backend reports a hard
    /// failure. On a headless host with no device access the backend falls back
    /// to a no-gamepad mode and construction still succeeds.
    pub(crate) fn new() -> InputResult<Self> {
        let backend = Backend::new()?;
        let mut gamepads = Self {
            pads: [PadState::new(); Gamepads::MAX_GAMEPADS],
            deadzones: Self::default_deadzones(),
            events: Vec::with_capacity(Gamepads::MAX_GAMEPADS * 2),
            backend,
        };
        Backend::seed(&mut gamepads);
        Ok(gamepads)
    }

    /// Advances edge state and drains backend events into this frame's state.
    /// Clears the previous frame's connection events.
    pub(crate) fn begin_frame(&mut self) {
        for pad in self.pads.iter_mut() {
            pad.held_last_frame = pad.held;
        }
        self.events.clear();
        Backend::drain(self);
    }

    fn apply(&mut self, input: GamepadInput) {
        match input {
            GamepadInput::Connected(id) => {
                let i = id.index();
                if i < Gamepads::MAX_GAMEPADS && !self.pads[i].connected {
                    self.pads[i].connected = true;
                    self.events.push(GamepadEvent::Connected(id));
                }
            }
            GamepadInput::Disconnected(id) => {
                let i = id.index();
                if i < Gamepads::MAX_GAMEPADS && self.pads[i].connected {
                    self.pads[i] = PadState::new();
                    self.events.push(GamepadEvent::Disconnected(id));
                }
            }
            GamepadInput::Button(id, button, pressed) => {
                let i = id.index();
                if i < Gamepads::MAX_GAMEPADS && self.pads[i].connected {
                    self.pads[i].held[button_index(button)] = pressed;
                }
            }
            GamepadInput::Axis(id, axis, value) => {
                let i = id.index();
                if i < Gamepads::MAX_GAMEPADS && self.pads[i].connected {
                    self.pads[i].raw_axes[axis_index(axis)] = value;
                }
            }
        }
    }

    /// Iterates currently-connected gamepad ids in ascending slot order.
    pub fn connected(&self) -> impl Iterator<Item = GamepadId> + '_ {
        (0..Gamepads::MAX_GAMEPADS)
            .filter(move |&i| self.pads[i].connected)
            .map(|i| GamepadId::from_index(i as u8))
    }

    /// `true` if `id` is currently connected.
    pub fn is_connected(&self, id: GamepadId) -> bool {
        let i = id.index();
        i < Gamepads::MAX_GAMEPADS && self.pads[i].connected
    }

    /// Connect/disconnect transitions that occurred this frame; cleared on
    /// `begin_frame`.
    pub fn connection_events(&self) -> &[GamepadEvent] {
        &self.events
    }

    /// `true` while `button` is held on `id` (false for a disconnected id).
    pub fn is_pressed(&self, id: GamepadId, button: GamepadButton) -> bool {
        let i = id.index();
        i < Gamepads::MAX_GAMEPADS
            && self.pads[i].connected
            && self.pads[i].held[button_index(button)]
    }

    /// `true` only on the frame `button` transitioned up to down.
    pub fn just_pressed(&self, id: GamepadId, button: GamepadButton) -> bool {
        let i = id.index();
        if i >= Gamepads::MAX_GAMEPADS {
            return false;
        }
        let b = button_index(button);
        self.pads[i].held[b] && !self.pads[i].held_last_frame[b]
    }

    /// `true` only on the frame `button` transitioned down to up.
    pub fn just_released(&self, id: GamepadId, button: GamepadButton) -> bool {
        let i = id.index();
        if i >= Gamepads::MAX_GAMEPADS {
            return false;
        }
        let b = button_index(button);
        !self.pads[i].held[b] && self.pads[i].held_last_frame[b]
    }

    /// Deadzoned axis value: sticks `[-1, 1]`, triggers `[0, 1]`. Returns `0.0`
    /// for a disconnected id.
    pub fn axis(&self, id: GamepadId, axis: GamepadAxis) -> f32 {
        let i = id.index();
        if i >= Gamepads::MAX_GAMEPADS || !self.pads[i].connected {
            return 0.0;
        }
        let a = axis_index(axis);
        self.deadzones[a].apply(self.pads[i].raw_axes[a])
    }

    /// Lowest-id connected gamepad, if any.
    pub fn first(&self) -> Option<GamepadId> {
        self.connected().next()
    }

    /// Sets the deadzone for a single axis. Takes effect on the next `axis`
    /// query.
    pub fn set_deadzone(&mut self, axis: GamepadAxis, dz: Deadzone) {
        self.deadzones[axis_index(axis)] = dz;
    }

    #[cfg(test)]
    pub(crate) fn inject(&mut self, input: GamepadInput) {
        self.apply(input);
    }
}

// --- Backend boundary ---------------------------------------------------------

#[cfg(not(test))]
struct Backend {
    gilrs: Option<gilrs::Gilrs>,
    id_map: Vec<gilrs::GamepadId>,
}

#[cfg(not(test))]
impl Backend {
    fn new() -> InputResult<Self> {
        // A headless host (no udev / no devices) must not be a hard error:
        // `NotImplemented` yields a usable no-gamepad context. A genuine backend
        // failure (`Other`) surfaces as `BackendInit`; the `gilrs` error is not
        // exposed.
        let gilrs = match gilrs::Gilrs::new() {
            Ok(g) => Some(g),
            Err(gilrs::Error::NotImplemented(g)) => Some(g),
            Err(gilrs::Error::InvalidAxisToBtn) => None,
            Err(gilrs::Error::Other(_)) => {
                return Err(crate::error::InputError::BackendInit {
                    context: "gilrs backend initialization failed",
                });
            }
            Err(_) => None,
        };
        Ok(Self {
            gilrs,
            id_map: Vec::with_capacity(Gamepads::MAX_GAMEPADS),
        })
    }

    fn slot_for(&mut self, gid: gilrs::GamepadId) -> Option<GamepadId> {
        if let Some(i) = self.id_map.iter().position(|&g| g == gid) {
            return Some(GamepadId::from_index(i as u8));
        }
        if self.id_map.len() < Gamepads::MAX_GAMEPADS {
            self.id_map.push(gid);
            return Some(GamepadId::from_index((self.id_map.len() - 1) as u8));
        }
        None
    }

    /// One-shot seed of gamepads already present at startup, run during
    /// construction (allocation here is setup-path, not per-frame).
    fn seed(pads: &mut Gamepads) {
        let present: Vec<gilrs::GamepadId> = match pads.backend.gilrs.as_ref() {
            Some(g) => g.gamepads().map(|(id, _)| id).collect(),
            None => Vec::new(),
        };
        for gid in present {
            if let Some(id) = pads.backend.slot_for(gid) {
                pads.apply(GamepadInput::Connected(id));
            }
        }
        // Connections surfaced here belong to construction, not a frame.
        pads.events.clear();
    }

    fn drain(pads: &mut Gamepads) {
        while let Some(event) = pads.backend.gilrs.as_mut().and_then(|g| g.next_event()) {
            let id = match pads.backend.slot_for(event.id) {
                Some(id) => id,
                None => continue,
            };
            if let Some(input) = translate_event(id, &event.event) {
                pads.apply(input);
            }
        }
    }
}

#[cfg(not(test))]
fn translate_event(id: GamepadId, event: &gilrs::EventType) -> Option<GamepadInput> {
    use gilrs::ev::EventType as E;
    match *event {
        E::Connected => Some(GamepadInput::Connected(id)),
        E::Disconnected => Some(GamepadInput::Disconnected(id)),
        E::ButtonPressed(b, _) => translate_button(b).map(|b| GamepadInput::Button(id, b, true)),
        E::ButtonReleased(b, _) => translate_button(b).map(|b| GamepadInput::Button(id, b, false)),
        E::AxisChanged(a, v, _) => translate_axis(a).map(|a| GamepadInput::Axis(id, a, v)),
        E::ButtonChanged(b, v, _) => {
            translate_trigger_axis(b).map(|a| GamepadInput::Axis(id, a, v))
        }
        _ => None,
    }
}

#[cfg(not(test))]
fn translate_button(b: gilrs::Button) -> Option<GamepadButton> {
    use gilrs::Button as B;
    Some(match b {
        B::South => GamepadButton::South,
        B::East => GamepadButton::East,
        B::North => GamepadButton::North,
        B::West => GamepadButton::West,
        B::LeftTrigger => GamepadButton::LeftBumper,
        B::RightTrigger => GamepadButton::RightBumper,
        B::LeftTrigger2 => GamepadButton::LeftTrigger,
        B::RightTrigger2 => GamepadButton::RightTrigger,
        B::Select => GamepadButton::Select,
        B::Start => GamepadButton::Start,
        B::Mode => GamepadButton::Mode,
        B::LeftThumb => GamepadButton::LeftStick,
        B::RightThumb => GamepadButton::RightStick,
        B::DPadUp => GamepadButton::DPadUp,
        B::DPadDown => GamepadButton::DPadDown,
        B::DPadLeft => GamepadButton::DPadLeft,
        B::DPadRight => GamepadButton::DPadRight,
        _ => return None,
    })
}

#[cfg(not(test))]
fn translate_axis(a: gilrs::Axis) -> Option<GamepadAxis> {
    use gilrs::Axis as A;
    Some(match a {
        A::LeftStickX => GamepadAxis::LeftStickX,
        A::LeftStickY => GamepadAxis::LeftStickY,
        A::RightStickX => GamepadAxis::RightStickX,
        A::RightStickY => GamepadAxis::RightStickY,
        A::LeftZ => GamepadAxis::LeftTrigger,
        A::RightZ => GamepadAxis::RightTrigger,
        _ => return None,
    })
}

#[cfg(not(test))]
fn translate_trigger_axis(b: gilrs::Button) -> Option<GamepadAxis> {
    use gilrs::Button as B;
    Some(match b {
        B::LeftTrigger2 => GamepadAxis::LeftTrigger,
        B::RightTrigger2 => GamepadAxis::RightTrigger,
        _ => return None,
    })
}

// In test builds the backend is inert: no `gilrs` init (no udev on the CI/WSL
// host), state is driven purely through `Gamepads::inject`. The `live` flag is
// read so the field is never dead.
#[cfg(test)]
struct Backend {
    live: bool,
}

#[cfg(test)]
impl Backend {
    fn new() -> InputResult<Self> {
        Ok(Self { live: false })
    }

    fn seed(_pads: &mut Gamepads) {}

    fn drain(pads: &mut Gamepads) {
        debug_assert!(!pads.backend.live);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_core::traits::ApproxEq;

    #[test]
    fn deadzone_inner_zeroes() {
        let dz = Deadzone::default();
        assert!(dz.apply(0.1).approx_eq_default(0.0));
        assert!(dz.apply(0.15).approx_eq_default(0.0));
    }

    #[test]
    fn deadzone_outer_is_unit() {
        let dz = Deadzone::default();
        assert!(dz.apply(1.0).approx_eq_default(1.0));
        assert!(dz.apply(-1.0).approx_eq_default(-1.0));
    }

    #[test]
    fn deadzone_rescales_endpoint_at_inner() {
        let dz = Deadzone::default();
        // Just above inner is near zero; mid is the rescaled fraction.
        let v = dz.apply(0.575);
        let expected = (0.575 - 0.15) / (1.0 - 0.15);
        assert!(v.approx_eq_default(expected));
    }

    #[test]
    fn deadzone_monotonic_and_sign_preserving() {
        let dz = Deadzone::default();
        assert!(dz.apply(0.5) > dz.apply(0.3));
        assert!(dz.apply(-0.5) < 0.0);
        assert!(dz.apply(-0.5).approx_eq_default(-dz.apply(0.5)));
    }

    fn pad() -> Gamepads {
        Gamepads::new().expect("test backend init")
    }

    #[test]
    fn connect_disconnect_events() {
        let mut g = pad();
        let id = GamepadId::from_index(0);
        g.inject(GamepadInput::Connected(id));
        assert!(g.is_connected(id));
        assert_eq!(g.first(), Some(id));
        g.inject(GamepadInput::Disconnected(id));
        assert!(!g.is_connected(id));
    }

    #[test]
    fn button_edges() {
        let mut g = pad();
        let id = GamepadId::from_index(0);
        g.inject(GamepadInput::Connected(id));
        g.begin_frame();
        g.inject(GamepadInput::Button(id, GamepadButton::South, true));
        assert!(g.is_pressed(id, GamepadButton::South));
        assert!(g.just_pressed(id, GamepadButton::South));
        g.begin_frame();
        assert!(!g.just_pressed(id, GamepadButton::South));
        g.inject(GamepadInput::Button(id, GamepadButton::South, false));
        assert!(g.just_released(id, GamepadButton::South));
    }

    #[test]
    fn axis_deadzoned() {
        let mut g = pad();
        let id = GamepadId::from_index(0);
        g.inject(GamepadInput::Connected(id));
        g.inject(GamepadInput::Axis(id, GamepadAxis::LeftStickX, 0.1));
        assert!(g.axis(id, GamepadAxis::LeftStickX).approx_eq_default(0.0));
        g.inject(GamepadInput::Axis(id, GamepadAxis::LeftStickX, 1.0));
        assert!(g.axis(id, GamepadAxis::LeftStickX).approx_eq_default(1.0));
    }

    #[test]
    fn trigger_axis_has_zero_inner_by_default() {
        let mut g = pad();
        let id = GamepadId::from_index(0);
        g.inject(GamepadInput::Connected(id));
        g.inject(GamepadInput::Axis(id, GamepadAxis::LeftTrigger, 0.05));
        assert!(g.axis(id, GamepadAxis::LeftTrigger).approx_eq_default(0.05));
    }
}
