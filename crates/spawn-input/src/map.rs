//! Action mapping: aggregate device bindings into queryable action state.

use std::collections::HashMap;

use spawn_core::Vec2;

use crate::action::ActionId;
use crate::binding::{AxisDirection, Binding};
use crate::error::{InputError, InputResult};
use crate::state::{GamepadId, InputState};

/// Resolved per-frame state of a single action.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActionState {
    /// Analog scalar in `[0, 1]` (or composite vector length): the max over
    /// bindings.
    pub value: f32,
    /// 2D vector from `Composite2D`/stick bindings (sum then clamped to unit
    /// length); zero if no 2D binding contributes.
    pub axis2: Vec2,
    /// `true` if any digital binding is held or any analog binding is above its
    /// activation.
    pub pressed: bool,
    /// `true` only on the frame the aggregate `pressed` rose.
    pub just_pressed: bool,
    /// `true` only on the frame the aggregate `pressed` fell.
    pub just_released: bool,
}

impl ActionState {
    const EMPTY: Self = Self {
        value: 0.0,
        axis2: Vec2::ZERO,
        pressed: false,
        just_pressed: false,
        just_released: false,
    };
}

struct Entry {
    bindings: Vec<Binding>,
    state: ActionState,
    pressed_last_frame: bool,
}

impl Entry {
    fn new() -> Self {
        Self {
            bindings: Vec::new(),
            state: ActionState::EMPTY,
            pressed_last_frame: false,
        }
    }
}

/// Maps actions to device bindings and recomputes their state each frame.
///
/// Aggregation: an action is `pressed` if ANY binding is active; its `value` is
/// the MAX over bindings; its `axis2` is the sum of contributing 2D bindings,
/// clamped to unit length. Edge bits derive from the aggregate `pressed` across
/// `update` calls.
pub struct ActionMap<A: ActionId> {
    entries: HashMap<A, Entry>,
    gamepad_id: Option<GamepadId>,
}

impl<A: ActionId> Default for ActionMap<A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: ActionId> ActionMap<A> {
    /// Empty map with auto (first) gamepad selection.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            gamepad_id: None,
        }
    }

    /// Adds a binding to an action; multiple bindings per action are allowed.
    pub fn bind(&mut self, action: A, binding: Binding) {
        self.entries
            .entry(action)
            .or_insert_with(Entry::new)
            .bindings
            .push(binding);
    }

    /// Selects which gamepad the gamepad bindings read; `None` = auto (the
    /// first connected pad).
    pub fn set_gamepad(&mut self, id: Option<GamepadId>) {
        self.gamepad_id = id;
    }

    /// Recomputes every action's state from device state. O(total bindings).
    /// Call once per frame after `InputState::begin_frame`/`process`.
    pub fn update(&mut self, input: &InputState) {
        let pad = self.gamepad_id.or_else(|| input.gamepads().first());
        for entry in self.entries.values_mut() {
            let state = evaluate(&entry.bindings, input, pad, entry.pressed_last_frame);
            entry.pressed_last_frame = state.pressed;
            entry.state = state;
        }
    }

    /// Resolved state for an action; an unbound action is all-false/zero.
    pub fn state(&self, action: A) -> ActionState {
        self.entries
            .get(&action)
            .map(|e| e.state)
            .unwrap_or(ActionState::EMPTY)
    }

    /// Analog value of an action.
    pub fn value(&self, action: A) -> f32 {
        self.state(action).value
    }

    /// 2D axis of an action.
    pub fn axis2(&self, action: A) -> Vec2 {
        self.state(action).axis2
    }

    /// Whether an action is currently pressed.
    pub fn pressed(&self, action: A) -> bool {
        self.state(action).pressed
    }

    /// Whether an action's press edge occurred this frame.
    pub fn just_pressed(&self, action: A) -> bool {
        self.state(action).just_pressed
    }

    /// Whether an action's release edge occurred this frame.
    pub fn just_released(&self, action: A) -> bool {
        self.state(action).just_released
    }

    /// Current bindings for an action (empty slice if unbound). For a rebinding
    /// UI.
    pub fn bindings(&self, action: A) -> &[Binding] {
        self.entries
            .get(&action)
            .map(|e| e.bindings.as_slice())
            .unwrap_or(&[])
    }

    /// Removes all bindings for an action.
    pub fn clear(&mut self, action: A) {
        if let Some(entry) = self.entries.get_mut(&action) {
            entry.bindings.clear();
        }
    }

    /// Removes one binding matching `binding`; `false` if it was absent.
    pub fn remove(&mut self, action: A, binding: &Binding) -> bool {
        if let Some(entry) = self.entries.get_mut(&action) {
            if let Some(i) = entry.bindings.iter().position(|b| b == binding) {
                entry.bindings.remove(i);
                return true;
            }
        }
        false
    }

    /// Replaces one binding; `Err(BindingNotFound)` if `old` is absent.
    pub fn replace(&mut self, action: A, old: &Binding, new: Binding) -> InputResult<()> {
        if let Some(entry) = self.entries.get_mut(&action) {
            if let Some(i) = entry.bindings.iter().position(|b| b == old) {
                entry.bindings[i] = new;
                return Ok(());
            }
        }
        Err(InputError::BindingNotFound)
    }

    /// Replaces the whole binding set for an action.
    pub fn rebind(&mut self, action: A, bindings: &[Binding]) {
        let entry = self.entries.entry(action).or_insert_with(Entry::new);
        entry.bindings.clear();
        entry.bindings.extend_from_slice(bindings);
    }

    /// Whether the action has at least one binding. Used by the context stack to
    /// decide per-action consumption.
    pub(crate) fn binds(&self, action: A) -> bool {
        self.entries
            .get(&action)
            .map(|e| !e.bindings.is_empty())
            .unwrap_or(false)
    }
}

