//! The per-frame input snapshot resource.
//!
//! spawn-input's `InputState` is engine-owned and not a world resource (it is not
//! `Clone`, it holds gamepad backend state), so systems cannot read it directly. The
//! engine instead publishes a read-only snapshot of the frame's keyboard and mouse
//! state into the world as `Res<InputFrame>`, refreshed at the top of each tick after
//! input is pumped and before any schedule runs, the same mirror-into-the-world
//! pattern used for `Time`. Touch and gamepad state are not mirrored in this phase.

use spawn_ecs::Resource;
use spawn_input::{InputState, Keyboard, Mouse};

/// A read-only snapshot of the current frame's keyboard and mouse state, exposed to
/// systems as `Res<InputFrame>`. Its `keyboard`/`mouse` accessors return the same
/// device-state views as `InputState`, so edge queries (`just_pressed` and friends)
/// behave identically.
#[derive(Debug, Clone, Copy)]
pub struct InputFrame {
    keyboard: Keyboard,
    mouse: Mouse,
}

impl Resource for InputFrame {}

impl InputFrame {
    pub(crate) fn snapshot(input: &InputState) -> Self {
        Self {
            keyboard: *input.keyboard(),
            mouse: *input.mouse(),
        }
    }

    pub fn keyboard(&self) -> &Keyboard {
        &self.keyboard
    }

    pub fn mouse(&self) -> &Mouse {
        &self.mouse
    }
}
