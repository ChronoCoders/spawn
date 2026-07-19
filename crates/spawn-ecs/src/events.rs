//! Double-buffered events: `Events<T>` (a resource), `EventWriter<T>`, and
//! `EventReader<T>`.
//!
//! `Events<T>` keeps two buffers, this frame's and the previous frame's,
//! swapped once per frame at the end-of-run stage boundary
//! ([`World::update_events`](crate::world::World::update_events), driven by
//! [`Schedule::run`](crate::schedule::Schedule::run)). An event sent in frame N
//! is readable in frames N and N+1, then dropped, so any reader that runs at
//! least once per frame sees every event exactly once. Because `Events<T>` is a
//! resource, `EventWriter<T>` is a resource write and `EventReader<T>` a resource
//! read: writers of the same type serialize (deterministic append order) and
//! readers of the same type share a batch, each advancing its own cursor.

use crate::error::{EcsError, EcsResult};
use crate::resource::{Res, ResMut, Resource};
use crate::system::{Access, SystemParam};
use crate::world::World;

/// Marker trait for types delivered as events. Explicit opt-in per type (no
/// blanket impl), mirroring `Component`/`Resource` so the set of event types
/// stays auditable.
pub trait Event: Send + Sync + 'static {}

/// One buffer of events plus the global index of its first element.
struct EventBuffer<T> {
    start: usize,
    events: Vec<T>,
}

impl<T> EventBuffer<T> {
    fn new() -> Self {
        Self {
            start: 0,
            events: Vec::new(),
        }
    }
}

/// A double-buffered event queue, stored as a world resource (one per event
/// type). `front` holds the current frame's events, `back` the previous frame's;
/// [`update`](Events::update) swaps them once per frame.
pub struct Events<T: Event> {
    front: EventBuffer<T>,
    back: EventBuffer<T>,
    event_count: usize,
}

impl<T: Event> Default for Events<T> {
    fn default() -> Self {
        Self {
            front: EventBuffer::new(),
            back: EventBuffer::new(),
            event_count: 0,
        }
    }
}

impl<T: Event> Resource for Events<T> {}

impl<T: Event> Events<T> {
    /// Appends one event to the current frame's buffer.
    pub fn send(&mut self, event: T) {
        self.front.events.push(event);
        self.event_count += 1;
    }

    /// Appends a batch of events to the current frame's buffer.
    pub fn send_batch<I: IntoIterator<Item = T>>(&mut self, events: I) {
        for event in events {
            self.send(event);
        }
    }

    /// One frame swap: the previous frame's buffer is dropped and reused as the
    /// new empty current buffer; this frame's buffer becomes the previous one.
    /// Capacity is retained (cleared, not freed), so steady-state volume is
    /// allocation-free.
    pub fn update(&mut self) {
        std::mem::swap(&mut self.front, &mut self.back);
        self.front.events.clear();
        self.front.start = self.event_count;
    }

    /// Total retained events across both buffers.
    pub fn len(&self) -> usize {
        self.front.events.len() + self.back.events.len()
    }

    /// Returns `true` iff no events are retained in either buffer.
    pub fn is_empty(&self) -> bool {
        self.front.events.is_empty() && self.back.events.is_empty()
    }

    /// Drops all retained events (both buffers), retaining capacity. For
    /// teardown/tests; the monotonic event count is preserved.
    pub fn clear(&mut self) {
        self.front.events.clear();
        self.back.events.clear();
        self.front.start = self.event_count;
        self.back.start = self.event_count;
    }

    /// The global index of the oldest still-retained event.
    fn oldest(&self) -> usize {
        self.back.start
    }

    /// Yields unread events for `cursor` in send order (previous-frame buffer
    /// before current-frame buffer), then advances the cursor to the latest
    /// event. A cursor behind the retained window is clamped forward, so a
    /// dropped event is never indexed.
    fn read<'a>(&'a self, cursor: &mut EventCursor) -> impl Iterator<Item = &'a T> {
        let read_from = cursor.last_read.max(self.oldest());
        let back_skip = (read_from - self.back.start).min(self.back.events.len());
        let front_skip = read_from
            .saturating_sub(self.front.start)
            .min(self.front.events.len());
        cursor.last_read = self.event_count;
        self.back.events[back_skip..]
            .iter()
            .chain(self.front.events[front_skip..].iter())
    }

    /// Count of currently-unread events for `cursor`, without advancing it.
    fn unread_len(&self, cursor: &EventCursor) -> usize {
        let read_from = cursor.last_read.max(self.oldest());
        self.event_count.saturating_sub(read_from)
    }
}

/// A reader's cursor: the global index up to which it has consumed events.
/// Persists across frames as an [`EventReader`]'s [`SystemParam::State`], so each
/// reader advances independently. A `Default` cursor (index `0`) is clamped up to
/// the oldest retained event on first read.
#[derive(Default)]
pub struct EventCursor {
    last_read: usize,
}

/// Sends events of type `T`. A resource write of `Events<T>`: writers of the same
/// type serialize in registration order, giving deterministic delivery order.
pub struct EventWriter<'w, T: Event> {
    events: ResMut<'w, Events<T>>,
}