fn evaluate(
    bindings: &[Binding],
    input: &InputState,
    pad: Option<GamepadId>,
    pressed_last_frame: bool,
) -> ActionState {
    let mut value = 0.0_f32;
    let mut axis2 = Vec2::ZERO;
    let mut pressed = false;

    for binding in bindings {
        let (v, vec, p) = eval_binding(binding, input, pad);
        value = value.max(v);
        axis2 += vec;
        pressed = pressed || p;
    }

    let len = axis2.length();
    if len > 1.0 {
        axis2 /= len;
    }

    ActionState {
        value,
        axis2,
        pressed,
        just_pressed: pressed && !pressed_last_frame,
        just_released: !pressed && pressed_last_frame,
    }
}

fn eval_binding(
    binding: &Binding,
    input: &InputState,
    pad: Option<GamepadId>,
) -> (f32, Vec2, bool) {
    match *binding {
        Binding::Key(key) => {
            let held = input.keyboard().is_pressed(key);
            (if held { 1.0 } else { 0.0 }, Vec2::ZERO, held)
        }
        Binding::MouseButton(button) => {
            let held = input.mouse().is_pressed(button);
            (if held { 1.0 } else { 0.0 }, Vec2::ZERO, held)
        }
        Binding::GamepadButton(button) => {
            let held = pad
                .map(|id| input.gamepads().is_pressed(id, button))
                .unwrap_or(false);
            (if held { 1.0 } else { 0.0 }, Vec2::ZERO, held)
        }
        Binding::GamepadAxis {
            axis,
            direction,
            threshold,
        } => {
            let raw = pad.map(|id| input.gamepads().axis(id, axis)).unwrap_or(0.0);
            let directional = directional_magnitude(raw, direction);
            let pressed = directional > threshold;
            (directional, Vec2::ZERO, pressed)
        }
        Binding::Composite2D {
            up,
            down,
            left,
            right,
        } => {
            let kb = input.keyboard();
            let mut v = Vec2::ZERO;
            if kb.is_pressed(up) {
                v.y += 1.0;
            }
            if kb.is_pressed(down) {
                v.y -= 1.0;
            }
            if kb.is_pressed(right) {
                v.x += 1.0;
            }
            if kb.is_pressed(left) {
                v.x -= 1.0;
            }
            let len = v.length();
            (len, v, len > 0.0)
        }
    }
}

