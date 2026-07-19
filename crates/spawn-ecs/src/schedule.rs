//! Deterministic, parallel system scheduling.
//!
//! A [`Stage`] holds systems in registration order, the canonical total order
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

/// A run condition: a read-only predicate over the world gating a system or set.
/// Evaluated once at the start of the stage's run, so it observes the world as of
/// stage entry (prior stages' structural changes are visible; same-stage
/// mutations are not). `Send` because it is stored in the system entry.
pub type RunCondition = Box<dyn Fn(&World) -> bool + Send>;

struct SystemEntry {
    system: Box<dyn BuildableSystem>,
    buffer: CommandBuffer,
    sets: Vec<&'static str>,
    condition: Option<RunCondition>,
    before: Vec<&'static str>,
    after: Vec<&'static str>,
}

impl SystemEntry {
    fn bare(system: Box<dyn BuildableSystem>) -> Self {
        Self {
            system,
            buffer: CommandBuffer::new(),
            sets: Vec::new(),
            condition: None,
            before: Vec::new(),
            after: Vec::new(),
        }
    }
}

/// Ordering + run-condition configuration attached to a named system set; applies
/// to every system that joins the set.
struct SetConfig {
    name: &'static str,
    condition: Option<RunCondition>,
    before: Vec<&'static str>,
    after: Vec<&'static str>,
}

/// An ordered group of systems run as a sequence of parallel batches.
///
/// Systems may be placed in named sets, gated by run conditions, and ordered with
/// `before`/`after` set constraints; [`build`](Stage::build) resolves those into a
/// single deterministic total order (topological sort, ties broken by registration
/// order) that drives batching and command application. With no configuration the
/// order is registration order.
pub struct Stage {
    name: &'static str,
    systems: Vec<SystemEntry>,
    set_configs: Vec<SetConfig>,
    order: Vec<usize>,
    batches: Vec<Vec<usize>>,
    active: Vec<bool>,
    built: bool,
}

