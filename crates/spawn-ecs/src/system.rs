//! Systems, their access sets, and the function-to-system adapter.
//!
//! Systems read the world through queries and emit structural changes only via
//! [`Commands`]; they never receive `&mut World`, which is what makes parallel
//! execution within a stage sound. Each system's [`Access`] is computed once at
//! construction by unioning its queries' read/write sets.

use crate::commands::Commands;
use crate::component::ComponentId;
use crate::error::{EcsError, EcsResult};
use crate::query::filter::QueryFilter;
use crate::query::{Query, QueryData};
use crate::resource::{Res, ResMut, Resource, ResourceId};
use crate::world::World;

/// A growable bitset keyed by [`ComponentId`]. Growth happens at schedule-build
/// time; intersection during scheduling is `O(words)` and allocation-free.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComponentMask {
    words: Vec<u64>,
}

impl ComponentMask {
    /// Sets the bit for `id`, growing the backing storage if needed.
    pub(crate) fn insert(&mut self, id: ComponentId) {
        self.insert_index(id.index());
    }

    /// Sets the bit at raw `index` (used for both component and resource ids).
    pub(crate) fn insert_index(&mut self, index: usize) {
        let word = index / 64;
        if word >= self.words.len() {
            self.words.resize(word + 1, 0);
        }
        self.words[word] |= 1u64 << (index % 64);
    }

    /// Returns `true` iff the bit for `id` is set.
    pub fn contains(&self, id: ComponentId) -> bool {
        self.contains_index(id.index())
    }

    /// Returns `true` iff the bit at raw `index` is set.
    pub(crate) fn contains_index(&self, index: usize) -> bool {
        let word = index / 64;
        self.words
            .get(word)
            .map(|w| w & (1u64 << (index % 64)) != 0)
            .unwrap_or(false)
    }

    /// Returns `true` iff the two masks share at least one set bit.
    pub fn intersects(&self, other: &ComponentMask) -> bool {
        self.words
            .iter()
            .zip(other.words.iter())
            .any(|(a, b)| a & b != 0)
    }

    /// Returns `true` iff no bit is set.
    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|w| *w == 0)
    }
}

/// The components and resources a system reads and writes. Component and
/// resource access are tracked in disjoint masks (the resource masks are keyed
/// by [`ResourceId`](crate::resource::ResourceId), the component masks by
/// [`ComponentId`]).
#[derive(Debug, Clone, Default)]
pub struct Access {
    reads: ComponentMask,
    writes: ComponentMask,
    resource_reads: ComponentMask,
    resource_writes: ComponentMask,
}

impl Access {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn add_read(&mut self, id: ComponentId) {
        self.reads.insert(id);
    }

    pub(crate) fn add_write(&mut self, id: ComponentId) {
        self.writes.insert(id);
    }

    pub(crate) fn add_resource_read(&mut self, id: ResourceId) {
        self.resource_reads.insert_index(id.index());
    }

    pub(crate) fn add_resource_write(&mut self, id: ResourceId) {
        self.resource_writes.insert_index(id.index());
    }

    pub fn reads(&self) -> &ComponentMask {
        &self.reads
    }

    pub fn writes(&self) -> &ComponentMask {
        &self.writes
    }

    pub fn resource_reads(&self) -> &ComponentMask {
        &self.resource_reads
    }

    pub fn resource_writes(&self) -> &ComponentMask {
        &self.resource_writes
    }

    /// Two systems conflict iff their component or resource access overlaps in a
    /// read-write or write-write pair. Read-read sharing, on a component or a
    /// resource, is non-conflicting.
    pub fn conflicts_with(&self, other: &Access) -> bool {
        self.writes.intersects(&other.writes)
            || self.writes.intersects(&other.reads)
            || self.reads.intersects(&other.writes)
            || self.resource_writes.intersects(&other.resource_writes)
            || self.resource_writes.intersects(&other.resource_reads)
            || self.resource_reads.intersects(&other.resource_writes)
    }
}

/// A schedulable unit of work over the world.
pub trait System: Send + Sync + 'static {
    /// Stable name, used for batching diagnostics and error reporting.
    fn name(&self) -> &'static str;
    /// The system's read/write access set, resolved at build time.
    fn access(&self) -> &Access;
    /// Runs the system. Structural changes go through `commands`, applied at the
    /// stage boundary. Returning `Err` aborts the stage.
    fn run(&mut self, world: &World, commands: &mut Commands<'_>) -> EcsResult<()>;
}

