//! The undoable [`Command`] abstraction and the Phase 1 built-in commands.

pub mod builtin;

use crate::error::EditorResult;
use spawn_ecs::World;

/// A reversible editor edit.
///
/// `apply` performs the edit and latches whatever revert state it needs (e.g.
/// the prior value of a component it overwrites). `revert` exactly undoes the
/// most recent `apply`, restoring the captured state. The pair must round-trip:
/// `apply` then `revert` leaves the [`World`] observably identical — same live
/// entities, same component values. This is the core correctness invariant.
///
/// `apply` takes `&mut self` because a command latches revert state on first
/// apply; a freshly constructed command holds no captured state. The
/// [`CommandStack`](crate::CommandStack) never calls `revert` before a
/// successful `apply`, so reverting un-applied state cannot occur through the
/// public API. On `Err`, an implementation must leave the world unmodified (or
/// restore it) so a failed `apply` is a no-op for the stack.
pub trait Command: 'static {
    /// Performs the edit, latching revert state on first apply.
    fn apply(&mut self, world: &mut World) -> EditorResult<()>;

    /// Restores the state captured by the most recent successful `apply`.
    fn revert(&mut self, world: &mut World) -> EditorResult<()>;

    /// Human-readable name surfaced in undo/redo UI (e.g. "Move").
    fn label(&self) -> &str;

    /// Type-erased self, used by [`try_merge`](Command::try_merge) to downcast a
    /// candidate `next` to the implementor's concrete type.
    fn as_any(&self) -> &dyn core::any::Any;

    /// Attempts to coalesce a subsequent command into `self` for continuous
    /// gestures (e.g. a drag emitting many moves per frame).
    ///
    /// Returns `true` if `next` was absorbed — then `next` is dropped and never
    /// pushed, so the whole gesture is one undo step. When absorbing, `self`
    /// adopts `next`'s new target value while retaining its own original
    /// captured revert state, so a single undo reverts the entire gesture. The
    /// default is non-mergeable (`false`). Implementations downcast via
    /// `next.as_any().downcast_ref::<Self>()`; a `None` downcast returns `false`.
    fn try_merge(&mut self, next: &dyn Command) -> bool {
        let _ = next;
        false
    }
}
