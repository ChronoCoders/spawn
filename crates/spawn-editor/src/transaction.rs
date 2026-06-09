//! Scoped, nestable transactions over the [`CommandStack`](crate::CommandStack).
//!
//! Mirrors Slate's `UTransBuffer` (UE editor): edits between an explicit
//! begin/end are grouped into one undoable composite, and transactions nest via
//! an action counter — the outermost `end` is the only one that commits. Nesting
//! flattens: every command captured at any depth lands in the single outermost
//! transaction, so one undo reverts the whole gesture. The Phase 1 merge-on-drag
//! is retained as the special case it is, applied within the captured list.
//!
//! Captured commands are applied at execute time (so a drag shows live
//! feedback); committing only packages them, and abort reverts them.

use crate::command::Command;
use crate::error::EditorResult;
use spawn_ecs::World;

/// The single undo entry a committed transaction pushes: its captured commands,
/// applied in order and reverted in reverse. Inherits the Phase 1 per-command id
/// caveat (a command that respawns an entity yields a new id on redo); the
/// transaction types the editor uses (transform/field edits on existing
/// entities) round-trip exactly.
pub(crate) struct CompositeCommand {
    label: String,
    commands: Vec<Box<dyn Command>>,
}

impl CompositeCommand {
    fn new(label: String, commands: Vec<Box<dyn Command>>) -> Self {
        Self { label, commands }
    }
}

impl Command for CompositeCommand {
    fn apply(&mut self, world: &mut World) -> EditorResult<()> {
        for command in self.commands.iter_mut() {
            command.apply(world)?;
        }
        Ok(())
    }

    fn revert(&mut self, world: &mut World) -> EditorResult<()> {
        for command in self.commands.iter_mut().rev() {
            command.revert(world)?;
        }
        Ok(())
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

/// The in-flight transaction: the captured (already-applied) commands, the
/// nesting depth (the action counter), and the label of the outermost `begin`
/// (nested labels are ignored — the first wins).
pub(crate) struct OpenTransaction {
    label: String,
    depth: u32,
    commands: Vec<Box<dyn Command>>,
}

impl OpenTransaction {
    pub(crate) fn new(label: String) -> Self {
        Self {
            label,
            depth: 1,
            commands: Vec::new(),
        }
    }

    pub(crate) fn depth(&self) -> u32 {
        self.depth
    }

    /// Raises the action counter for a nested `begin`.
    pub(crate) fn enter(&mut self) {
        self.depth += 1;
    }

    /// Lowers the action counter for an `end`; returns `true` when the outermost
    /// scope closed and the transaction is ready to commit.
    pub(crate) fn exit(&mut self) -> bool {
        if self.depth > 1 {
            self.depth -= 1;
            false
        } else {
            true
        }
    }

    /// Captures an already-applied command. When `allow_merge`, a continuous
    /// gesture coalesces into the last captured command (drag-merge within the
    /// transaction) instead of accumulating one command per frame.
    pub(crate) fn capture(&mut self, command: Box<dyn Command>, allow_merge: bool) {
        if allow_merge {
            if let Some(last) = self.commands.last_mut() {
                if last.try_merge(command.as_ref()) {
                    return;
                }
            }
        }
        self.commands.push(command);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Consumes the transaction into its committable composite.
    pub(crate) fn into_composite(self) -> CompositeCommand {
        CompositeCommand::new(self.label, self.commands)
    }

    /// Reverts every captured command in reverse for an abort, restoring the
    /// world to its pre-transaction state. Continues restoring the rest even if a
    /// `revert` fails, returning the first error encountered.
    pub(crate) fn rollback(self, world: &mut World) -> EditorResult<()> {
        let mut first_err = None;
        for mut command in self.commands.into_iter().rev() {
            if let Err(err) = command.revert(world) {
                if first_err.is_none() {
                    first_err = Some(err);
                }
            }
        }
        match first_err {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}