impl Stage {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            systems: Vec::new(),
            set_configs: Vec::new(),
            order: Vec::new(),
            batches: Vec::new(),
            active: Vec::new(),
            built: false,
        }
    }

    /// Registers a system with no set/condition/ordering configuration. Adding a
    /// system invalidates any prior build.
    pub fn add_system<P, S>(&mut self, system: S) -> &mut Self
    where
        S: IntoSystem<P>,
        S::Sys: BuildableSystem,
    {
        self.systems
            .push(SystemEntry::bare(Box::new(system.into_system())));
        self.built = false;
        self
    }

    /// Registers a system and returns a [`SystemConfigurator`] to place it in
    /// sets, gate it with a run condition, or order it relative to sets.
    pub fn add_configured_system<P, S>(&mut self, system: S) -> SystemConfigurator<'_>
    where
        S: IntoSystem<P>,
        S::Sys: BuildableSystem,
    {
        self.systems
            .push(SystemEntry::bare(Box::new(system.into_system())));
        self.built = false;
        let index = self.systems.len() - 1;
        SystemConfigurator { stage: self, index }
    }

    /// Configures a named set (a shared run condition and/or ordering that applies
    /// to every member). Returns a [`SetConfigurator`]. A set need not be
    /// configured to be used as an ordering target.
    pub fn configure_set(&mut self, set: &'static str) -> SetConfigurator<'_> {
        self.built = false;
        let index = self.set_config_index(set);
        SetConfigurator { stage: self, index }
    }

    fn set_config_index(&mut self, set: &'static str) -> usize {
        if let Some(i) = self.set_configs.iter().position(|c| c.name == set) {
            return i;
        }
        self.set_configs.push(SetConfig {
            name: set,
            condition: None,
            before: Vec::new(),
            after: Vec::new(),
        });
        self.set_configs.len() - 1
    }

    fn set_config(&self, set: &'static str) -> Option<&SetConfig> {
        self.set_configs.iter().find(|c| c.name == set)
    }

    /// Resolves access sets, derives the total order from the `before`/`after`
    /// constraints, and builds parallel batches over that order. Errors with
    /// [`EcsError::ComponentNotRegistered`] if a queried component is unregistered,
    /// or [`EcsError::ScheduleCycle`] if the ordering constraints are contradictory.
    pub fn build(&mut self, world: &World) -> EcsResult<()> {
        for entry in &mut self.systems {
            entry.system.resolve_access(world)?;
        }
        let n = self.systems.len();
        let edges = self.ordering_edges();
        self.order =
            topological_order(n, &edges).ok_or(EcsError::ScheduleCycle { context: self.name })?;
        // Any ordering relationship (either direction) forces a batch boundary
        // even without a data conflict, so an explicit `before`/`after` is honored
        // for two systems that would otherwise run in parallel.
        let mut related = vec![false; n * n];
        for &(a, b) in &edges {
            related[a * n + b] = true;
            related[b * n + a] = true;
        }
        self.batches.clear();
        for &idx in &self.order {
            let access = self.systems[idx].system.access().clone();
            let mut placed = false;
            for batch in &mut self.batches {
                let conflict = batch.iter().any(|&other| {
                    related[idx * n + other]
                        || self.systems[other].system.access().conflicts_with(&access)
                });
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
        // Restore ascending order within each batch: batching walks `self.order`
        // (topological), so a batch's members may be out of index order, but the
        // parallel-batch disjoint split walks indices strictly ascending. Batch
        // members are conflict-free and command-applied via `self.order`, so their
        // within-batch order is otherwise irrelevant.
        for batch in &mut self.batches {
            batch.sort_unstable();
        }
        self.active = vec![true; n];
        self.built = true;
        Ok(())
    }

    /// The `(before, after)` labels effective for system `idx`: its own plus those
    /// inherited from every set it belongs to.
    fn effective_edges(&self, idx: usize) -> (Vec<&'static str>, Vec<&'static str>) {
        let entry = &self.systems[idx];
        let mut before = entry.before.clone();
        let mut after = entry.after.clone();
        for set in &entry.sets {
            if let Some(cfg) = self.set_config(set) {
                before.extend(cfg.before.iter().copied());
                after.extend(cfg.after.iter().copied());
            }
        }
        (before, after)
    }

    /// `(from, to)` ordering edges (`from` precedes `to`) derived from every
    /// system's effective `before`/`after` set constraints expanded over the set's
    /// members. Self-edges are skipped.
    fn ordering_edges(&self) -> Vec<(usize, usize)> {
        let mut edges = Vec::new();
        for i in 0..self.systems.len() {
            let (before, after) = self.effective_edges(i);
            for label in before {
                for j in 0..self.systems.len() {
                    if i != j && self.systems[j].sets.contains(&label) {
                        edges.push((i, j));
                    }
                }
            }
            for label in after {
                for j in 0..self.systems.len() {
                    if i != j && self.systems[j].sets.contains(&label) {
                        edges.push((j, i));
                    }
                }
            }
        }
        edges
    }

    fn run(&mut self, world: &mut World) -> EcsResult<()> {
        if !self.built {
            return Err(EcsError::ScheduleNotBuilt);
        }
        for entry in &mut self.systems {
            entry.buffer.clear();
        }
        // Evaluate run conditions once at stage entry (main thread, exclusive
        // world) into the reused `active` mask; taken out for the run so the batch
        // paths can borrow it alongside `&mut self.systems`, then restored.
        for i in 0..self.systems.len() {
            let a = self.is_active(i, world);
            self.active[i] = a;
        }
        let active = std::mem::take(&mut self.active);
        let result = self.run_batches(world, &active);
        if result.is_ok() {
            self.apply_buffers(world, &active);
        }
        for entry in &mut self.systems {
            entry.buffer.clear();
        }
        self.active = active;
        result
    }

    /// Whether system `idx` runs this frame: its own run condition (if any) and
    /// every set condition it inherits must all hold.
    fn is_active(&self, idx: usize, world: &World) -> bool {
        let entry = &self.systems[idx];
        if let Some(cond) = &entry.condition {
            if !cond(world) {
                return false;
            }
        }
        for set in &entry.sets {
            if let Some(cfg) = self.set_config(set) {
                if let Some(cond) = &cfg.condition {
                    if !cond(world) {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn run_batches(&mut self, world: &World, active: &[bool]) -> EcsResult<()> {
        let allocator = world.allocator();
        let registry = world.registry();
        let stage_name = self.name;
        // `batches` and the per-batch index buffer were sized at build time;
        // iterating them here allocates nothing.
        for batch_idx in 0..self.batches.len() {
            let batch_len = self.batches[batch_idx].len();
            if batch_len == 1 {
                // Single-system batch: run inline with no scoped thread and no
                // temporary collection, keeps the steady-state run allocation
                // free. Panics are caught and surfaced like the parallel path.
                let sys_idx = self.batches[batch_idx][0];
                if !active[sys_idx] {
                    continue;
                }
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
                self.run_parallel_batch(batch_idx, world, allocator, registry, stage_name, active)?;
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
        active: &[bool],
    ) -> EcsResult<()> {
        let batch = &self.batches[batch_idx];
        // Borrow exactly this batch's active system entries mutably and disjointly;
        // inactive systems are split off but not run.
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
            if active[idx] {
                selected.push(head);
            }
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

    fn apply_buffers(&mut self, world: &mut World, active: &[bool]) {
        // Apply in total (topological) order so command effects follow the same
        // order used for conflict resolution; skip systems that did not run.
        for &idx in &self.order {
            if active[idx] {
                world.apply_buffer(&mut self.systems[idx].buffer);
            }
        }
    }
}

/// Kahn's algorithm: a deterministic topological order of `0..n` honoring the
/// `(from, to)` precedence `edges`, ties broken by lowest index. `None` if the
/// edges contain a cycle (no total order exists). Duplicate edges are tolerated.
fn topological_order(n: usize, edges: &[(usize, usize)]) -> Option<Vec<usize>> {
    let mut in_degree = vec![0usize; n];
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(from, to) in edges {
        adjacency[from].push(to);
        in_degree[to] += 1;
    }
    let mut order = Vec::with_capacity(n);
    let mut ready: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
    while let Some(pos) = ready
        .iter()
        .enumerate()
        .min_by_key(|(_, &i)| i)
        .map(|(p, _)| p)
    {
        let node = ready.remove(pos);
        order.push(node);
        for &next in &adjacency[node] {
            in_degree[next] -= 1;
            if in_degree[next] == 0 {
                ready.push(next);
            }
        }
    }
    if order.len() == n {
        Some(order)
    } else {
        None
    }
}

/// Configures the just-added system: its set membership, run condition, and
/// ordering relative to sets. Returned by [`Stage::add_configured_system`].
pub struct SystemConfigurator<'a> {
    stage: &'a mut Stage,
    index: usize,
}

impl SystemConfigurator<'_> {
    /// Places the system in `set`, inheriting the set's ordering and run condition.
    pub fn in_set(self, set: &'static str) -> Self {
        self.stage.systems[self.index].sets.push(set);
        self
    }

    /// Orders the system before every member of `set`.
    pub fn before(self, set: &'static str) -> Self {
        self.stage.systems[self.index].before.push(set);
        self
    }

    /// Orders the system after every member of `set`.
    pub fn after(self, set: &'static str) -> Self {
        self.stage.systems[self.index].after.push(set);
        self
    }

    /// Gates the system on `condition`; when it returns false at stage entry the
    /// system is skipped for that run and its command buffer is not applied.
    pub fn run_if<F>(self, condition: F) -> Self
    where
        F: Fn(&World) -> bool + Send + 'static,
    {
        self.stage.systems[self.index].condition = Some(Box::new(condition));
        self
    }
}

/// Configures a named system set: its ordering relative to other sets and a run
/// condition shared by every member. Returned by [`Stage::configure_set`].
pub struct SetConfigurator<'a> {
    stage: &'a mut Stage,
    index: usize,
}

impl SetConfigurator<'_> {
    /// Orders every member of this set before every member of `set`.
    pub fn before(self, set: &'static str) -> Self {
        self.stage.set_configs[self.index].before.push(set);
        self
    }

    /// Orders every member of this set after every member of `set`.
    pub fn after(self, set: &'static str) -> Self {
        self.stage.set_configs[self.index].after.push(set);
        self
    }

    /// Gates every member of this set on `condition`.
    pub fn run_if<F>(self, condition: F) -> Self
    where
        F: Fn(&World) -> bool + Send + 'static,
    {
        self.stage.set_configs[self.index].condition = Some(Box::new(condition));
        self
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
    /// ([`World::update_events`](World::update_events)), the single end-of-frame
    /// stage boundary. A system error aborts the failing stage (after its batch
    /// joins) and is returned; that stage's pending command buffers are discarded
    /// and the event swap is skipped for that frame.
    pub fn run(&mut self, world: &mut World) -> EcsResult<()> {
        world.increment_change_tick();
        self.run_stages(world)?;
        world.update_events();
        Ok(())
    }

    /// Runs every stage in order *without* swapping event buffers. Used when one
    /// frame runs more than one schedule (e.g. an engine's fixed-step and
    /// variable schedules): the caller runs each with `run_stages` and then calls
    /// [`World::update_events`](World::update_events) exactly once per frame, so
    /// events are not swapped multiple times within a frame. Same per-stage
    /// command-application, determinism, and allocation guarantees as
    /// [`run`](Schedule::run); only the once-per-call event swap is omitted.
    pub fn run_stages(&mut self, world: &mut World) -> EcsResult<()> {
        for stage in &mut self.stages {
            stage.run(world)?;
        }
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

    #[test]
    fn run_stages_does_not_swap_events_but_run_does() {
        use crate::events::{EventWriter, Events};
        // One writer of Ev; each schedule pass sends exactly one event.
        let build = || {
            let mut world = World::new();
            world.init_event::<Ev>();
            let mut schedule = Schedule::new();
            let mut stage = Stage::new("emit");
            stage.add_system(|mut w: EventWriter<'_, Ev>, _c: &mut Commands<'_>| {
                w.send(Ev);
                Ok(())
            });
            schedule.add_stage(stage);
            schedule.build(&world).unwrap();
            (world, schedule)
        };

        // run_stages never swaps: three passes accumulate three events.
        let (mut world, mut schedule) = build();
        schedule.run_stages(&mut world).unwrap();
        schedule.run_stages(&mut world).unwrap();
        schedule.run_stages(&mut world).unwrap();
        assert_eq!(
            world.get_resource::<Events<Ev>>().unwrap().len(),
            3,
            "run_stages must not swap event buffers"
        );

        // run swaps once per call: events never accumulate past the two-frame
        // window, so three passes leave at most two retained.
        let (mut world, mut schedule) = build();
        schedule.run(&mut world).unwrap();
        schedule.run(&mut world).unwrap();
        schedule.run(&mut world).unwrap();
        assert!(
            world.get_resource::<Events<Ev>>().unwrap().len() <= 2,
            "run must swap event buffers once per call"
        );
    }

    struct Order(Vec<u32>);
    impl Resource for Order {}

    #[test]
    fn explicit_ordering_resolves_a_deterministic_total_order() {
        let mut world = World::new();
        world.insert_resource(Order(Vec::new()));
        let mut schedule = Schedule::new();
        let mut stage = Stage::new("s");
        // Registered z, y, x (reverse of the desired run order). ResMut serializes
        // the three, so the recorded order is exactly the resolved total order.
        stage
            .add_configured_system(|mut o: ResMut<'_, Order>, _c: &mut Commands<'_>| {
                o.0.push(3);
                Ok(())
            })
            .in_set("z");
        stage
            .add_configured_system(|mut o: ResMut<'_, Order>, _c: &mut Commands<'_>| {
                o.0.push(2);
                Ok(())
            })
            .in_set("y");
        stage
            .add_configured_system(|mut o: ResMut<'_, Order>, _c: &mut Commands<'_>| {
                o.0.push(1);
                Ok(())
            })
            .in_set("x");
        stage.configure_set("y").after("x");
        stage.configure_set("z").after("y");
        schedule.add_stage(stage);
        schedule.build(&world).unwrap();
        schedule.run_stages(&mut world).unwrap();
        assert_eq!(
            world.get_resource::<Order>().unwrap().0,
            vec![1, 2, 3],
            "x → y → z despite reverse registration"
        );
    }

    #[test]
    fn contradictory_ordering_is_a_schedule_cycle() {
        let mut world = World::new();
        world.insert_resource(Order(Vec::new()));
        let mut schedule = Schedule::new();
        let mut stage = Stage::new("s");
        stage
            .add_configured_system(|_c: &mut Commands<'_>| Ok(()))
            .in_set("s1");
        stage
            .add_configured_system(|_c: &mut Commands<'_>| Ok(()))
            .in_set("s2");
        stage.configure_set("s1").before("s2");
        stage.configure_set("s2").before("s1");
        schedule.add_stage(stage);
        assert!(matches!(
            schedule.build(&world),
            Err(EcsError::ScheduleCycle { .. })
        ));
    }

    #[test]
    fn false_run_condition_skips_the_system_and_drops_its_commands() {
        let mut world = World::new();
        let mut schedule = Schedule::new();
        let mut stage = Stage::new("s");
        stage
            .add_configured_system(|c: &mut Commands<'_>| {
                c.spawn();
                Ok(())
            })
            .run_if(|_w: &World| false);
        schedule.add_stage(stage);
        schedule.build(&world).unwrap();
        schedule.run_stages(&mut world).unwrap();
        assert_eq!(
            world.entity_count(),
            0,
            "a false run condition must skip the spawn and drop its command"
        );
    }

    #[test]
    fn true_run_condition_runs_the_system() {
        let mut world = World::new();
        let mut schedule = Schedule::new();
        let mut stage = Stage::new("s");
        stage
            .add_configured_system(|c: &mut Commands<'_>| {
                c.spawn();
                Ok(())
            })
            .run_if(|_w: &World| true);
        schedule.add_stage(stage);
        schedule.build(&world).unwrap();
        schedule.run_stages(&mut world).unwrap();
        assert_eq!(
            world.entity_count(),
            1,
            "a true run condition runs the spawn"
        );
    }

    #[test]
    fn transitively_ordered_disjoint_systems_run_without_panic() {
        // Three disjoint (conflict-free) systems chained by set ordering
        // (c → b → a) topologically order [2,1,0]; the two systems with no direct
        // edge and no data conflict share a parallel batch whose indices are not
        // ascending, exercising the parallel-batch split walk under ordering.
        let mut world = World::new();
        let mut schedule = Schedule::new();
        let mut stage = Stage::new("s");
        stage
            .add_configured_system(|c: &mut Commands<'_>| {
                c.spawn();
                Ok(())
            })
            .in_set("a");
        stage
            .add_configured_system(|c: &mut Commands<'_>| {
                c.spawn();
                Ok(())
            })
            .in_set("b");
        stage
            .add_configured_system(|c: &mut Commands<'_>| {
                c.spawn();
                Ok(())
            })
            .in_set("c");
        stage.configure_set("b").after("c");
        stage.configure_set("a").after("b");
        schedule.add_stage(stage);
        schedule.build(&world).unwrap();
        schedule.run_stages(&mut world).unwrap();
        assert_eq!(
            world.entity_count(),
            3,
            "every transitively-ordered disjoint system runs exactly once"
        );
    }

    #[test]
    fn set_run_condition_gates_every_member() {
        let mut world = World::new();
        let mut schedule = Schedule::new();
        let mut stage = Stage::new("s");
        stage
            .add_configured_system(|c: &mut Commands<'_>| {
                c.spawn();
                Ok(())
            })
            .in_set("gated");
        stage
            .add_configured_system(|c: &mut Commands<'_>| {
                c.spawn();
                Ok(())
            })
            .in_set("gated");
        stage.configure_set("gated").run_if(|_w: &World| false);
        schedule.add_stage(stage);
        schedule.build(&world).unwrap();
        schedule.run_stages(&mut world).unwrap();
        assert_eq!(
            world.entity_count(),
            0,
            "a set-level false condition gates every member"
        );
    }
}
