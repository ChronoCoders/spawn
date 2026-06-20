//! Entity parent/child relationships and hierarchical transform propagation.
//!
//! `Parent` and `Children` are kept two-sided-consistent only at structural-apply
//! time (direct `&mut World` ops or applied `Commands`), mirroring the rest of the
//! crate's deferred-mutation discipline. Dead ids left by a non-recursive despawn
//! are tolerated and pruned lazily by the next hierarchy op or by
//! `propagate_transforms`; generation checks keep a stale id from ever aliasing a
//! live entity.

use spawn_core::Transform3D;
use spawn_serialize::{Serialize, SerializeResult, Stream};

use crate::component::Component;
use crate::entity::Entity;
use crate::serialize::{serialize_entity, EntityMap, MapEntities, SerializeComponent};
use crate::world::World;

/// The single parent of an entity. Created and edited only through the
/// `World`/`Commands` hierarchy ops so it stays consistent with the parent's
/// [`Children`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parent(Entity);

impl Component for Parent {}

impl Parent {
    /// The parent entity.
    pub fn get(&self) -> Entity {
        self.0
    }

    pub(crate) fn new(parent: Entity) -> Self {
        Self(parent)
    }
}

impl Default for Parent {
    fn default() -> Self {
        Self(Entity::PLACEHOLDER)
    }
}

impl Serialize for Parent {
    fn serialize<S: Stream>(&mut self, s: &mut S) -> SerializeResult<()> {
        serialize_entity(s, &mut self.0)
    }
}

impl SerializeComponent for Parent {
    fn type_name() -> &'static str {
        "Parent"
    }
}

impl MapEntities for Parent {
    fn map_entities(&mut self, map: &EntityMap) {
        if let Some(parent) = map.get(self.0) {
            self.0 = parent;
        }
    }
}

/// The ordered children of an entity. Insertion order is preserved and drives
/// deterministic transform propagation. Edited only through the hierarchy ops.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Children(Vec<Entity>);

impl Component for Children {}

impl Children {
    /// The children in order.
    pub fn iter(&self) -> impl Iterator<Item = Entity> + '_ {
        self.0.iter().copied()
    }

    /// The number of children.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are no children.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether `entity` is a child.
    pub fn contains(&self, entity: Entity) -> bool {
        self.0.contains(&entity)
    }

    pub(crate) fn single(child: Entity) -> Self {
        Self(vec![child])
    }

    pub(crate) fn push(&mut self, child: Entity) {
        self.0.push(child);
    }

    pub(crate) fn remove(&mut self, child: Entity) {
        if let Some(pos) = self.0.iter().position(|&e| e == child) {
            self.0.remove(pos);
        }
    }

    pub(crate) fn as_slice(&self) -> &[Entity] {
        &self.0
    }

    pub(crate) fn replace(&mut self, children: Vec<Entity>) {
        self.0 = children;
    }
}

impl Serialize for Children {
    fn serialize<S: Stream>(&mut self, s: &mut S) -> SerializeResult<()> {
        let mut len = self.0.len() as u64;
        s.serialize_bits(&mut len, 32)?;
        if !s.is_writing() {
            self.0 = vec![Entity::PLACEHOLDER; len as usize];
        }
        for child in self.0.iter_mut() {
            serialize_entity(s, child)?;
        }
        Ok(())
    }
}

impl SerializeComponent for Children {
    fn type_name() -> &'static str {
        "Children"
    }
}

impl MapEntities for Children {
    fn map_entities(&mut self, map: &EntityMap) {
        for child in self.0.iter_mut() {
            if let Some(mapped) = map.get(*child) {
                *child = mapped;
            }
        }
    }
}

/// World-space transform derived by [`propagate_transforms`] from an entity's
/// local [`Transform3D`] composed with its parent chain. A root's value equals
/// its local transform.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GlobalTransform3D(Transform3D);

impl Component for GlobalTransform3D {}

impl GlobalTransform3D {
    /// The composed world-space transform.
    pub fn get(&self) -> &Transform3D {
        &self.0
    }
}

/// Recomputes [`GlobalTransform3D`] for every entity with a local [`Transform3D`]:
/// a root's global equals its local; a child's is `parent_global ∘ local`. Entities
/// reached as a child but lacking a local transform are skipped, and dead ids in a
/// [`Children`] list are pruned. Deterministic and independent of thread scheduling.
pub fn propagate_transforms(world: &mut World) {
    let locals: Vec<Entity> = world
        .query::<(Entity, &Transform3D)>()
        .iter()
        .map(|(entity, _)| entity)
        .collect();
    let mut roots = Vec::new();
    for entity in locals {
        let rooted = match world.parent(entity) {
            Some(parent) => !world.contains(parent),
            None => true,
        };
        if rooted {
            roots.push(entity);
        }
    }

    let mut stack: Vec<(Entity, Option<Transform3D>)> = Vec::with_capacity(roots.len());
    for &root in roots.iter().rev() {
        stack.push((root, None));
    }
    while let Some((entity, parent_global)) = stack.pop() {
        let local = match world.get::<Transform3D>(entity) {
            Some(local) => *local,
            None => continue,
        };
        let global = match parent_global {
            Some(parent) => parent.mul(local),
            None => local,
        };
        if let Some(slot) = world.get_mut::<GlobalTransform3D>(entity) {
            slot.0 = global;
        } else {
            let _ = world.insert(entity, GlobalTransform3D(global));
        }

        let raw = match world.get::<Children>(entity) {
            Some(children) => children.as_slice().to_vec(),
            None => continue,
        };
        let live: Vec<Entity> = raw.iter().copied().filter(|&c| world.contains(c)).collect();
        if live.len() != raw.len() {
            if let Some(children) = world.get_mut::<Children>(entity) {
                children.replace(live.clone());
            }
        }
        for &child in live.iter().rev() {
            stack.push((child, Some(global)));
        }
    }
}
