//! Action identity for the mapping layer.
//!
//! User code may define actions with any `Copy + Eq + Hash` type (e.g. a custom
//! enum) thanks to the blanket [`ActionId`] impl, or use the provided [`Action`]
//! newtype for string-free integer actions.

/// Marker trait for types usable as action identities in an
/// [`ActionMap`](crate::map::ActionMap).
///
/// A blanket impl covers every `Copy + Eq + Hash + 'static` type, so user enums
/// are usable directly without an explicit impl.
pub trait ActionId: Copy + Eq + core::hash::Hash + 'static {}

impl<T: Copy + Eq + core::hash::Hash + 'static> ActionId for T {}

/// Provided integer action identity for string-free action sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Action(pub u32);

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    enum MyAction {
        Jump,
        Move,
    }

    fn takes_action<A: ActionId>(_a: A) {}

    #[test]
    fn newtype_and_user_enum_are_action_ids() {
        takes_action(Action(7));
        takes_action(MyAction::Jump);
        takes_action(MyAction::Move);
    }
}
