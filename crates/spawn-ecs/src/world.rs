//! The [`World`]: entity allocator, component registry, archetype storage, and
//! a world-level command buffer.
//!
//! `World` is `Send` so it can cross thread boundaries between frames, but
//! parallel access inside a stage is mediated by disjoint borrows, never by
//! sharing `&mut World`.

use crate::archetype::ArchetypeStore;
use crate::bundle::Bundle;
use crate::change::Tick;
use crate::commands::{Command, CommandBuffer, Commands};
use crate::component::{
    column_slice, column_slice_mut, AnyValue, Component, ComponentId, ComponentRegistry,
};
use crate::entity::{Entity, EntityAllocator};
use crate::error::{EcsError, EcsResult};
use crate::events::Event;
use crate::hierarchy::{Children, Parent};
use crate::query::{Query, QueryData};
use crate::reflect::{FieldValue, Reflect, ReflectRegistry, ReflectResult, ReflectedComponent};
use crate::resource::{Res, ResMut, Resource, ResourceId, Resources};
use crate::serialize::{EntityMap, MapEntities, SerializeComponent, SerializeRegistry};
use spawn_serialize::SerializeResult;

/// Container of all entities, components, and archetype storage.
pub struct World {
    allocator: EntityAllocator,
    registry: ComponentRegistry,
    reflect: ReflectRegistry,
    archetypes: ArchetypeStore,
    command_buffer: CommandBuffer,
    resources: Resources,
    serialize: SerializeRegistry,
    change_tick: Tick,
    event_updaters: Vec<fn(&mut World)>,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    /// An empty world with no registered components.
    pub fn new() -> Self {
        Self {
            allocator: EntityAllocator::new(),
            registry: ComponentRegistry::new(),
            reflect: ReflectRegistry::new(),
            archetypes: ArchetypeStore::new(),
            command_buffer: CommandBuffer::new(),
            resources: Resources::new(),
            serialize: SerializeRegistry::new(),
            change_tick: Tick::ZERO.next(),
            event_updaters: Vec::new(),
        }
    }

    pub(crate) fn registry(&self) -> &ComponentRegistry {
        &self.registry
    }

    pub(crate) fn allocator(&self) -> &EntityAllocator {
        &self.allocator
    }

    /// The world's current frame change tick.
    pub fn change_tick(&self) -> Tick {
        self.change_tick
    }

    /// Advances the change tick by one and returns the new value. Called once per
    /// frame by [`Schedule::run`](crate::schedule::Schedule::run); call it
    /// directly when driving more than one schedule per frame.
    pub fn increment_change_tick(&mut self) -> Tick {
        self.change_tick = self.change_tick.next();
        self.change_tick
    }

    /// Registers `T`, returning its dense id. Idempotent: re-registering returns
    /// the existing id.
    pub fn register<T: Component>(&mut self) -> ComponentId {
        self.registry.register::<T>()
    }

    /// Returns `T`'s id, or `None` if `T` was never registered.
    pub fn component_id<T: Component>(&self) -> Option<ComponentId> {
        self.registry.component_id::<T>()
    }

    /// Registers `T` as a component (idempotent, like [`register`](World::register))
    /// and records its reflection vtable, so the inspector can enumerate and
    /// read/write its leaf fields by name. Safe to call repeatedly.
    pub fn register_reflect<T: Reflect>(&mut self) -> ComponentId {
        let id = self.registry.register::<T>();
        self.reflect.insert::<T>(id);
        id
    }

    /// Whether `component` has a reflection vtable.
    pub fn is_reflected(&self, component: ComponentId) -> bool {
        self.reflect.is_reflected(component)
    }

    /// The reflected components present on `entity`, in [`ComponentId`] order.
    /// Empty if the entity is dead or has none. Cold path (selection-change), so
    /// this allocates a `Vec`; the per-field accessors below do not.
    pub fn reflected_components(&self, entity: Entity) -> Vec<ReflectedComponent> {
        if !self.contains(entity) {
            return Vec::new();
        }
        self.reflect.present_on(self, entity)
    }