/// Extension binding a system's deferred component/resource access to dense ids
/// at schedule-build time. Kept off the public [`System`] trait so the public
/// surface matches the spec; `#[doc(hidden)]` and not for external impl.
#[doc(hidden)]
pub trait BuildableSystem: System {
    fn resolve_access(&mut self, world: &World) -> EcsResult<()>;
}

/// A typed view of the world that a system declares as a parameter. Its access
/// set is derived at build time and the value is constructed fresh each run from
/// the shared `&World` plus the system's own persistent state.
///
/// Implemented for [`Query`], [`Res`], and [`ResMut`]. [`Commands`] is not a
/// `SystemParam`: it borrows the system's own command buffer (not `&World`) and
/// is the mandatory trailing parameter of every system.
pub trait SystemParam {
    /// Persistent per-system local state, owned across frames and
    /// `Default`-initialized at construction.
    type State: Default + Send + Sync + 'static;
    /// The value handed to the system body for one run.
    type Item<'w, 's>;

    /// Unions this parameter's component/resource access into `access` at build
    /// time, resolving types to dense ids against the live world.
    fn resolve_access(world: &World, access: &mut Access) -> EcsResult<()>;

    /// Constructs the parameter for one run from the shared world and the
    /// system's local state.
    fn get<'w, 's>(world: &'w World, state: &'s mut Self::State) -> EcsResult<Self::Item<'w, 's>>;
}

impl<Q: QueryData, F: QueryFilter + 'static> SystemParam for Query<'_, Q, F> {
    type State = crate::query::QueryState;
    type Item<'w, 's> = Query<'w, Q, F>;

    fn resolve_access(world: &World, access: &mut Access) -> EcsResult<()> {
        let mut err: Option<EcsError> = None;
        Q::access(&mut |tid, name, write| {
            if err.is_some() {
                return;
            }
            match world.registry().component_id_of(tid) {
                Some(id) => {
                    if write {
                        access.add_write(id);
                    } else {
                        access.add_read(id);
                    }
                }
                None => err = Some(EcsError::ComponentNotRegistered { component: name }),
            }
        });
        match err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn get<'w>(
        world: &'w World,
        state: &mut crate::query::QueryState,
    ) -> EcsResult<Query<'w, Q, F>> {
        let last_run = state.take_last_run(world.change_tick());
        Ok(world.query_param::<Q, F>(last_run))
    }
}

impl<T: Resource> SystemParam for Res<'_, T> {
    type State = ();
    type Item<'w, 's> = Res<'w, T>;

    fn resolve_access(world: &World, access: &mut Access) -> EcsResult<()> {
        match world.resource_id::<T>() {
            Some(id) => {
                access.add_resource_read(id);
                Ok(())
            }
            None => Err(EcsError::ResourceNotRegistered {
                resource: std::any::type_name::<T>(),
            }),
        }
    }

    fn get<'w>(world: &'w World, _state: &mut ()) -> EcsResult<Res<'w, T>> {
        world
            .get_resource::<T>()
            .ok_or(EcsError::ResourceNotRegistered {
                resource: std::any::type_name::<T>(),
            })
    }
}

impl<T: Resource> SystemParam for ResMut<'_, T> {
    type State = ();
    type Item<'w, 's> = ResMut<'w, T>;

    fn resolve_access(world: &World, access: &mut Access) -> EcsResult<()> {
        match world.resource_id::<T>() {
            Some(id) => {
                access.add_resource_write(id);
                Ok(())
            }
            None => Err(EcsError::ResourceNotRegistered {
                resource: std::any::type_name::<T>(),
            }),
        }
    }

    fn get<'w>(world: &'w World, _state: &mut ()) -> EcsResult<ResMut<'w, T>> {
        world
            .get_resource_mut::<T>()
            .ok_or(EcsError::ResourceNotRegistered {
                resource: std::any::type_name::<T>(),
            })
    }
}

/// Converts a function/closure into a [`System`].
pub trait IntoSystem<Params> {
    /// The concrete system produced.
    type Sys: System;
    fn into_system(self) -> Self::Sys;
}

type RunFn =
    Box<dyn for<'w, 'c> FnMut(&'w World, &mut Commands<'c>) -> EcsResult<()> + Send + Sync>;
type ResolveFn = Box<dyn Fn(&World, &mut Access) -> EcsResult<()> + Send + Sync>;

