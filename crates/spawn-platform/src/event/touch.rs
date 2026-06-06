//! Touch events.

/// A single touch point update.
///
/// `id` identifies the contact for its lifetime (press through release). `x`/`y`
/// are physical pixels relative to the window, origin top-left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchEvent {
    pub phase: TouchPhase,
    pub id: u64,
    pub x: f64,
    pub y: f64,
}

/// Lifecycle phase of a touch contact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TouchPhase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

pub(crate) fn translate_phase(phase: winit::event::TouchPhase) -> TouchPhase {
    match phase {
        winit::event::TouchPhase::Started => TouchPhase::Started,
        winit::event::TouchPhase::Moved => TouchPhase::Moved,
        winit::event::TouchPhase::Ended => TouchPhase::Ended,
        winit::event::TouchPhase::Cancelled => TouchPhase::Cancelled,
    }
}

pub(crate) fn translate_touch(touch: winit::event::Touch) -> TouchEvent {
    TouchEvent {
        phase: translate_phase(touch.phase),
        id: touch.id,
        x: touch.location.x,
        y: touch.location.y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_round_trip() {
        assert_eq!(
            translate_phase(winit::event::TouchPhase::Started),
            TouchPhase::Started
        );
        assert_eq!(
            translate_phase(winit::event::TouchPhase::Moved),
            TouchPhase::Moved
        );
        assert_eq!(
            translate_phase(winit::event::TouchPhase::Ended),
            TouchPhase::Ended
        );
        assert_eq!(
            translate_phase(winit::event::TouchPhase::Cancelled),
            TouchPhase::Cancelled
        );
    }

    #[test]
    fn touch_translates_position_and_id() {
        let touch = winit::event::Touch {
            device_id: winit::event::DeviceId::dummy(),
            phase: winit::event::TouchPhase::Started,
            location: winit::dpi::PhysicalPosition::new(10.0, 20.0),
            force: None,
            id: 42,
        };
        let translated = translate_touch(touch);
        assert_eq!(
            translated,
            TouchEvent {
                phase: TouchPhase::Started,
                id: 42,
                x: 10.0,
                y: 20.0,
            }
        );
    }
}
