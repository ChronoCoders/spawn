//! The [`EditorState`] aggregate: command stack, selection, and the Edit/Play
//! mode machine with its play-snapshot contract.

use crate::error::{EditorError, EditorResult};
use crate::selection::Selection;
use crate::stack::CommandStack;
use spawn_core::Transform3D;
use spawn_ecs::{Entity, World};

/// Whether the editor is authoring (`Edit`) or running the simulation (`Play`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorMode {
    /// Authoring the scene; edits mutate the persistent world.
    Edit,
    /// Running the simulation; mutations are discarded on `exit_play`.
    Play,
}

/// A play-mode snapshot of the editor-managed component set (Phase 1:
/// `Transform3D`) plus the live-entity roster.
struct PlaySnapshot {
    transforms: Vec<(Entity, Transform3D)>,
    live: Vec<Entity>,
}

/// The editor's top-level mutable state.
pub struct EditorState {
    pub commands: CommandStack,
    pub selection: Selection,
    mode: EditorMode,
    snapshot: Option<PlaySnapshot>,
}

impl EditorState {
    /// A fresh state in `Edit` mode with a default-capacity stack and empty
    /// selection.
    pub fn new() -> Self {
        Self {
            commands: CommandStack::with_default_capacity(),
            selection: Selection::new(),
            mode: EditorMode::Edit,
            snapshot: None,
        }
    }

    /// As [`new`](EditorState::new) but with an explicit stack capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            commands: CommandStack::new(capacity),
            selection: Selection::new(),
            mode: EditorMode::Edit,
            snapshot: None,
        }
    }

    pub fn mode(&self) -> EditorMode {
        self.mode
    }

    pub fn is_playing(&self) -> bool {
        self.mode == EditorMode::Play
    }

    /// Transitions Edit → Play, snapshotting the authoring world so all runtime
    /// mutations can be discarded on stop.
    ///
    /// Returns `InvalidMode` if already in `Play`. The Phase 1 snapshot captures
    /// only the editor-managed component set (`Transform3D`) over every live
    /// entity plus the live-entity roster; a generic byte-exact world snapshot
    /// is gated on spawn-ecs gaining world cloning or serialization.
    pub fn enter_play(&mut self, world: &mut World) -> EditorResult<()> {
        if self.mode == EditorMode::Play {
            return Err(EditorError::InvalidMode {
                context: "enter_play while already playing",
            });
        }
        let transforms: Vec<(Entity, Transform3D)> = world
            .query::<(Entity, &Transform3D)>()
            .iter()
            .map(|(e, t)| (e, *t))
            .collect();
        let live: Vec<Entity> = world.query::<Entity>().iter_entities().collect();
        self.snapshot = Some(PlaySnapshot { transforms, live });
        self.mode = EditorMode::Play;
        Ok(())
    }

    /// Transitions Play → Edit, restoring the world from the snapshot.
    ///
    /// Returns `InvalidMode` if not in `Play`. Restore despawns entities created
    /// during play, re-spawns entities despawned during play (best-effort: new
    /// ids, so external references are not stable across play), and rewrites the
    /// `Transform3D` of surviving entities to their pre-play values. After
    /// restore, [`Selection::retain_live`] reconciles the selection against the
    /// restored world (dropping entities whose ids did not survive).
    pub fn exit_play(&mut self, world: &mut World) -> EditorResult<()> {
        if self.mode != EditorMode::Play {
            return Err(EditorError::InvalidMode {
                context: "exit_play while not playing",
            });
        }
        let snapshot = match self.snapshot.take() {
            Some(snapshot) => snapshot,
            None => {
                self.mode = EditorMode::Edit;
                return Ok(());
            }
        };

        let current: Vec<Entity> = world.query::<Entity>().iter_entities().collect();
        for entity in current {
            if !snapshot.live.contains(&entity) {
                world.despawn(entity)?;
            }
        }
        for entity in &snapshot.live {
            if !world.contains(*entity) {
                let transform = snapshot
                    .transforms
                    .iter()
                    .find(|(e, _)| e == entity)
                    .map(|(_, t)| *t);
                match transform {
                    Some(t) => {
                        world.spawn_with((t,));
                    }
                    None => {
                        world.spawn();
                    }
                }
            }
        }
        for (entity, transform) in &snapshot.transforms {
            if let Some(current) = world.get_mut::<Transform3D>(*entity) {
                *current = *transform;
            }
        }

        self.mode = EditorMode::Edit;
        self.selection.retain_live(world);
        Ok(())
    }
}

impl Default for EditorState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_core::{ApproxEq, Vec3};
    use spawn_ecs::World;

    fn world() -> World {
        let mut w = World::new();
        w.register::<Transform3D>();
        w
    }

    fn at(x: f32) -> Transform3D {
        Transform3D::from_translation(Vec3::new(x, 0.0, 0.0))
    }

    #[test]
    fn defaults_to_edit_mode() {
        let s = EditorState::new();
        assert_eq!(s.mode(), EditorMode::Edit);
        assert!(!s.is_playing());
        assert_eq!(s.commands.capacity(), 256);
    }

    #[test]
    fn with_capacity_sets_stack() {
        let s = EditorState::with_capacity(7);
        assert_eq!(s.commands.capacity(), 7);
    }

    #[test]
    fn enter_play_twice_is_invalid() {
        let mut w = world();
        let mut s = EditorState::new();
        s.enter_play(&mut w).unwrap();
        assert!(s.is_playing());
        assert!(matches!(
            s.enter_play(&mut w),
            Err(EditorError::InvalidMode { .. })
        ));
    }

    #[test]
    fn exit_play_without_play_is_invalid() {
        let mut w = world();
        let mut s = EditorState::new();
        assert!(matches!(
            s.exit_play(&mut w),
            Err(EditorError::InvalidMode { .. })
        ));
    }

    #[test]
    fn play_restores_transform_mutation() {
        let mut w = world();
        let e = w.spawn_with((at(1.0),));
        let mut s = EditorState::new();
        s.enter_play(&mut w).unwrap();
        *w.get_mut::<Transform3D>(e).unwrap() = at(99.0);
        s.exit_play(&mut w).unwrap();
        assert!(w.get::<Transform3D>(e).unwrap().approx_eq_default(at(1.0)));
        assert_eq!(s.mode(), EditorMode::Edit);
    }

    #[test]
    fn play_spawned_entities_are_removed() {
        let mut w = world();
        let before = w.entity_count();
        let mut s = EditorState::new();
        s.enter_play(&mut w).unwrap();
        w.spawn_with((at(5.0),));
        w.spawn();
        s.exit_play(&mut w).unwrap();
        assert_eq!(w.entity_count(), before);
    }

    #[test]
    fn play_despawned_entities_are_recreated() {
        let mut w = world();
        let e = w.spawn_with((at(3.0),));
        let mut s = EditorState::new();
        s.enter_play(&mut w).unwrap();
        w.despawn(e).unwrap();
        s.exit_play(&mut w).unwrap();
        assert_eq!(w.entity_count(), 1);
        let query = w.query::<(Entity, &Transform3D)>();
        let restored = query.iter().next().unwrap();
        assert!(restored.1.approx_eq_default(at(3.0)));
    }
}