/// The concrete [`System`] produced by [`IntoSystem`] for a function. The
/// parameters (which carry lifetimes) are erased into the boxed `run` closure at
/// construction, which also owns the parameters' persistent state by `move`; the
/// boxed `resolve` closure unions parameter access at build time. `#[doc(hidden)]`:
/// an opaque adapter, not stable surface.
#[doc(hidden)]
pub struct FunctionSystem {
    run: RunFn,
    resolve: ResolveFn,
    name: &'static str,
    access: Access,
}

impl System for FunctionSystem {
    fn name(&self) -> &'static str {
        self.name
    }

    fn access(&self) -> &Access {
        &self.access
    }

    fn run(&mut self, world: &World, commands: &mut Commands<'_>) -> EcsResult<()> {
        (self.run)(world, commands)
    }
}

impl BuildableSystem for FunctionSystem {
    fn resolve_access(&mut self, world: &World) -> EcsResult<()> {
        self.access = Access::new();
        (self.resolve)(world, &mut self.access)
    }
}

impl<Func> IntoSystem<fn()> for Func
where
    Func: FnMut(&mut Commands<'_>) -> EcsResult<()> + Send + Sync + 'static,
{
    type Sys = FunctionSystem;

    fn into_system(mut self) -> FunctionSystem {
        let run: RunFn =
            Box::new(move |_world: &World, commands: &mut Commands<'_>| self(commands));
        let resolve: ResolveFn = Box::new(|_world, _access| Ok(()));
        FunctionSystem {
            run,
            resolve,
            name: std::any::type_name::<Func>(),
            access: Access::new(),
        }
    }
}

macro_rules! impl_into_system {
    ($($param:ident),+) => {
        impl<Func, $($param,)+> IntoSystem<fn($($param,)+)> for Func
        where
            Func: Send + Sync + 'static,
            $($param: SystemParam,)+
            for<'a> &'a mut Func:
                FnMut($($param,)+ &mut Commands<'_>) -> EcsResult<()>
                + FnMut($(<$param as SystemParam>::Item<'a, 'a>,)+ &mut Commands<'_>)
                    -> EcsResult<()>,
        {
            type Sys = FunctionSystem;

            // Tuple-macro idiom: the type params `$param` are reused as value binding
            // names (upper-camel-case), so the snake-case lint is suppressed here.
            #[allow(non_snake_case)]
            fn into_system(mut self) -> FunctionSystem {
                let resolve: ResolveFn = Box::new(|world: &World, access: &mut Access| {
                    $(<$param as SystemParam>::resolve_access(world, access)?;)+
                    Ok(())
                });
                let mut state: ($(<$param as SystemParam>::State,)+) = Default::default();
                let run: RunFn = Box::new(move |world: &World, commands: &mut Commands<'_>| {
                    // A generic inner call nudges inference so the user closure's
                    // declared parameter types unify with each param's `Item` (the
                    // standard system-function dispatch idiom).
                    #[allow(non_snake_case)]
                    #[allow(clippy::too_many_arguments)]
                    fn call_inner<$($param,)+>(
                        mut f: impl FnMut($($param,)+ &mut Commands<'_>) -> EcsResult<()>,
                        $($param: $param,)+
                        commands: &mut Commands<'_>,
                    ) -> EcsResult<()> {
                        f($($param,)+ commands)
                    }
                    let ($($param,)+) = &mut state;
                    $(let $param = <$param as SystemParam>::get(world, $param)?;)+
                    call_inner(&mut self, $($param,)+ commands)
                });
                FunctionSystem {
                    run,
                    resolve,
                    name: std::any::type_name::<Func>(),
                    access: Access::new(),
                }
            }
        }
    };
}

impl_into_system!(P1);
impl_into_system!(P1, P2);
impl_into_system!(P1, P2, P3);
impl_into_system!(P1, P2, P3, P4);
impl_into_system!(P1, P2, P3, P4, P5);
impl_into_system!(P1, P2, P3, P4, P5, P6);
impl_into_system!(P1, P2, P3, P4, P5, P6, P7);
impl_into_system!(P1, P2, P3, P4, P5, P6, P7, P8);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::Component;

    struct Pos;
    struct Vel;
    impl Component for Pos {}
    impl Component for Vel {}

    struct Clock;
    struct Budget;
    impl Resource for Clock {}
    impl Resource for Budget {}

    fn mask(ids: &[ComponentId]) -> ComponentMask {
        let mut m = ComponentMask::default();
        for &id in ids {
            m.insert(id);
        }
        m
    }

    #[test]
    fn mask_set_and_intersect() {
        let a = mask(&[ComponentId::from_raw(0), ComponentId::from_raw(70)]);
        let b = mask(&[ComponentId::from_raw(70)]);
        let c = mask(&[ComponentId::from_raw(1)]);
        assert!(a.intersects(&b));
        assert!(!a.intersects(&c));
        assert!(a.contains(ComponentId::from_raw(70)));
        assert!(!c.is_empty());
        assert!(ComponentMask::default().is_empty());
    }

    #[test]
    fn conflict_truth_table() {
        let id0 = ComponentId::from_raw(0);
        let mut read_a = Access::new();
        read_a.add_read(id0);
        let mut read_b = Access::new();
        read_b.add_read(id0);
        let mut write_a = Access::new();
        write_a.add_write(id0);
        let mut write_b = Access::new();
        write_b.add_write(id0);

        assert!(!read_a.conflicts_with(&read_b));
        assert!(read_a.conflicts_with(&write_b));
        assert!(write_a.conflicts_with(&read_b));
        assert!(write_a.conflicts_with(&write_b));
    }

    #[test]
    fn into_system_resolves_access() {
        let mut world = World::new();
        world.register::<Pos>();
        world.register::<Vel>();
        let system =
            (|_q: Query<'_, (&mut Pos, &Vel), ()>, _c: &mut Commands<'_>| Ok(())).into_system();
        let mut boxed = system;
        boxed.resolve_access(&world).unwrap();
        let pos = world.component_id::<Pos>().unwrap();
        let vel = world.component_id::<Vel>().unwrap();
        assert!(boxed.access().writes().contains(pos));
        assert!(boxed.access().reads().contains(vel));
        assert!(!boxed.access().writes().contains(vel));
    }

    fn access_of<P, S>(world: &World, system: S) -> Access
    where
        S: IntoSystem<P>,
        S::Sys: BuildableSystem,
    {
        let mut boxed = system.into_system();
        boxed.resolve_access(world).unwrap();
        boxed.access().clone()
    }

    #[test]
    fn res_and_resmut_resolve_resource_access() {
        let mut world = World::new();
        world.insert_resource(Clock);
        world.insert_resource(Budget);
        let clock = world.resource_id::<Clock>().unwrap();
        let budget = world.resource_id::<Budget>().unwrap();

        let reader = access_of(&world, |_r: Res<'_, Clock>, _c: &mut Commands<'_>| Ok(()));
        assert!(reader.resource_reads().contains_index(clock.index()));
        assert!(!reader.resource_writes().contains_index(clock.index()));

        let writer = access_of(&world, |_r: ResMut<'_, Budget>, _c: &mut Commands<'_>| {
            Ok(())
        });
        assert!(writer.resource_writes().contains_index(budget.index()));
        assert!(!writer.resource_reads().contains_index(budget.index()));
    }

    #[test]
    fn resource_conflict_truth_table() {
        let mut world = World::new();
        world.insert_resource(Clock);

        let read_a = access_of(&world, |_r: Res<'_, Clock>, _c: &mut Commands<'_>| Ok(()));
        let read_b = access_of(&world, |_r: Res<'_, Clock>, _c: &mut Commands<'_>| Ok(()));
        let write_a = access_of(
            &world,
            |_r: ResMut<'_, Clock>, _c: &mut Commands<'_>| Ok(()),
        );
        let write_b = access_of(
            &world,
            |_r: ResMut<'_, Clock>, _c: &mut Commands<'_>| Ok(()),
        );

        assert!(!read_a.conflicts_with(&read_b));
        assert!(read_a.conflicts_with(&write_b));
        assert!(write_a.conflicts_with(&read_b));
        assert!(write_a.conflicts_with(&write_b));
    }

    #[test]
    fn different_resources_do_not_conflict() {
        let mut world = World::new();
        world.insert_resource(Clock);
        world.insert_resource(Budget);
        let clock_writer = access_of(
            &world,
            |_r: ResMut<'_, Clock>, _c: &mut Commands<'_>| Ok(()),
        );
        let budget_writer = access_of(&world, |_r: ResMut<'_, Budget>, _c: &mut Commands<'_>| {
            Ok(())
        });
        assert!(!clock_writer.conflicts_with(&budget_writer));
    }

    #[test]
    fn unregistered_resource_is_build_error() {
        let world = World::new();
        let mut boxed = (|_r: Res<'_, Clock>, _c: &mut Commands<'_>| Ok(())).into_system();
        assert!(matches!(
            boxed.resolve_access(&world),
            Err(EcsError::ResourceNotRegistered { .. })
        ));
    }
}