fn directional_magnitude(raw: f32, direction: AxisDirection) -> f32 {
    match direction {
        AxisDirection::Positive => raw.max(0.0),
        AxisDirection::Negative => (-raw).max(0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_core::traits::ApproxEq;
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

    fn press(input: &mut InputState, key: KeyCode) {
        input.process(&key_event(key, true));
    }

    #[test]
    fn two_bindings_either_presses_and_value_is_max() {
        let mut input = InputState::new().expect("init");
        let mut map = ActionMap::new();
        let jump = Action(0);
        map.bind(jump, Binding::Key(KeyCode::Space));
        map.bind(jump, Binding::Key(KeyCode::Enter));

        input.begin_frame();
        press(&mut input, KeyCode::Space);
        map.update(&input);
        assert!(map.pressed(jump));
        assert!(map.value(jump).approx_eq_default(1.0));
        assert!(map.just_pressed(jump));

        input.begin_frame();
        input.process(&key_event(KeyCode::Space, false));
        map.update(&input);
        assert!(!map.pressed(jump));
        assert!(map.just_released(jump));
    }

    #[test]
    fn removing_a_binding_updates_next_frame() {
        let mut input = InputState::new().expect("init");
        let mut map = ActionMap::new();
        let act = Action(1);
        map.bind(act, Binding::Key(KeyCode::Space));
        map.bind(act, Binding::Key(KeyCode::Enter));
        assert!(map.remove(act, &Binding::Key(KeyCode::Space)));

        input.begin_frame();
        press(&mut input, KeyCode::Space);
        map.update(&input);
        assert!(!map.pressed(act));
    }

    #[test]
    fn composite2d_directions_and_diagonal_clamp() {
        let mut input = InputState::new().expect("init");
        let mut map = ActionMap::new();
        let mv = Action(2);
        map.bind(
            mv,
            Binding::Composite2D {
                up: KeyCode::W,
                down: KeyCode::S,
                left: KeyCode::A,
                right: KeyCode::D,
            },
        );

        input.begin_frame();
        press(&mut input, KeyCode::W);
        map.update(&input);
        assert!(map.axis2(mv).approx_eq_default(Vec2::new(0.0, 1.0)));

        input.begin_frame();
        press(&mut input, KeyCode::W);
        press(&mut input, KeyCode::S);
        map.update(&input);
        assert!(map.axis2(mv).approx_eq_default(Vec2::ZERO));

        input.begin_frame();
        press(&mut input, KeyCode::W);
        press(&mut input, KeyCode::D);
        map.update(&input);
        assert!(map.axis2(mv).length() <= 1.0 + spawn_core::EPSILON);
        assert!(map.value(mv).approx_eq_default(map.axis2(mv).length()));
    }

    #[test]
    fn replace_absent_is_error_and_present_ok() {
        let mut map: ActionMap<Action> = ActionMap::new();
        let act = Action(3);
        map.bind(act, Binding::Key(KeyCode::A));
        assert!(matches!(
            map.replace(act, &Binding::Key(KeyCode::B), Binding::Key(KeyCode::C)),
            Err(InputError::BindingNotFound)
        ));
        assert!(map
            .replace(act, &Binding::Key(KeyCode::A), Binding::Key(KeyCode::B))
            .is_ok());
        assert_eq!(map.bindings(act), &[Binding::Key(KeyCode::B)]);
    }

    #[test]
    fn rebind_replaces_set_and_clear_empties() {
        let mut map: ActionMap<Action> = ActionMap::new();
        let act = Action(4);
        map.bind(act, Binding::Key(KeyCode::A));
        map.rebind(act, &[Binding::Key(KeyCode::X), Binding::Key(KeyCode::Y)]);
        assert_eq!(map.bindings(act).len(), 2);
        map.clear(act);
        assert!(map.bindings(act).is_empty());
    }

    #[test]
    fn unbound_action_is_empty() {
        let map: ActionMap<Action> = ActionMap::new();
        let st = map.state(Action(99));
        assert_eq!(st, ActionState::EMPTY);
    }
}
