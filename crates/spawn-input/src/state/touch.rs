//! Touch device state: raw touch points with id lifecycle. No gesture detection.

use spawn_core::Vec2;
use spawn_platform::{TouchEvent, TouchPhase as PlatformTouchPhase};

/// Stable identity of a touch contact for its lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TouchId(pub u64);

/// Lifecycle phase of a tracked touch point.
///
/// `Stationary` is synthesized by `begin_frame` for points that received no
/// event in the new frame; it never arrives from the platform directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Started,
    Moved,
    Stationary,
    Ended,
    Cancelled,
}

/// A single tracked touch point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchPoint {
    pub id: TouchId,
    pub position: Vec2,
    pub phase: TouchPhase,
}

/// Raw multi-touch state with a fixed capacity of [`Touch::MAX_TOUCHES`] points.
#[derive(Debug)]
pub struct Touch {
    slots: [Option<TouchPoint>; Touch::MAX_TOUCHES],
}

impl Touch {
    /// Maximum simultaneously-tracked touch points. Events introducing an id
    /// beyond this capacity are dropped (not an error).
    pub const MAX_TOUCHES: usize = 10;

    pub(crate) fn new() -> Self {
        Self {
            slots: [None; Touch::MAX_TOUCHES],
        }
    }

    /// Ages out `Ended`/`Cancelled` points from the previous frame and demotes
    /// any surviving `Started`/`Moved` point to `Stationary`. A `process` call
    /// later in the frame re-promotes a point that receives an event.
    pub(crate) fn begin_frame(&mut self) {
        for slot in self.slots.iter_mut() {
            match slot {
                Some(p) if matches!(p.phase, TouchPhase::Ended | TouchPhase::Cancelled) => {
                    *slot = None;
                }
                Some(p) if matches!(p.phase, TouchPhase::Started | TouchPhase::Moved) => {
                    p.phase = TouchPhase::Stationary;
                }
                _ => {}
            }
        }
    }

    pub(crate) fn process(&mut self, event: &TouchEvent) {
        let id = TouchId(event.id);
        let phase = match event.phase {
            PlatformTouchPhase::Started => TouchPhase::Started,
            PlatformTouchPhase::Moved => TouchPhase::Moved,
            PlatformTouchPhase::Ended => TouchPhase::Ended,
            PlatformTouchPhase::Cancelled => TouchPhase::Cancelled,
        };
        let point = TouchPoint {
            id,
            position: Vec2::new(event.x as f32, event.y as f32),
            phase,
        };

        for p in self.slots.iter_mut().flatten() {
            if p.id == id {
                *p = point;
                return;
            }
        }
        for slot in self.slots.iter_mut() {
            if slot.is_none() {
                *slot = Some(point);
                return;
            }
        }
        // Capacity reached: drop the event for the new id.
    }

    /// Iterates active touch points, including `Ended`/`Cancelled` for the one
    /// frame before they are aged out.
    pub fn active(&self) -> impl Iterator<Item = TouchPoint> + '_ {
        self.slots.iter().filter_map(|s| *s)
    }

    /// Returns the tracked point for `id`, if active.
    pub fn get(&self, id: TouchId) -> Option<TouchPoint> {
        self.slots.iter().flatten().find(|p| p.id == id).copied()
    }

    /// Number of active touch points.
    pub fn count(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(id: u64, phase: PlatformTouchPhase, x: f64, y: f64) -> TouchEvent {
        TouchEvent { phase, id, x, y }
    }

    #[test]
    fn lifecycle_started_moved_ended() {
        let mut t = Touch::new();
        t.process(&ev(1, PlatformTouchPhase::Started, 0.0, 0.0));
        assert_eq!(t.count(), 1);
        assert_eq!(t.get(TouchId(1)).unwrap().phase, TouchPhase::Started);

        t.begin_frame();
        assert_eq!(t.get(TouchId(1)).unwrap().phase, TouchPhase::Stationary);

        t.process(&ev(1, PlatformTouchPhase::Moved, 5.0, 6.0));
        assert_eq!(t.get(TouchId(1)).unwrap().phase, TouchPhase::Moved);

        t.begin_frame();
        t.process(&ev(1, PlatformTouchPhase::Ended, 5.0, 6.0));
        assert_eq!(t.get(TouchId(1)).unwrap().phase, TouchPhase::Ended);

        t.begin_frame();
        assert_eq!(t.count(), 0);
        assert!(t.get(TouchId(1)).is_none());
    }

    #[test]
    fn capacity_overflow_drops_without_panic() {
        let mut t = Touch::new();
        for i in 0..(Touch::MAX_TOUCHES as u64 + 5) {
            t.process(&ev(i, PlatformTouchPhase::Started, 0.0, 0.0));
        }
        assert_eq!(t.count(), Touch::MAX_TOUCHES);
    }

    #[test]
    fn active_iterates_all() {
        let mut t = Touch::new();
        t.process(&ev(1, PlatformTouchPhase::Started, 1.0, 1.0));
        t.process(&ev(2, PlatformTouchPhase::Started, 2.0, 2.0));
        assert_eq!(t.active().count(), 2);
    }
}
