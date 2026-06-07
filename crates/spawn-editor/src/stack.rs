//! The [`CommandStack`]: bounded undo/redo history with dirty-flag tracking.

use crate::command::Command;
use crate::error::{EditorError, EditorResult};
use spawn_ecs::World;

/// Bounded undo/redo history of executed commands.
///
/// Undo and redo are O(1) stack operations (one push/pop of a boxed command plus
/// one virtual `revert`/`apply`). When the undo history exceeds `capacity` the
/// oldest command is evicted from the front and can never be undone again;
/// eviction adjusts save-point bookkeeping.
pub struct CommandStack {
    undo: Vec<Box<dyn Command>>,
    redo: Vec<Box<dyn Command>>,
    capacity: usize,
    /// Undo-history length at the last `mark_saved`, or `None` once a save point
    /// has been invalidated by eviction or `clear` (until the next `mark_saved`).
    save_point: Option<usize>,
}

impl CommandStack {
    /// A stack bounded to `capacity` undoable commands. `capacity == 0` is
    /// clamped to `1`, since a zero-depth stack is meaningless.
    pub fn new(capacity: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            capacity: capacity.max(1),
            save_point: Some(0),
        }
    }

    /// A stack with the default depth of `256`.
    pub fn with_default_capacity() -> Self {
        Self::new(256)
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Number of undoable commands currently held.
    pub fn len(&self) -> usize {
        self.undo.len()
    }

    pub fn is_empty(&self) -> bool {
        self.undo.is_empty()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Label of the command `undo` would revert (for UI like "Undo Move").
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|c| c.label())
    }

    /// Label of the command `redo` would re-apply.
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|c| c.label())
    }

    /// Applies `command`, then pushes it onto the undo history.
    ///
    /// On `Ok` the redo history is cleared (a new edit invalidates redo) and the
    /// capacity bound is enforced. On `Err` the command is not pushed and the
    /// stack is unchanged (`apply` implementations leave the world unmodified on
    /// error).
    pub fn execute(
        &mut self,
        mut command: Box<dyn Command>,
        world: &mut World,
    ) -> EditorResult<()> {
        command.apply(world)?;
        self.redo.clear();
        self.undo.push(command);
        self.enforce_capacity();
        Ok(())
    }

    /// Applies `command`, then merges it into the top-of-undo command if
    /// possible, otherwise pushes it.
    ///
    /// After a successful `apply`, if the redo history is empty and the current
    /// top-of-undo command's `try_merge` absorbs `command`, the new command is
    /// dropped (not pushed) so a continuous gesture stays one undo step. Merging
    /// never crosses a redo boundary; if no merge occurs this falls back to
    /// [`execute`](CommandStack::execute) push semantics.
    pub fn execute_merged(
        &mut self,
        mut command: Box<dyn Command>,
        world: &mut World,
    ) -> EditorResult<()> {
        command.apply(world)?;
        if self.redo.is_empty() {
            if let Some(top) = self.undo.last_mut() {
                if top.try_merge(command.as_ref()) {
                    return Ok(());
                }
            }
        }
        self.redo.clear();
        self.undo.push(command);
        self.enforce_capacity();
        Ok(())
    }

    /// Reverts the top undo command, moving it to the redo history on success.
    ///
    /// On `Err` the command stays on the undo history and the error propagates
    /// (the world is assumed restored or left untouched by `revert`).
    pub fn undo(&mut self, world: &mut World) -> EditorResult<()> {
        let mut command = self.undo.pop().ok_or(EditorError::NothingToUndo)?;
        match command.revert(world) {
            Ok(()) => {
                self.redo.push(command);
                Ok(())
            }
            Err(err) => {
                self.undo.push(command);
                Err(err)
            }
        }
    }

    /// Re-applies the top redo command, moving it back to the undo history on
    /// success. On `Err` it stays on the redo history.
    pub fn redo(&mut self, world: &mut World) -> EditorResult<()> {
        let mut command = self.redo.pop().ok_or(EditorError::NothingToRedo)?;
        match command.apply(world) {
            Ok(()) => {
                self.undo.push(command);
                Ok(())
            }
            Err(err) => {
                self.redo.push(command);
                Err(err)
            }
        }
    }

    /// Drops all undo and redo history. If the document was clean at the moment
    /// of the call (currently at the save point) it stays clean at the new empty
    /// position `0`; otherwise the save point is invalidated, since the cleared
    /// stack can no longer return to the saved content, and
    /// [`is_dirty`](CommandStack::is_dirty) reports `true` thereafter.
    pub fn clear(&mut self) {
        let was_clean = !self.is_dirty();
        self.undo.clear();
        self.redo.clear();
        self.save_point = if was_clean { Some(0) } else { None };
    }

    /// Sets the save point to the current undo position; the document is clean
    /// immediately afterwards.
    pub fn mark_saved(&mut self) {
        self.save_point = Some(self.undo.len());
    }

    /// `true` iff the current undo position differs from the save point, or the
    /// save point was invalidated by eviction or `clear`.
    pub fn is_dirty(&self) -> bool {
        match self.save_point {
            Some(point) => point != self.undo.len(),
            None => true,
        }
    }

    /// Evicts oldest commands until the bound holds, decrementing the save point
    /// and invalidating it if the saved position is evicted past.
    fn enforce_capacity(&mut self) {
        while self.undo.len() > self.capacity {
            self.undo.remove(0);
            self.save_point = match self.save_point {
                Some(0) => None,
                Some(point) => Some(point - 1),
                None => None,
            };
        }
    }
}