    /// Reads a reflected leaf. `None` if the entity is dead, lacks the component,
    /// the component is not reflected, or `field` is unknown. Allocation-free.
    pub fn reflect_get_field(
        &self,
        entity: Entity,
        component: ComponentId,
        field: &str,
    ) -> Option<FieldValue> {
        self.reflect.get_field(self, entity, component, field)
    }

    /// Writes a reflected leaf through the normal mutation path.
    /// `ComponentNotReflected` if `component` was never `register_reflect`-ed;
    /// `MissingComponent` if the entity is dead or lacks it; `UnknownField` /
    /// `TypeMismatch` per [`Reflect::reflect_set`]. Allocation-free.
    pub fn reflect_set_field(
        &mut self,
        entity: Entity,
        component: ComponentId,
        field: &str,
        value: FieldValue,
    ) -> ReflectResult<()> {
        let set = self.reflect.set_fn(component)?;
        set(self, entity, field, value)
    }

    /// Registers `T` as a serializable component (idempotent), assigning its
    /// on-wire id in registration order. Both a saved world and the world loading
    /// it must register the same serializable components in the same order.
    pub fn register_serializable<T: SerializeComponent>(&mut self) -> ComponentId {
        let id = self.registry.register::<T>();
        self.serialize.insert::<T>(id);
        id
    }

    /// As [`register_serializable`](World::register_serializable), but records a
    /// remap hook so `T`'s [`Entity`] references are translated to their loaded
    /// counterparts after [`deserialize_world`](World::deserialize_world).
    pub fn register_serializable_mapped<T: SerializeComponent + MapEntities>(
        &mut self,
    ) -> ComponentId {
        let id = self.registry.register::<T>();
        self.serialize.insert_mapped::<T>(id, |world, entity, map| {
            if let Some(component) = world.get_mut::<T>(entity) {
                component.map_entities(map);
            }
        });
        id
    }

    /// Whether `component` has a registered serialize codec.
    pub fn is_serializable(&self, component: ComponentId) -> bool {
        self.serialize.is_serializable(component)
    }

    /// Serializes every live entity and its registered serializable components to
    /// a byte buffer. Non-serializable components are skipped. Deterministic:
    /// identical world + registration order yields identical bytes.
    pub fn serialize_world(&self) -> SerializeResult<Vec<u8>> {
        self.serialize.encode(self)
    }

    /// Clears the world's entities (keeping registrations) and repopulates it from
    /// `bytes`, returning the original-to-loaded [`EntityMap`]. The world must have
    /// the same serializable types registered in the same order as the saved one.
    pub fn deserialize_world(&mut self, bytes: &[u8]) -> EcsResult<EntityMap> {
        let decoders = self.serialize.decoders();
        crate::serialize::decode(self, &decoders, bytes)
    }

    pub(crate) fn clear_entities(&mut self) {
        let live: Vec<Entity> = self.query::<Entity>().iter().collect();
        for entity in live {
            let _ = self.despawn(entity);
        }
    }

    /// Sets `child`'s parent to `parent`, keeping both `Parent` and `Children`
    /// consistent. `EntityNotFound` if either is not live; `HierarchyCycle` if
    /// `parent` is `child` or one of its descendants.
    pub fn set_parent(&mut self, child: Entity, parent: Entity) -> EcsResult<()> {
        if !self.contains(child) {
            return Err(EcsError::EntityNotFound { entity: child });
        }
        if !self.contains(parent) {
            return Err(EcsError::EntityNotFound { entity: parent });
        }
        if self.would_cycle(parent, child) {
            return Err(EcsError::HierarchyCycle { entity: child });
        }
        if let Some(old) = self.parent(child) {
            if old == parent {
                return Ok(());
            }
            if let Some(children) = self.get_mut::<Children>(old) {
                children.remove(child);
            }
        }
        let _ = self.insert(child, Parent::new(parent));
        if let Some(children) = self.get_mut::<Children>(parent) {
            if !children.contains(child) {
                children.push(child);
            }
        } else {
            let _ = self.insert(parent, Children::single(child));
        }
        Ok(())
    }

