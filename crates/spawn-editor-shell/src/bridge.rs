//! Input bridge: build the UI input snapshot and partition the pointer between
//! the panels and the viewport (one grammar; the viewport acts only when the
//! pointer is not over a panel).

use spawn_core::{Rect, Vec2};
use spawn_input::{InputState, MouseButton};
use spawn_ui::UiInputState;

/// Where the pointer is acting this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerTarget {
    /// Over a docked panel, input routes to the UI only.
    Panel,
    /// Over the viewport, camera/picking/gizmos act (the UI still receives the
    /// snapshot for hover, but does not consume the gesture).
    Viewport,
}

/// Builds the per-frame UI input snapshot from the platform input state.
pub fn ui_input(input: &InputState) -> UiInputState {
    let mouse = input.mouse();
    UiInputState {
        pointer: mouse.position(),
        primary_down: mouse.is_pressed(MouseButton::Left),
        secondary_down: mouse.is_pressed(MouseButton::Right),
        wheel: mouse.wheel(),
    }
}

/// Decides whether the pointer is over the viewport or a panel. With the fixed
/// four-region layout the panels and the viewport are disjoint, so a pointer
/// inside the viewport rect targets the viewport; otherwise a panel.
pub fn pointer_target(viewport: Rect, pointer: Vec2) -> PointerTarget {
    if viewport.contains_point(pointer) {
        PointerTarget::Viewport
    } else {
        PointerTarget::Panel
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_is_exclusive() {
        let viewport = Rect::new(Vec2::new(170.0, 28.0), Vec2::new(1030.0, 702.0));
        assert_eq!(
            pointer_target(viewport, Vec2::new(600.0, 400.0)),
            PointerTarget::Viewport
        );
        // Over the left outliner column.
        assert_eq!(
            pointer_target(viewport, Vec2::new(80.0, 400.0)),
            PointerTarget::Panel
        );
        // Over the top toolbar.
        assert_eq!(
            pointer_target(viewport, Vec2::new(600.0, 10.0)),
            PointerTarget::Panel
        );
    }
}
