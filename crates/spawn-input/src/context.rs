//! Layered input contexts with top-down per-action consumption.
//!
//! The top context is evaluated first; for any action it binds, it consumes that
//! action so lower contexts do not see it. Actions a context does not bind fall
//! through to the next context down. Consumption is per-action, not
//! all-or-nothing.

use spawn_core::Vec2;

use crate::action::ActionId;
use crate::map::{ActionMap, ActionState};
use crate::state::InputState;

/// Identity of a context layer on the stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContextId(pub u32);

/// Ordered stack of input contexts, each owning its own [`ActionMap`].
pub struct InputContextStack<A: ActionId> {
    layers: Vec<(ContextId, ActionMap<A>)>,
}

impl<A: ActionId> Default for InputContextStack<A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: ActionId> InputContextStack<A> {
    /// Empty stack.
    pub fn new() -> Self {
        Self { layers: Vec::new() }
    }

    /// Pushes a context onto the top of the stack.
    pub fn push(&mut self, id: ContextId, map: ActionMap<A>) {
        self.layers.push((id, map));
    }

    /// Pops the top context, if any.
    pub fn pop(&mut self) -> Option<(ContextId, ActionMap<A>)> {
        self.layers.pop()
    }

    /// The id of the top context, if any.
    pub fn top(&self) -> Option<ContextId> {
        self.layers.last().map(|(id, _)| *id)
    }

    /// Mutable access to a layered context's map, for runtime rebinding.
    /// `None` if `id` is not on the stack.
    pub fn map_mut(&mut self, id: ContextId) -> Option<&mut ActionMap<A>> {
        self.layers
            .iter_mut()
            .find(|(cid, _)| *cid == id)
            .map(|(_, map)| map)
    }

    /// Updates every context's map from device state. O(total bindings).
    pub fn update(&mut self, input: &InputState) {
        for (_, map) in self.layers.iter_mut() {
            map.update(input);
        }
    }

    /// Resolved state for an action: the topmost context that binds the action
    /// wins; lower contexts are shadowed for that action.
    pub fn state(&self, action: A) -> ActionState {
        for (_, map) in self.layers.iter().rev() {
            if map.binds(action) {
                return map.state(action);
            }
        }
        ActionState {
            value: 0.0,
            axis2: Vec2::ZERO,
            pressed: false,
            just_pressed: false,
            just_released: false,
        }
    }

    /// Consumption-resolved analog value.
    pub fn value(&self, action: A) -> f32 {
        self.state(action).value
    }

    /// Consumption-resolved 2D axis.
    pub fn axis2(&self, action: A) -> Vec2 {
        self.state(action).axis2
    }

    /// Consumption-resolved pressed state.
    pub fn pressed(&self, action: A) -> bool {
        self.state(action).pressed
    }

    /// Consumption-resolved press edge.
    pub fn just_pressed(&self, action: A) -> bool {
        self.state(action).just_pressed
    }

    /// Consumption-resolved release edge.
    pub fn just_released(&self, action: A) -> bool {
        self.state(action).just_released
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_platform::{ButtonState, KeyCode, KeyboardEvent, PlatformEvent};

    use crate::action::Action;
    use crate::binding::Binding;

    fn key_event(key: KeyCode, pressed: bool) -> PlatformEvent {
        PlatformEvent::Keyboard(KeyboardEvent {
            key,
            state: if pressed {
                ButtonState::Pressed
            } else {
                ButtonState::Released
            },
            repeat: false,
        })
    }

    const FIRE: Action = Action(0);
    const PAUSE: Action = Action(1);

    fn gameplay() -> ActionMap<Action> {
        let mut m = ActionMap::new();
        m.bind(FIRE, Binding::Key(KeyCode::Space));
        m.bind(PAUSE, Binding::Key(KeyCode::Escape));
        m
    }

    fn menu() -> ActionMap<Action> {
        let mut m = ActionMap::new();
        m.bind(FIRE, Binding::Key(KeyCode::Space));
        m
    }

    #[test]
    fn top_context_consumes_bound_action() {
        let mut stack = InputContextStack::new();
        stack.push(ContextId(0), gameplay());
        stack.push(ContextId(1), menu());

        let mut input = InputState::new().expect("init");
        input.begin_frame();
        input.process(&key_event(KeyCode::Space, true));
        input.process(&key_event(KeyCode::Escape, true));
        stack.update(&input);

        // FIRE is bound by both; top (menu) wins but is still pressed.
        assert!(stack.pressed(FIRE));
        // PAUSE is bound only in gameplay; menu does not bind it, so it falls
        // through and remains visible.
        assert!(stack.pressed(PAUSE));
        assert_eq!(stack.top(), Some(ContextId(1)));
    }

    #[test]
    fn pop_restores_lower_visibility() {
        let mut stack = InputContextStack::new();
        stack.push(ContextId(0), gameplay());

        let mut blocking = ActionMap::new();
        blocking.bind(FIRE, Binding::Key(KeyCode::Enter));
        stack.push(ContextId(1), blocking);

        let mut input = InputState::new().expect("init");
        input.begin_frame();
        input.process(&key_event(KeyCode::Space, true));
        stack.update(&input);
        // Top binds FIRE to Enter (not held) and shadows gameplay's Space.
        assert!(!stack.pressed(FIRE));

        stack.pop();
        stack.update(&input);
        assert!(stack.pressed(FIRE));
    }

    #[test]
    fn map_mut_allows_rebinding_layer() {
        let mut stack = InputContextStack::new();
        stack.push(ContextId(0), gameplay());
        let m = stack.map_mut(ContextId(0)).expect("present");
        m.rebind(FIRE, &[Binding::Key(KeyCode::F)]);
        assert!(stack.map_mut(ContextId(9)).is_none());

        let mut input = InputState::new().expect("init");
        input.begin_frame();
        input.process(&key_event(KeyCode::F, true));
        stack.update(&input);
        assert!(stack.pressed(FIRE));
    }
}
