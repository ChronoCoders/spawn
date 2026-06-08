//! Deterministic, parallel system scheduling.
//!
//! A [`Stage`] holds systems in registration order — the canonical total order
//! used for conflict resolution and command-application order. `build`
//! greedily partitions systems into batches: a system joins the current batch
//! iff it conflicts with none already in it, else it opens a new batch. Batches
//! run sequentially; systems within a batch run in parallel on
//! [`std::thread::scope`] threads. This yields a unique batch layout for a given
//! registration order and access set, so observable results never depend on
//! thread scheduling.

use crate::commands::{CommandBuffer, Commands};
use crate::error::{EcsError, EcsResult};
use crate::system::{BuildableSystem, IntoSystem};
use crate::world::World;
use std::panic::AssertUnwindSafe;

struct SystemEntry {
    system: Box<dyn BuildableSystem>,
    buffer: CommandBuffer,
}

/// An ordered group of systems run as a sequence of parallel batches.
pub struct Stage {
    name: &'static str,
    systems: Vec<SystemEntry>,
    batches: Vec<Vec<usize>>,
    built: bool,
}

impl Stage {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            systems: Vec::new(),
            batches: Vec::new(),
            built: false,
        }
    }

    /// Registers a system. Registration order is the deterministic total order
    /// for batching and command application; adding a system invalidates any
    /// prior build.
    pub fn add_system<P, S>(&mut self, system: S) -> &mut Self
    where
        S: IntoSystem<P>,
        S::Sys: BuildableSystem,
    {
        self.systems.push(SystemEntry {
            system: Box::new(system.into_system()),
            buffer: CommandBuffer::new(),
        });
        self.built = false;
        self
    }

    /// Resolves access sets and builds parallel batches. Errors with
    /// [`EcsError::ComponentNotRegistered`] if a queried component is
    /// unregistered.
    pub fn build(&mut self, world: &World) -> EcsResult<()> {
        for entry in &mut self.systems {
            entry.system.resolve_access(world)?;
        }
        self.batches.clear();
        for idx in 0..self.systems.len() {
            let access = self.systems[idx].system.access().clone();
            let mut placed = false;
            for batch in &mut self.batches {
                let conflict = batch
                    .iter()
                    .any(|&other| self.systems[other].system.access().conflicts_with(&access));
                if !conflict {
                    batch.push(idx);
                    placed = true;
                    break;
                }
            }
            if !placed {
                self.batches.push(vec![idx]);
            }
        }
        self.built = true;
        Ok(())
    }

    fn run(&mut self, world: &mut World) -> EcsResult<()> {
        if !self.built {
            return Err(EcsError::ScheduleNotBuilt);
        }
        for entry in &mut self.systems {
            entry.buffer.clear();
        }
        let result = self.run_batches(world);
        if result.is_ok() {
            self.apply_buffers(world);
        }
        for entry in &mut self.systems {
            entry.buffer.clear();
        }
        result
    }

    fn run_batches(&mut self, world: &World) -> EcsResult<()> {
        let allocator = world.allocator();
        let registry = world.registry();
        let stage_name = self.name;
        // `batches` and the per-batch index buffer were sized at build time;
        // iterating them here allocates nothing.
        for batch_idx in 0..self.batches.len() {
            let batch_len = self.batches[batch_idx].len();
            if batch_len == 1 {
                // Single-system batch: run inline with no scoped thread and no
                // temporary collection — keeps the steady-state run allocation
                // free. Panics are caught and surfaced like the parallel path.
                let sys_idx = self.batches[batch_idx][0];
                let entry = &mut self.systems[sys_idx];
                let name = entry.system.name();
                let mut commands = Commands::new(&mut entry.buffer, allocator, registry);
                let system = &mut entry.system;
                let outcome =
                    std::panic::catch_unwind(AssertUnwindSafe(|| system.run(world, &mut commands)));
                match outcome {
                    Ok(result) => result?,
                    Err(_) => return Err(EcsError::SystemPanicked { system: name }),
                }
            } else {
                self.run_parallel_batch(batch_idx, world, allocator, registry, stage_name)?;
            }
        }
        Ok(())
    }

    fn run_parallel_batch(
        &mut self,
        batch_idx: usize,
        world: &World,
        allocator: &crate::entity::EntityAllocator,
        registry: &crate::component::ComponentRegistry,
        stage_name: &'static str,
    ) -> EcsResult<()> {
        let batch = &self.batches[batch_idx];
        // Borrow exactly this batch's system entries mutably and disjointly.
        let mut selected: Vec<&mut SystemEntry> = Vec::with_capacity(batch.len());
        let mut rest: &mut [SystemEntry] = &mut self.systems;
        let mut base = 0usize;
        for &idx in batch {
            let offset = idx - base;
            let taken = std::mem::take(&mut rest);
            let (_, tail) = taken.split_at_mut(offset);
            let (head, new_rest) = match tail.split_first_mut() {
                Some(parts) => parts,
                None => continue,
            };
            selected.push(head);
            rest = new_rest;
            base = idx + 1;
        }
        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(selected.len());
            for entry in selected {
                let name = entry.system.name();
                let handle = scope.spawn(move || {
                    let mut commands = Commands::new(&mut entry.buffer, allocator, registry);
                    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        entry.system.run(world, &mut commands)
                    }));
                    (name, outcome)
                });
                handles.push(handle);
            }
            let mut first_err: Option<EcsError> = None;
            for handle in handles {
                let err = match handle.join() {
                    Ok((_, Ok(Ok(())))) => None,
                    Ok((_, Ok(Err(e)))) => Some(e),
                    Ok((name, Err(_panic))) => Some(EcsError::SystemPanicked { system: name }),
                    Err(_) => Some(EcsError::SystemPanicked { system: stage_name }),
                };
                if first_err.is_none() {
                    first_err = err;
                }
            }
            match first_err {
                Some(err) => Err(err),
                None => Ok(()),
            }
        })
    }

    fn apply_buffers(&mut self, world: &mut World) {
        for entry in &mut self.systems {
            world.apply_buffer(&mut entry.buffer);
        }
    }
}