impl Default for CommandStack {
    fn default() -> Self {
        Self::with_default_capacity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::builtin::{SetTransform3D, SpawnEntity};
    use spawn_core::{ApproxEq, Transform3D, Vec3};
    use spawn_ecs::{Entity, World};

    fn world() -> World {
        let mut w = World::new();
        w.register::<Transform3D>();
        w
    }

    fn at(x: f32) -> Transform3D {
        Transform3D::from_translation(Vec3::new(x, 0.0, 0.0))
    }

    fn fixture(w: &mut World) -> Entity {
        w.spawn_with((Transform3D::IDENTITY,))
    }

    #[test]
    fn capacity_clamped_to_one() {
        assert_eq!(CommandStack::new(0).capacity(), 1);
        assert_eq!(CommandStack::with_default_capacity().capacity(), 256);
    }

    #[test]
    fn execute_pushes_and_clears_redo() {
        let mut w = world();
        let mut s = CommandStack::new(8);
        s.execute(Box::new(SpawnEntity::new()), &mut w).unwrap();
        s.undo(&mut w).unwrap();
        assert!(s.can_redo());
        s.execute(Box::new(SpawnEntity::new()), &mut w).unwrap();
        assert!(!s.can_redo());
    }

    #[test]
    fn undo_redo_labels_and_flags() {
        let mut w = world();
        let mut s = CommandStack::new(8);
        assert!(!s.can_undo());
        assert_eq!(s.undo_label(), None);
        s.execute(Box::new(SpawnEntity::new()), &mut w).unwrap();
        assert_eq!(s.undo_label(), Some("Spawn Entity"));
        s.undo(&mut w).unwrap();
        assert_eq!(s.redo_label(), Some("Spawn Entity"));
        s.redo(&mut w).unwrap();
        assert!(s.can_undo());
    }

    #[test]
    fn nothing_to_undo_or_redo() {
        let mut w = world();
        let mut s = CommandStack::new(4);
        assert!(matches!(s.undo(&mut w), Err(EditorError::NothingToUndo)));
        assert!(matches!(s.redo(&mut w), Err(EditorError::NothingToRedo)));
    }

    #[test]
    fn merge_coalesces_into_single_undo() {
        let mut w = world();
        let e = fixture(&mut w);
        let mut s = CommandStack::new(16);
        for x in 1..=5 {
            s.execute_merged(Box::new(SetTransform3D::new(e, at(x as f32))), &mut w)
                .unwrap();
        }
        assert_eq!(s.len(), 1);
        assert!(w.get::<Transform3D>(e).unwrap().approx_eq_default(at(5.0)));
        s.undo(&mut w).unwrap();
        assert!(w
            .get::<Transform3D>(e)
            .unwrap()
            .approx_eq_default(Transform3D::IDENTITY));
    }

    #[test]
    fn merge_does_not_cross_entities_or_commands() {
        let mut w = world();
        let e1 = fixture(&mut w);
        let e2 = fixture(&mut w);
        let mut s = CommandStack::new(16);
        s.execute_merged(Box::new(SetTransform3D::new(e1, at(1.0))), &mut w)
            .unwrap();
        s.execute_merged(Box::new(SetTransform3D::new(e2, at(1.0))), &mut w)
            .unwrap();
        assert_eq!(s.len(), 2);
        s.execute_merged(Box::new(SpawnEntity::new()), &mut w)
            .unwrap();
        s.execute_merged(Box::new(SetTransform3D::new(e1, at(2.0))), &mut w)
            .unwrap();
        assert_eq!(s.len(), 4);
    }

    #[test]
    fn history_bound_evicts_oldest() {
        let mut w = world();
        let mut s = CommandStack::new(3);
        for _ in 0..5 {
            s.execute(Box::new(SpawnEntity::new()), &mut w).unwrap();
        }
        assert_eq!(s.len(), 3);
        s.undo(&mut w).unwrap();
        s.undo(&mut w).unwrap();
        s.undo(&mut w).unwrap();
        assert!(matches!(s.undo(&mut w), Err(EditorError::NothingToUndo)));
    }

    #[test]
    fn dirty_flag_across_save_points() {
        let mut w = world();
        let e = fixture(&mut w);
        let mut s = CommandStack::new(8);
        assert!(!s.is_dirty());
        s.execute(Box::new(SetTransform3D::new(e, at(1.0))), &mut w)
            .unwrap();
        assert!(s.is_dirty());
        s.mark_saved();
        assert!(!s.is_dirty());
        s.execute(Box::new(SetTransform3D::new(e, at(2.0))), &mut w)
            .unwrap();
        assert!(s.is_dirty());
        s.undo(&mut w).unwrap();
        assert!(!s.is_dirty());
        s.redo(&mut w).unwrap();
        assert!(s.is_dirty());
    }

    #[test]
    fn eviction_past_save_point_invalidates_it() {
        let mut w = world();
        let mut s = CommandStack::new(2);
        s.execute(Box::new(SpawnEntity::new()), &mut w).unwrap();
        s.mark_saved();
        assert!(!s.is_dirty());
        s.execute(Box::new(SpawnEntity::new()), &mut w).unwrap();
        s.execute(Box::new(SpawnEntity::new()), &mut w).unwrap();
        assert!(s.is_dirty());
        s.mark_saved();
        assert!(!s.is_dirty());
    }

    #[test]
    fn clear_dirties_unless_at_origin() {
        let mut w = world();
        let mut s = CommandStack::new(8);
        s.clear();
        assert!(!s.is_dirty());
        s.execute(Box::new(SpawnEntity::new()), &mut w).unwrap();
        s.clear();
        assert!(s.is_dirty());
        assert!(s.is_empty());
        assert!(!s.can_redo());
    }

    #[test]
    fn error_apply_leaves_stack_unchanged() {
        let mut w = world();
        let e = w.spawn();
        let mut s = CommandStack::new(8);
        let before = s.len();
        let res = s.execute(Box::new(SetTransform3D::new(e, at(1.0))), &mut w);
        assert!(matches!(res, Err(EditorError::ComponentMissing { .. })));
        assert_eq!(s.len(), before);
        assert!(!s.can_undo());
    }
}
