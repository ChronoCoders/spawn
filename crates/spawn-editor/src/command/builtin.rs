//! Phase 1 built-in commands: [`SpawnEntity`], [`DespawnEntity`],
//! [`SetTransform3D`].
//!
//! spawn-ecs has no reflection in Phase 1, so a command cannot generically
//! snapshot an arbitrary entity. Each command knows the concrete component types
//! it manipulates and captures them by value. The editor-managed component set
//! in Phase 1 is exactly [`Transform3D`].

use crate::command::Command;
use crate::error::{EditorError, EditorResult};
use spawn_core::Transform3D;
use spawn_ecs::{Entity, World};

/// Spawns an entity, optionally with an initial [`Transform3D`].
pub struct SpawnEntity {
    initial: Option<Transform3D>,
    spawned: Option<Entity>,
}

impl SpawnEntity {
    /// Spawns a bare entity (empty archetype).
    pub fn new() -> Self {
        Self {
            initial: None,
            spawned: None,
        }
    }

    /// Spawns an entity and inserts `transform`.
    pub fn with_transform(transform: Transform3D) -> Self {
        Self {
            initial: Some(transform),
            spawned: None,
        }
    }

    /// The spawned entity; `Some` only after a successful `apply`.
    ///
    /// Re-applying after a revert (redo) spawns a *fresh* entity whose
    /// `index`/`generation` may differ from the original, so callers must read
    /// the current id here and never cache a pre-undo id.
    pub fn entity(&self) -> Option<Entity> {
        self.spawned
    }
}

impl Default for SpawnEntity {
    fn default() -> Self {
        Self::new()
    }
}

impl Command for SpawnEntity {
    fn apply(&mut self, world: &mut World) -> EditorResult<()> {
        let entity = match self.initial {
            Some(transform) => world.spawn_with((transform,)),
            None => world.spawn(),
        };
        self.spawned = Some(entity);
        Ok(())
    }

    fn revert(&mut self, world: &mut World) -> EditorResult<()> {
        match self.spawned.take() {
            Some(entity) => world
                .despawn(entity)
                .map_err(|_| EditorError::EntityNotFound { entity }),
            None => Ok(()),
        }
    }

