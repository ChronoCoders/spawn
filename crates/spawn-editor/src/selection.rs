//! The [`Selection`] model: an ordered set of entities with a primary, plus a
//! change-snapshot type.

use spawn_ecs::{Entity, World};

/// An ordered set of selected entities with a distinguished primary.
///
/// Insertion order is preserved for stable UI listing. The backing store is a
/// `Vec<Entity>` with linear membership checks; selections are editor-scale
/// (small), so a `HashSet` is not warranted.
pub struct Selection {
    entities: Vec<Entity>,
    primary: Option<Entity>,
}

impl Selection {
    /// An empty selection with no primary.
    pub fn new() -> Self {
        Self {
            entities: Vec::new(),
            primary: None,
        }
    }

    /// Adds `entity` to the set and makes it primary. Returns `true` if the
    /// selection changed (membership grew or the primary moved).
    pub fn select(&mut self, entity: Entity) -> bool {
        let was_present = self.entities.contains(&entity);
        if !was_present {
            self.entities.push(entity);
        }
        let primary_changed = self.primary != Some(entity);
        self.primary = Some(entity);
        !was_present || primary_changed
    }

    /// Removes `entity`. If it was primary, the primary becomes the last
    /// remaining entity (or `None`). Returns whether the selection changed.
    pub fn deselect(&mut self, entity: Entity) -> bool {
        let Some(pos) = self.entities.iter().position(|&e| e == entity) else {
            return false;
        };
        self.entities.remove(pos);
        if self.primary == Some(entity) {
            self.primary = self.entities.last().copied();
        }
        true
    }

    /// Selects `entity` if absent, else deselects it. Returns whether changed.
    pub fn toggle(&mut self, entity: Entity) -> bool {
        if self.entities.contains(&entity) {
            self.deselect(entity)
        } else {
            self.select(entity)
        }
    }

    /// Empties the set and clears the primary. Returns whether changed.
    pub fn clear(&mut self) -> bool {
        if self.entities.is_empty() && self.primary.is_none() {
            return false;
        }
        self.entities.clear();
        self.primary = None;
        true
    }

    /// Makes `entity` primary, selecting it first if absent. Returns whether
    /// changed.
    pub fn set_primary(&mut self, entity: Entity) -> bool {
        if !self.entities.contains(&entity) {
            return self.select(entity);
        }
        if self.primary == Some(entity) {
            return false;
        }
        self.primary = Some(entity);
        true
    }

    pub fn primary(&self) -> Option<Entity> {
        self.primary
    }

    pub fn is_selected(&self, entity: Entity) -> bool {
        self.entities.contains(&entity)
    }

    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// Iterates the selection in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = Entity> + '_ {
        self.entities.iter().copied()
    }

    /// Drops entities for which `World::contains` is `false`, reconciling the
    /// selection after undo/redo of despawns. If the dropped set included the
    /// primary, the primary becomes the last surviving entity (or `None`).
    /// Returns whether the selection changed.
    pub fn retain_live(&mut self, world: &World) -> bool {
        let before = self.entities.len();
        self.entities.retain(|&e| world.contains(e));
        let changed = self.entities.len() != before;
        if let Some(primary) = self.primary {
            if !self.entities.contains(&primary) {
                self.primary = self.entities.last().copied();
            }
        }
        changed
    }

    /// A snapshot of the current state for emission to the UI layer.
    ///
    /// Callers treat a `true` return from any mutator as "emit `snapshot()`";
    /// no-op mutators return `false` and emit nothing.
    pub fn snapshot(&self) -> SelectionChanged {
        SelectionChanged {
            selected: self.entities.clone(),
            primary: self.primary,
        }
    }
}

impl Default for Selection {
    fn default() -> Self {
        Self::new()
    }
}

/// An observable snapshot of a [`Selection`] state change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionChanged {
    /// Selected entities in insertion order.
    pub selected: Vec<Entity>,
    pub primary: Option<Entity>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_core::Transform3D;
    use spawn_ecs::World;

    fn world() -> World {
        let mut w = World::new();
        w.register::<Transform3D>();
        w
    }

    #[test]
    fn select_sets_primary_and_reports_change() {
        let mut w = world();
        let a = w.spawn();
        let b = w.spawn();
        let mut s = Selection::new();
        assert!(s.select(a));
        assert_eq!(s.primary(), Some(a));
        assert!(s.select(b));
        assert_eq!(s.primary(), Some(b));
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn reselecting_present_changes_only_when_primary_moves() {
        let mut w = world();
        let a = w.spawn();
        let b = w.spawn();
        let mut s = Selection::new();
        s.select(a);
        s.select(b);
        assert!(s.select(a));
        assert!(!s.select(a));
    }

    #[test]
    fn deselect_primary_falls_back_and_absent_is_noop() {
        let mut w = world();
        let a = w.spawn();
        let b = w.spawn();
        let c = w.spawn();
        let mut s = Selection::new();
        s.select(a);
        s.select(b);
        assert!(s.deselect(b));
        assert_eq!(s.primary(), Some(a));
        assert!(!s.deselect(c));
    }

    #[test]
    fn toggle_round_trips() {
        let mut w = world();
        let a = w.spawn();
        let mut s = Selection::new();
        assert!(s.toggle(a));
        assert!(s.is_selected(a));
        assert!(s.toggle(a));
        assert!(!s.is_selected(a));
    }

    #[test]
    fn clear_reports_change_only_when_nonempty() {
        let mut w = world();
        let a = w.spawn();
        let mut s = Selection::new();
        assert!(!s.clear());
        s.select(a);
        assert!(s.clear());
        assert_eq!(s.primary(), None);
    }

    #[test]
    fn set_primary_selects_if_absent() {
        let mut w = world();
        let a = w.spawn();
        let b = w.spawn();
        let mut s = Selection::new();
        s.select(a);
        assert!(s.set_primary(b));
        assert!(s.is_selected(b));
        assert!(!s.set_primary(b));
    }

    #[test]
    fn snapshot_reflects_order_and_primary() {
        let mut w = world();
        let a = w.spawn();
        let b = w.spawn();
        let mut s = Selection::new();
        s.select(a);
        s.select(b);
        let snap = s.snapshot();
        assert_eq!(snap.selected, vec![a, b]);
        assert_eq!(snap.primary, Some(b));
    }

    #[test]
    fn retain_live_drops_dead_and_fixes_primary() {
        let mut w = world();
        let a = w.spawn();
        let b = w.spawn();
        let mut s = Selection::new();
        s.select(a);
        s.select(b);
        w.despawn(b).unwrap();
        assert!(s.retain_live(&w));
        assert!(!s.is_selected(b));
        assert_eq!(s.primary(), Some(a));
        assert!(!s.retain_live(&w));
    }

    #[test]
    fn iter_is_insertion_order() {
        let mut w = world();
        let a = w.spawn();
        let b = w.spawn();
        let c = w.spawn();
        let mut s = Selection::new();
        s.select(c);
        s.select(a);
        s.select(b);
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![c, a, b]);
    }
}
