//! Input bindings: the device-level sources an action can map to.

use spawn_platform::{KeyCode, MouseButton};

use crate::state::{GamepadAxis, GamepadButton};

/// Direction along a gamepad axis a binding responds to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AxisDirection {
    Positive,
    Negative,
}

/// A single binding source for an action.
///
/// Aggregation across multiple bindings on one action is performed by
/// [`ActionMap`](crate::map::ActionMap), not here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Binding {
    Key(KeyCode),
    MouseButton(MouseButton),
    GamepadButton(GamepadButton),
    /// Pressed when the deadzoned axis value in `direction` exceeds `threshold`;
    /// its analog `value` is that directional magnitude in `[0, 1]`.
    GamepadAxis {
        axis: GamepadAxis,
        direction: AxisDirection,
        threshold: f32,
    },
    /// Composite 2D axis from four keys. Contributes a `Vec2` to `axis2`; its
    /// scalar `value` is the vector length and it is `pressed` when that length
    /// is greater than zero. `up`/`down` map to `+Y`/`-Y`, `right`/`left` to
    /// `+X`/`-X`.
    Composite2D {
        up: KeyCode,
        down: KeyCode,
        left: KeyCode,
        right: KeyCode,
    },
}