    fn label(&self) -> &str {
        "Spawn Entity"
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

/// Despawns an entity, capturing its editor-managed components for revert.
pub struct DespawnEntity {
    target: Entity,
    transform: Option<Transform3D>,
}

impl DespawnEntity {
    /// Targets `entity` for despawn.
    pub fn new(entity: Entity) -> Self {
        Self {
            target: entity,
            transform: None,
        }
    }

    /// The current target id.
    ///
    /// After a `revert` re-creates the entity it returns a *new* id (despawn
    /// freed the slot), so this is updated to that id and a subsequent redo
    /// despawns the correct re-created entity.
    pub fn entity(&self) -> Entity {
        self.target
    }
}

impl Command for DespawnEntity {
    fn apply(&mut self, world: &mut World) -> EditorResult<()> {
        if !world.contains(self.target) {
            return Err(EditorError::EntityNotFound {
                entity: self.target,
            });
        }
        let snapshot = world.get::<Transform3D>(self.target).copied();
        world
            .despawn(self.target)
            .map_err(|_| EditorError::EntityNotFound {
                entity: self.target,
            })?;
        self.transform = snapshot;
        Ok(())
    }

    fn revert(&mut self, world: &mut World) -> EditorResult<()> {
        let entity = match self.transform {
            Some(transform) => world.spawn_with((transform,)),
            None => world.spawn(),
        };
        self.target = entity;
        Ok(())
    }

    fn label(&self) -> &str {
        "Despawn Entity"
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

/// Overwrites an entity's [`Transform3D`], mergeable across a continuous drag.
pub struct SetTransform3D {
    target: Entity,
    new_value: Transform3D,
    old_value: Option<Transform3D>,
}

impl SetTransform3D {
    /// Sets `entity`'s transform to `new_value` on apply.
    pub fn new(entity: Entity, new_value: Transform3D) -> Self {
        Self {
            target: entity,
            new_value,
            old_value: None,
        }
    }

    /// The targeted entity.
    pub fn entity(&self) -> Entity {
        self.target
    }
}

impl Command for SetTransform3D {
    fn apply(&mut self, world: &mut World) -> EditorResult<()> {
        let current =
            world
                .get_mut::<Transform3D>(self.target)
                .ok_or(EditorError::ComponentMissing {
                    entity: self.target,
                    component: "Transform3D",
                })?;
        if self.old_value.is_none() {
            self.old_value = Some(*current);
        }
        *current = self.new_value;
        Ok(())
    }

    fn revert(&mut self, world: &mut World) -> EditorResult<()> {
        let old = match self.old_value {
            Some(old) => old,
            None => return Ok(()),
        };
        let current =
            world
                .get_mut::<Transform3D>(self.target)
                .ok_or(EditorError::ComponentMissing {
                    entity: self.target,
                    component: "Transform3D",
                })?;
        *current = old;
        Ok(())
    }

    fn label(&self) -> &str {
        "Move"
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    /// Merges iff `next` is a [`SetTransform3D`] on the same entity. On success
    /// `self` adopts `next`'s new value and keeps its original captured old
    /// value, collapsing a drag into one undoable step.
    fn try_merge(&mut self, next: &dyn Command) -> bool {
        match next.as_any().downcast_ref::<SetTransform3D>() {
            Some(other) if other.target == self.target => {
                self.new_value = other.new_value;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_core::{ApproxEq, Vec3};

    fn world() -> World {
        let mut w = World::new();
        w.register::<Transform3D>();
        w
    }

    #[test]
    fn spawn_apply_revert_roundtrip() {
        let mut w = world();
        let before = w.entity_count();
        let mut cmd = SpawnEntity::new();
        cmd.apply(&mut w).unwrap();
        assert_eq!(w.entity_count(), before + 1);
        assert!(cmd.entity().is_some());
        cmd.revert(&mut w).unwrap();
        assert_eq!(w.entity_count(), before);
    }

    #[test]
    fn spawn_with_transform_inserts_component() {
        let mut w = world();
        let t = Transform3D::from_translation(Vec3::new(1.0, 2.0, 3.0));
        let mut cmd = SpawnEntity::with_transform(t);
        cmd.apply(&mut w).unwrap();
        let e = cmd.entity().unwrap();
        assert!(w.get::<Transform3D>(e).unwrap().approx_eq_default(t));
    }

    #[test]
    fn redo_spawns_fresh_entity() {
        let mut w = world();
        let mut cmd = SpawnEntity::new();
        cmd.apply(&mut w).unwrap();
        let first = cmd.entity().unwrap();
        cmd.revert(&mut w).unwrap();
        cmd.apply(&mut w).unwrap();
        let second = cmd.entity().unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn despawn_apply_revert_restores_transform() {
        let mut w = world();
        let t = Transform3D::from_translation(Vec3::new(4.0, 5.0, 6.0));
        let e = w.spawn_with((t,));
        let mut cmd = DespawnEntity::new(e);
        cmd.apply(&mut w).unwrap();
        assert!(!w.contains(e));
        cmd.revert(&mut w).unwrap();
        let restored = cmd.entity();
        assert!(w.contains(restored));
        assert!(w.get::<Transform3D>(restored).unwrap().approx_eq_default(t));
    }

    #[test]
    fn despawn_missing_entity_errors() {
        let mut w = world();
        let e = w.spawn();
        w.despawn(e).unwrap();
        let mut cmd = DespawnEntity::new(e);
        assert!(matches!(
            cmd.apply(&mut w),
            Err(EditorError::EntityNotFound { .. })
        ));
    }

    #[test]
    fn set_transform_roundtrip() {
        let mut w = world();
        let old = Transform3D::from_translation(Vec3::new(1.0, 0.0, 0.0));
        let new = Transform3D::from_translation(Vec3::new(9.0, 0.0, 0.0));
        let e = w.spawn_with((old,));
        let mut cmd = SetTransform3D::new(e, new);
        cmd.apply(&mut w).unwrap();
        assert!(w.get::<Transform3D>(e).unwrap().approx_eq_default(new));
        cmd.revert(&mut w).unwrap();
        assert!(w.get::<Transform3D>(e).unwrap().approx_eq_default(old));
    }

    #[test]
    fn set_transform_missing_component_errors() {
        let mut w = world();
        let e = w.spawn();
        let mut cmd = SetTransform3D::new(e, Transform3D::IDENTITY);
        assert!(matches!(
            cmd.apply(&mut w),
            Err(EditorError::ComponentMissing { .. })
        ));
    }

    #[test]
    fn reapply_does_not_recapture_old_value() {
        let mut w = world();
        let old = Transform3D::from_translation(Vec3::new(1.0, 0.0, 0.0));
        let mid = Transform3D::from_translation(Vec3::new(2.0, 0.0, 0.0));
        let e = w.spawn_with((old,));
        let mut cmd = SetTransform3D::new(e, mid);
        cmd.apply(&mut w).unwrap();
        cmd.revert(&mut w).unwrap();
        cmd.apply(&mut w).unwrap();
        cmd.revert(&mut w).unwrap();
        assert!(w.get::<Transform3D>(e).unwrap().approx_eq_default(old));
    }

    #[test]
    fn merge_same_entity_keeps_old_adopts_new() {
        let a = Transform3D::from_translation(Vec3::new(1.0, 0.0, 0.0));
        let b = Transform3D::from_translation(Vec3::new(2.0, 0.0, 0.0));
        let mut w = world();
        let e = w.spawn_with((Transform3D::IDENTITY,));
        let mut first = SetTransform3D::new(e, a);
        first.apply(&mut w).unwrap();
        let second = SetTransform3D::new(e, b);
        assert!(first.try_merge(&second));
        assert!(first.new_value.approx_eq_default(b));
        assert!(first
            .old_value
            .unwrap()
            .approx_eq_default(Transform3D::IDENTITY));
    }

    #[test]
    fn merge_rejects_other_entity_and_other_type() {
        let mut w = world();
        let e1 = w.spawn_with((Transform3D::IDENTITY,));
        let e2 = w.spawn_with((Transform3D::IDENTITY,));
        let mut first = SetTransform3D::new(e1, Transform3D::IDENTITY);
        let other_entity = SetTransform3D::new(e2, Transform3D::IDENTITY);
        assert!(!first.try_merge(&other_entity));
        let spawn = SpawnEntity::new();
        assert!(!first.try_merge(&spawn));
    }
}
