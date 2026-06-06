//! Deferred structural changes.
//!
//! [`Commands`] records `spawn`/`despawn`/`insert`/`remove` into a per-system
//! buffer during `run`; the buffer is applied to the world at the stage
//! boundary in deterministic order. Buffers are allocated once at schedule-build
//! time and cleared (not freed) each frame, so recording allocates nothing after
//! warm-up. Reserved ids come from the allocator's atomic high-water counter, so
//! concurrent systems never collide.

use crate::bundle::Bundle;
use crate::component::{AnyValue, Component, ComponentId, ComponentRegistry};
use crate::entity::{Entity, EntityAllocator};

/// One recorded structural mutation.
pub(crate) enum Command {
    Spawn {
        entity: Entity,
        components: Vec<(ComponentId, AnyValue)>,
    },
    Despawn {
        entity: Entity,
    },
    Insert {
        entity: Entity,
        component: ComponentId,
        value: AnyValue,
    },
    Remove {
        entity: Entity,
        component: ComponentId,
    },
}

/// A reusable per-system buffer of recorded [`Command`]s.
#[derive(Default)]
pub(crate) struct CommandBuffer {
    commands: Vec<Command>,
}

impl CommandBuffer {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn clear(&mut self) {
        self.commands.clear();
    }

    pub(crate) fn drain(&mut self) -> std::vec::Drain<'_, Command> {
        self.commands.drain(..)
    }
}

/// Records deferred structural changes for one system. Borrows a command buffer
/// (not world storage) plus the allocator for reserving spawn ids.
pub struct Commands<'w> {
    buffer: &'w mut CommandBuffer,
    allocator: &'w EntityAllocator,
    registry: &'w ComponentRegistry,
}

impl<'w> Commands<'w> {
    pub(crate) fn new(
        buffer: &'w mut CommandBuffer,
        allocator: &'w EntityAllocator,
        registry: &'w ComponentRegistry,
    ) -> Self {
        Self {
            buffer,
            allocator,
            registry,
        }
    }

    /// Reserves an entity id immediately (from the allocator's atomic counter);
    /// the slot becomes live only when the buffer is applied.
    pub fn spawn(&mut self) -> Entity {
        let entity = self.allocator.reserve();
        self.buffer.commands.push(Command::Spawn {
            entity,
            components: Vec::new(),
        });
        entity
    }

    /// Reserves an entity id and records its bundle for deferred placement.
    pub fn spawn_with<B: Bundle>(&mut self, bundle: B) -> Entity {
        let entity = self.allocator.reserve();
        let components = self.write_bundle(bundle);
        self.buffer
            .commands
            .push(Command::Spawn { entity, components });
        entity
    }

    /// Records a despawn. A no-op at apply time if the entity is already dead.
    pub fn despawn(&mut self, entity: Entity) {
        self.buffer.commands.push(Command::Despawn { entity });
    }

    /// Records an insert. Dropped silently at apply time if the entity is dead.
    pub fn insert<T: Component>(&mut self, entity: Entity, value: T) {
        if let Some(component) = self.registry.component_id::<T>() {
            self.buffer.commands.push(Command::Insert {
                entity,
                component,
                value: Box::new(value),
            });
        }
    }

    /// Records a remove. Dropped silently at apply time if the entity is dead or
    /// lacks the component, or if `T` is unregistered.
    pub fn remove<T: Component>(&mut self, entity: Entity) {
        if let Some(component) = self.registry.component_id::<T>() {
            self.buffer
                .commands
                .push(Command::Remove { entity, component });
        }
    }

    fn write_bundle<B: Bundle>(&self, bundle: B) -> Vec<(ComponentId, AnyValue)> {
        bundle.write_into_registered(self.registry)
    }
}