    /// Detaches `child` from its current parent (both sides). `Ok` no-op if it has
    /// no parent.
    pub fn remove_parent(&mut self, child: Entity) -> EcsResult<()> {
        if let Some(old) = self.parent(child) {
            if let Some(children) = self.get_mut::<Children>(old) {
                children.remove(child);
            }
            let _ = self.remove::<Parent>(child);
        }
        Ok(())
    }

    /// Convenience for [`set_parent(child, parent)`](World::set_parent).
    pub fn add_child(&mut self, parent: Entity, child: Entity) -> EcsResult<()> {
        self.set_parent(child, parent)
    }

    /// The entity's children in insertion order, or an empty slice. May contain
    /// dead ids until the next hierarchy op or propagation prunes them.
    pub fn children(&self, entity: Entity) -> &[Entity] {
        match self.get::<Children>(entity) {
            Some(children) => children.as_slice(),
            None => &[],
        }
    }

    /// The entity's parent, if any.
    pub fn parent(&self, entity: Entity) -> Option<Entity> {
        self.get::<Parent>(entity).map(|p| p.get())
    }

    /// Despawns `entity` and its whole subtree (post-order), detaching it from its
    /// own parent first. `EntityNotFound` if `entity` is not live.
    pub fn despawn_recursive(&mut self, entity: Entity) -> EcsResult<()> {
        if !self.contains(entity) {
            return Err(EcsError::EntityNotFound { entity });
        }
        if let Some(parent) = self.parent(entity) {
            if let Some(children) = self.get_mut::<Children>(parent) {
                children.remove(entity);
            }
        }
        for victim in self.subtree_postorder(entity) {
            let _ = self.despawn(victim);
        }
        Ok(())
    }

    fn would_cycle(&self, parent: Entity, child: Entity) -> bool {
        let cap = self.entity_count() + 1;
        let mut cursor = Some(parent);
        let mut steps = 0;
        while let Some(current) = cursor {
            if current == child {
                return true;
            }
            steps += 1;
            if steps > cap {
                return true;
            }
            cursor = self.parent(current);
        }
        false
    }

    fn subtree_postorder(&self, root: Entity) -> Vec<Entity> {
        let mut out = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut stack = vec![root];
        while let Some(entity) = stack.pop() {
            if !visited.insert(entity) {
                continue;
            }
            out.push(entity);
            if let Some(children) = self.get::<Children>(entity) {
                for child in children.iter() {
                    if self.contains(child) && !visited.contains(&child) {
                        stack.push(child);
                    }
                }
            }
        }
        out.reverse();
        out
    }

    pub fn spawn(&mut self) -> Entity {
        let entity = self.allocator.allocate();
        self.archetypes.place_empty(entity);
        entity
    }

    /// Spawns an entity and inserts `bundle` in one archetype placement.
    pub fn spawn_with<B: Bundle>(&mut self, bundle: B) -> Entity {
        let entity = self.allocator.allocate();
        self.archetypes.place_empty(entity);
        let pairs = bundle.write_into(&mut self.registry);
        self.apply_inserts(entity, pairs);
        entity
    }

    /// Despawns `entity`, freeing its slot and bumping its generation. Returns
    /// `EntityNotFound` if it is stale or already dead.
    pub fn despawn(&mut self, entity: Entity) -> EcsResult<()> {
        if !self.allocator.is_live(entity) {
            return Err(EcsError::EntityNotFound { entity });
        }
        self.archetypes.remove_entity(entity);
        self.allocator.free(entity);
        Ok(())
    }

    /// Inserts (or overwrites) `value` on `entity`, moving it to the target
    /// archetype. Returns `EntityNotFound` if the entity is not live.
    pub fn insert<T: Component>(&mut self, entity: Entity, value: T) -> EcsResult<()> {
        if !self.allocator.is_live(entity) {
            return Err(EcsError::EntityNotFound { entity });
        }
        let id = self.registry.register::<T>();
        self.apply_inserts(entity, vec![(id, Box::new(value) as AnyValue)]);
        Ok(())
    }