impl<T: Event> EventWriter<'_, T> {
    /// Sends one event, readable this frame and next.
    pub fn send(&mut self, event: T) {
        self.events.send(event);
    }

    /// Sends a batch of events.
    pub fn send_batch<I: IntoIterator<Item = T>>(&mut self, events: I) {
        self.events.send_batch(events);
    }
}

/// Reads events of type `T` through a per-reader cursor. A resource read of
/// `Events<T>`: multiple readers of the same type share a batch and each receive
/// every event exactly once.
pub struct EventReader<'w, 's, T: Event> {
    events: Res<'w, Events<T>>,
    cursor: &'s mut EventCursor,
}

impl<T: Event> EventReader<'_, '_, T> {
    /// Yields unread events in send order and advances this reader's cursor.
    /// Allocation-free (borrows the buffers).
    pub fn read(&mut self) -> impl Iterator<Item = &T> + '_ {
        self.events.read(self.cursor)
    }

    /// Count of events this reader has not yet consumed.
    pub fn len(&self) -> usize {
        self.events.unread_len(self.cursor)
    }

    /// Returns `true` iff this reader has no unread events.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Per-event-type updater stored by `World::init_event` and run by
/// `World::update_events`; swaps the type's double buffer once per frame.
pub(crate) fn events_updater<T: Event>(world: &mut World) {
    if let Some(mut events) = world.get_resource_mut::<Events<T>>() {
        events.update();
    }
}

impl<T: Event> SystemParam for EventWriter<'_, T> {
    type State = ();
    type Item<'w, 's> = EventWriter<'w, T>;

    fn resolve_access(world: &World, access: &mut Access) -> EcsResult<()> {
        match world.resource_id::<Events<T>>() {
            Some(id) => {
                access.add_resource_write(id);
                Ok(())
            }
            None => Err(EcsError::EventsNotInitialized {
                event: std::any::type_name::<T>(),
            }),
        }
    }

    fn get<'w>(world: &'w World, _state: &mut ()) -> EcsResult<EventWriter<'w, T>> {
        world
            .get_resource_mut::<Events<T>>()
            .map(|events| EventWriter { events })
            .ok_or(EcsError::EventsNotInitialized {
                event: std::any::type_name::<T>(),
            })
    }
}

impl<T: Event> SystemParam for EventReader<'_, '_, T> {
    type State = EventCursor;
    type Item<'w, 's> = EventReader<'w, 's, T>;

    fn resolve_access(world: &World, access: &mut Access) -> EcsResult<()> {
        match world.resource_id::<Events<T>>() {
            Some(id) => {
                access.add_resource_read(id);
                Ok(())
            }
            None => Err(EcsError::EventsNotInitialized {
                event: std::any::type_name::<T>(),
            }),
        }
    }

    fn get<'w, 's>(
        world: &'w World,
        state: &'s mut EventCursor,
    ) -> EcsResult<EventReader<'w, 's, T>> {
        let events = world
            .get_resource::<Events<T>>()
            .ok_or(EcsError::EventsNotInitialized {
                event: std::any::type_name::<T>(),
            })?;
        Ok(EventReader {
            events,
            cursor: state,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ping(u32);
    impl Event for Ping {}

    #[test]
    fn send_then_read_once() {
        let mut events = Events::<Ping>::default();
        events.send(Ping(1));
        events.send(Ping(2));
        let mut cursor = EventCursor::default();
        let got: Vec<u32> = events.read(&mut cursor).map(|p| p.0).collect();
        assert_eq!(got, vec![1, 2]);
        // Second read in the same frame yields nothing.
        assert_eq!(events.read(&mut cursor).count(), 0);
    }

    #[test]
    fn independent_cursors() {
        let mut events = Events::<Ping>::default();
        events.send(Ping(7));
        let mut a = EventCursor::default();
        let mut b = EventCursor::default();
        assert_eq!(
            events.read(&mut a).map(|p| p.0).collect::<Vec<_>>(),
            vec![7]
        );
        assert_eq!(
            events.read(&mut b).map(|p| p.0).collect::<Vec<_>>(),
            vec![7]
        );
    }

    #[test]
    fn two_frame_lifetime_then_dropped() {
        let mut events = Events::<Ping>::default();
        events.send(Ping(1)); // frame N
        events.update(); // end of N
        let mut late = EventCursor::default();
        // A reader first running in frame N+1 still sees the frame-N event.
        assert_eq!(events.unread_len(&late), 1);
        events.update(); // end of N+1 -> frame-N event dropped
        assert_eq!(events.read(&mut late).count(), 0);
        assert_eq!(events.len(), 0);
    }

    #[test]
    fn no_cross_frame_leak_when_idle() {
        let mut events = Events::<Ping>::default();
        events.send(Ping(1));
        events.update();
        events.update();
        assert!(events.is_empty());
    }

    #[test]
    fn ordering_previous_then_current() {
        let mut events = Events::<Ping>::default();
        events.send(Ping(1)); // frame N
        events.update();
        events.send(Ping(2)); // frame N+1
        let mut cursor = EventCursor::default();
        let got: Vec<u32> = events.read(&mut cursor).map(|p| p.0).collect();
        assert_eq!(
            got,
            vec![1, 2],
            "previous-frame events precede current-frame"
        );
    }
}