impl Stage {
    #[cfg(test)]
    pub(crate) fn batch_count(&self) -> usize {
        self.batches.len()
    }
}

/// An ordered list of stages run in sequence.
pub struct Schedule {
    stages: Vec<Stage>,
}

impl Default for Schedule {
    fn default() -> Self {
        Self::new()
    }
}

impl Schedule {
    pub fn new() -> Self {
        Self { stages: Vec::new() }
    }

    /// Appends a stage; stages run in insertion order.
    pub fn add_stage(&mut self, stage: Stage) -> &mut Self {
        self.stages.push(stage);
        self
    }

    /// Builds every stage against `world`. Must be called before [`run`](Schedule::run).
    pub fn build(&mut self, world: &World) -> EcsResult<()> {
        for stage in &mut self.stages {
            stage.build(world)?;
        }
        Ok(())
    }

    /// Runs every stage in order, applying each stage's command buffers at its
    /// boundary so a later stage observes earlier structural changes. After the
    /// first build/run, no allocation occurs in this path. After all stages
    /// complete, swaps every event double buffer once
    /// ([`World::update_events`](World::update_events)) — the single end-of-frame
    /// stage boundary. A system error aborts the failing stage (after its batch
    /// joins) and is returned; that stage's pending command buffers are discarded
    /// and the event swap is skipped for that frame.
    pub fn run(&mut self, world: &mut World) -> EcsResult<()> {
        for stage in &mut self.stages {
            stage.run(world)?;
        }
        world.update_events();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::Commands;
    use crate::query::Query;
    use crate::resource::{Res, ResMut, Resource};
    use crate::world::World;

    struct Marker;
    impl crate::component::Component for Marker {}
    struct R;
    impl Resource for R {}

    #[test]
    fn resource_conflict_splits_batches() {
        let mut world = World::new();
        world.register::<Marker>();
        world.insert_resource(R);
        let mut stage = Stage::new("s");
        stage.add_system(
            |_q: Query<'_, &Marker, ()>, _r: ResMut<'_, R>, _c: &mut Commands<'_>| Ok(()),
        );
        stage
            .add_system(|_q: Query<'_, &Marker, ()>, _r: Res<'_, R>, _c: &mut Commands<'_>| Ok(()));
        stage.build(&world).unwrap();
        assert_eq!(
            stage.batch_count(),
            2,
            "writer and reader of R must not share a batch"
        );
    }

    #[test]
    fn distinct_resource_readers_share_a_batch() {
        let mut world = World::new();
        world.register::<Marker>();
        world.insert_resource(R);
        let mut stage = Stage::new("s");
        stage
            .add_system(|_q: Query<'_, &Marker, ()>, _r: Res<'_, R>, _c: &mut Commands<'_>| Ok(()));
        stage
            .add_system(|_q: Query<'_, &Marker, ()>, _r: Res<'_, R>, _c: &mut Commands<'_>| Ok(()));
        stage.build(&world).unwrap();
        assert_eq!(
            stage.batch_count(),
            1,
            "two readers of R must share a batch"
        );
    }

    struct Ev;
    impl crate::events::Event for Ev {}

    #[test]
    fn event_writer_and_reader_conflict() {
        use crate::events::{EventReader, EventWriter};
        let mut world = World::new();
        world.init_event::<Ev>();
        let mut stage = Stage::new("s");
        stage.add_system(|mut _w: EventWriter<'_, Ev>, _c: &mut Commands<'_>| Ok(()));
        stage.add_system(|mut _r: EventReader<'_, '_, Ev>, _c: &mut Commands<'_>| Ok(()));
        stage.build(&world).unwrap();
        assert_eq!(
            stage.batch_count(),
            2,
            "an event writer and reader of the same type must not share a batch"
        );
    }

    #[test]
    fn event_readers_share_a_batch() {
        use crate::events::EventReader;
        let mut world = World::new();
        world.init_event::<Ev>();
        let mut stage = Stage::new("s");
        stage.add_system(|mut _r: EventReader<'_, '_, Ev>, _c: &mut Commands<'_>| Ok(()));
        stage.add_system(|mut _r: EventReader<'_, '_, Ev>, _c: &mut Commands<'_>| Ok(()));
        stage.build(&world).unwrap();
        assert_eq!(
            stage.batch_count(),
            1,
            "two event readers of the same type must share a batch"
        );
    }
}