    /// Removes `T` from `entity`. `Ok(None)` if the entity is live but lacks
    /// `T`; `Err(EntityNotFound)` if it is not live.
    pub fn remove<T: Component>(&mut self, entity: Entity) -> EcsResult<Option<T>> {
        if !self.allocator.is_live(entity) {
            return Err(EcsError::EntityNotFound { entity });
        }
        let id = match self.registry.component_id::<T>() {
            Some(id) => id,
            None => return Ok(None),
        };
        let removed = self.apply_remove(entity, id);
        Ok(removed.and_then(|boxed| boxed.downcast::<T>().ok().map(|b| *b)))
    }

    pub fn contains(&self, entity: Entity) -> bool {
        self.allocator.is_live(entity)
    }

    pub fn has<T: Component>(&self, entity: Entity) -> bool {
        let id = match self.registry.component_id::<T>() {
            Some(id) => id,
            None => return false,
        };
        match self.archetypes.location(entity) {
            Some((aid, _)) => self.archetypes.archetype(aid).contains(id),
            None => false,
        }
    }

    /// Borrows `entity`'s `T`. `None` if the entity is not live or lacks `T`.
    pub fn get<T: Component>(&self, entity: Entity) -> Option<&T> {
        if !self.allocator.is_live(entity) {
            return None;
        }
        let id = self.registry.component_id::<T>()?;
        let (aid, row) = self.archetypes.location(entity)?;
        let column = self.archetypes.archetype(aid).column(id)?;
        column_slice::<T>(column)?.get(row)
    }

    /// Mutably borrows `entity`'s `T`. `None` if not live or lacking `T`.
    pub fn get_mut<T: Component>(&mut self, entity: Entity) -> Option<&mut T> {
        if !self.allocator.is_live(entity) {
            return None;
        }
        let id = self.registry.component_id::<T>()?;
        let (aid, row) = self.archetypes.location(entity)?;
        let tick = self.change_tick;
        self.archetypes.mark_changed(aid, id, row, tick);
        let column = self.archetypes.archetype_mut(aid).column_mut(id)?;
        column_slice_mut::<T>(column)?.get_mut(row)
    }

    pub fn entity_count(&self) -> usize {
        self.allocator.live_count()
    }

    /// Archetype count, including the always-present empty archetype. For
    /// tests/metrics.
    pub fn archetype_count(&self) -> usize {
        self.archetypes.count()
    }

