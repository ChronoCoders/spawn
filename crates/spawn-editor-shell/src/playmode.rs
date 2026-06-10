//! Edit/Play toggle over the Phase 1 snapshot contract (conservative). No PIE
//! world isolation is invented; `enter_play`/`exit_play` are the spawn-editor
//! snapshot/restore, unchanged.

use spawn_ecs::World;
use spawn_editor::{EditorResult, EditorState};

/// Toggles between Edit and Play. Entering snapshots the managed component set;
/// exiting restores it (and `exit_play` reconciles the selection internally).
pub fn toggle(editor: &mut EditorState, world: &mut World) -> EditorResult<()> {
    if editor.is_playing() {
        editor.exit_play(world)
    } else {
        editor.enter_play(world)
    }
}

/// Whether editor edits (gizmo/inspector/outliner mutations) are permitted. The
/// shell suppresses edits while playing (Phase 1 §6.1 caller-responsibility).
pub fn edits_allowed(editor: &EditorState) -> bool {
    !editor.is_playing()
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_core::{Transform3D, Vec3};
    use spawn_editor::EditorMode;

    #[test]
    fn toggle_enters_then_restores_snapshot() {
        let mut world = World::new();
        world.register::<Transform3D>();
        let e = world.spawn_with((Transform3D::from_translation(Vec3::new(1.0, 0.0, 0.0)),));
        let mut editor = EditorState::new();

        assert!(edits_allowed(&editor));
        toggle(&mut editor, &mut world).unwrap();
        assert_eq!(editor.mode(), EditorMode::Play);
        assert!(!edits_allowed(&editor));

        // Mutate during play, then stop → restored.
        *world.get_mut::<Transform3D>(e).unwrap() =
            Transform3D::from_translation(Vec3::new(9.0, 0.0, 0.0));
        toggle(&mut editor, &mut world).unwrap();
        assert_eq!(editor.mode(), EditorMode::Edit);
        assert_eq!(world.get::<Transform3D>(e).unwrap().translation.x, 1.0);
    }
}