    /// A read-only query; `Q` must be read-only data.
    pub fn query<Q: QueryData>(&self) -> Query<'_, Q, ()> {
        Query::new_shared(&self.archetypes, &self.registry, Tick::ZERO)
    }

    /// A query permitting `&mut T` data.
    pub fn query_mut<Q: QueryData>(&mut self) -> Query<'_, Q, ()> {
        let tick = self.change_tick;
        Query::new_exclusive(&mut self.archetypes, &self.registry, tick)
    }

    pub(crate) fn query_param<Q: QueryData, F: crate::query::filter::QueryFilter>(
        &self,
        last_run: Tick,
    ) -> Query<'_, Q, F> {
        Query::new_shared(&self.archetypes, &self.registry, last_run)
    }

    /// A world-level command buffer; queued ops are applied by
    /// [`apply_commands`](World::apply_commands).
    pub fn commands(&mut self) -> Commands<'_> {
        Commands::new(&mut self.command_buffer, &self.allocator, &self.registry)
    }

    /// Applies the world-level command buffer in recorded order.
    pub fn apply_commands(&mut self) {
        let mut taken = std::mem::take(&mut self.command_buffer);
        self.apply_buffer(&mut taken);
        taken.clear();
        self.command_buffer = taken;
    }

    /// Inserts `value` as the singleton resource of type `T`, registering `T`'s
    /// id if new and overwriting any existing value.
    pub fn insert_resource<T: Resource>(&mut self, value: T) {
        self.resources.insert(value);
    }

    /// Removes and returns the resource of type `T`, leaving its id registered
    /// with an empty slot. `None` if absent.
    pub fn remove_resource<T: Resource>(&mut self) -> Option<T> {
        self.resources.remove::<T>()
    }

    pub fn contains_resource<T: Resource>(&self) -> bool {
        self.resources.contains::<T>()
    }

    /// Shared access to the resource of type `T`. Takes `&self` (a read guard via
    /// interior mutability), so it is valid in a scheduled system holding
    /// `&World`. `None` if `T` is absent.
    pub fn get_resource<T: Resource>(&self) -> Option<Res<'_, T>> {
        self.resources.get::<T>()
    }

    /// Exclusive access to the resource of type `T`. Takes `&self` (a write guard
    /// via interior mutability); uncontended by the scheduler's conflict
    /// relation, usable inside a system and via direct world access. `None` if
    /// absent.
    pub fn get_resource_mut<T: Resource>(&self) -> Option<ResMut<'_, T>> {
        self.resources.get_mut::<T>()
    }

    /// The dense id of resource `T`, or `None` if `T` was never inserted. For
    /// tests/metrics and for system access resolution.
    pub fn resource_id<T: Resource>(&self) -> Option<ResourceId> {
        self.resources.id_of::<T>()
    }

    /// Registers an event type `T`: inserts an empty `Events<T>` resource (if
    /// absent) and records its per-frame updater. Idempotent — a second call is a
    /// no-op. `EventWriter<T>`/`EventReader<T>` require this before schedule build.
    pub fn init_event<T: Event>(&mut self) {
        if self.contains_resource::<crate::events::Events<T>>() {
            return;
        }
        self.insert_resource(crate::events::Events::<T>::default());
        self.event_updaters.push(crate::events::events_updater::<T>);
    }

    /// Swaps the double buffer of every initialized event type, dropping events
    /// older than one frame. Called once per frame by
    /// [`Schedule::run`](crate::schedule::Schedule::run); allocation-free.
    pub fn update_events(&mut self) {
        let mut i = 0;
        while i < self.event_updaters.len() {
            let updater = self.event_updaters[i];
            updater(self);
            i += 1;
        }
    }

    pub(crate) fn apply_buffer(&mut self, buffer: &mut CommandBuffer) {
        for command in buffer.drain() {
            match command {
                Command::Spawn { entity, components } => {
                    if self.allocator.materialize(entity) {
                        self.archetypes.place_empty(entity);
                        if !components.is_empty() {
                            self.apply_inserts(entity, components);
                        }
                    }
                }
                Command::Despawn { entity } => {
                    if self.allocator.is_live(entity) {
                        self.archetypes.remove_entity(entity);
                        self.allocator.free(entity);
                    }
                }
                Command::Insert {
                    entity,
                    component,
                    value,
                } => {
                    if self.allocator.is_live(entity) {
                        self.apply_inserts(entity, vec![(component, value)]);
                    }
                }
                Command::Remove { entity, component } => {
                    if self.allocator.is_live(entity) {
                        self.apply_remove(entity, component);
                    }
                }
                Command::InsertResource { apply, value } => apply(self, value),
                Command::RemoveResource { remove } => remove(self),
                Command::SetParent { child, parent } => {
                    let _ = self.set_parent(child, parent);
                }
                Command::RemoveParent { child } => {
                    let _ = self.remove_parent(child);
                }
                Command::DespawnRecursive { entity } => {
                    let _ = self.despawn_recursive(entity);
                }
            }
        }
    }

    fn apply_inserts(&mut self, entity: Entity, pairs: Vec<(ComponentId, AnyValue)>) {
        let registry = &self.registry;
        let tick = self.change_tick;
        self.archetypes
            .insert_components(entity, pairs, tick, |cid| new_column(registry, cid));
    }

    fn apply_remove(&mut self, entity: Entity, id: ComponentId) -> Option<AnyValue> {
        let registry = &self.registry;
        let tick = self.change_tick;
        self.archetypes
            .remove_component(entity, id, tick, |cid| new_column(registry, cid))
    }
}

/// Builds a fresh column for a registered id. Falls back to a zero-sized unit
/// column for an unregistered id (which the archetype layer never stores into),
/// keeping the factory total without panicking.
fn new_column(
    registry: &ComponentRegistry,
    id: ComponentId,
) -> Box<dyn crate::component::ComponentColumn> {
    registry
        .new_column(id)
        .unwrap_or_else(crate::component::unit_column)
}
