//! Systems, their access sets, and the function-to-system adapter.
//!
//! Systems read the world through queries and emit structural changes only via
//! [`Commands`]; they never receive `&mut World`, which is what makes parallel
//! execution within a stage sound. Each system's [`Access`] is computed once at
//! construction by unioning its queries' read/write sets.

use crate::commands::Commands;
use crate::component::{ComponentId, ComponentRegistry};
use crate::error::EcsResult;
use crate::query::filter::QueryFilter;
use crate::query::{Query, QueryData};
use crate::world::World;
use std::any::TypeId;

/// A growable bitset keyed by [`ComponentId`]. Growth happens at schedule-build
/// time; intersection during scheduling is `O(words)` and allocation-free.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComponentMask {
    words: Vec<u64>,
}

impl ComponentMask {
    /// Sets the bit for `id`, growing the backing storage if needed.
    pub(crate) fn insert(&mut self, id: ComponentId) {
        let bit = id.index();
        let word = bit / 64;
        if word >= self.words.len() {
            self.words.resize(word + 1, 0);
        }
        self.words[word] |= 1u64 << (bit % 64);
    }

    /// Returns `true` iff the bit for `id` is set.
    pub fn contains(&self, id: ComponentId) -> bool {
        let bit = id.index();
        let word = bit / 64;
        self.words
            .get(word)
            .map(|w| w & (1u64 << (bit % 64)) != 0)
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

/// The components a system reads and writes.
#[derive(Debug, Clone, Default)]
pub struct Access {
    reads: ComponentMask,
    writes: ComponentMask,
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

    pub fn reads(&self) -> &ComponentMask {
        &self.reads
    }

    pub fn writes(&self) -> &ComponentMask {
        &self.writes
    }

    /// Two systems conflict iff their writes overlap, or one's writes overlap
    /// the other's reads. Read-read sharing is non-conflicting.
    pub fn conflicts_with(&self, other: &Access) -> bool {
        self.writes.intersects(&other.writes)
            || self.writes.intersects(&other.reads)
            || self.reads.intersects(&other.writes)
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

/// Extension binding a system's deferred component access to dense ids at
/// schedule-build time. Kept off the public [`System`] trait so the public
/// surface matches the spec; `#[doc(hidden)]` and not for external impl.
#[doc(hidden)]
pub trait BuildableSystem: System {
    fn resolve_access(&mut self, registry: &ComponentRegistry) -> EcsResult<()>;
}

/// Converts a function/closure into a [`System`].
pub trait IntoSystem<Params> {
    /// The concrete system produced.
    type Sys: System;
    fn into_system(self) -> Self::Sys;
}

type RunFn =
    Box<dyn for<'w, 'c> FnMut(&'w World, &mut Commands<'c>) -> EcsResult<()> + Send + Sync>;

/// The concrete [`System`] produced by [`IntoSystem`] for a function. The query
/// types (which carry lifetimes) are erased into the boxed `run` closure at
/// construction; `pending` holds each queried component's
/// `(TypeId, type name, is-write)` until access is bound to dense ids at build
/// time. `#[doc(hidden)]`: an opaque adapter, not stable surface.
#[doc(hidden)]
pub struct FunctionSystem {
    run: RunFn,
    name: &'static str,
    access: Access,
    pending: Vec<(TypeId, &'static str, bool)>,
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
    fn resolve_access(&mut self, registry: &ComponentRegistry) -> EcsResult<()> {
        for (tid, name, write) in &self.pending {
            match registry.component_id_of(*tid) {
                Some(id) => {
                    if *write {
                        self.access.add_write(id);
                    } else {
                        self.access.add_read(id);
                    }
                }
                None => {
                    return Err(crate::error::EcsError::ComponentNotRegistered { component: name })
                }
            }
        }
        Ok(())
    }
}

macro_rules! impl_into_system {
    ($marker:ident, $($q:ident),*) => {
        impl<Func, $($q,)* QF> IntoSystem<($marker, fn($($q,)*), QF)> for Func
        where
            Func: FnMut($(Query<'_, $q, QF>,)* &mut Commands<'_>) -> EcsResult<()>
                + Send + Sync + 'static,
            $($q: QueryData,)*
            QF: QueryFilter + 'static,
        {
            type Sys = FunctionSystem;

            #[allow(non_snake_case)]
            fn into_system(mut self) -> FunctionSystem {
                let mut pending = Vec::new();
                $(<$q as QueryData>::access(&mut |tid, name, write| {
                    pending.push((tid, name, write));
                });)*
                let run: RunFn = Box::new(move |world: &World, commands: &mut Commands<'_>| {
                    $(let $q: Query<'_, $q, QF> = world.query_param::<$q, QF>();)*
                    self($($q,)* commands)
                });
                FunctionSystem {
                    run,
                    name: std::any::type_name::<Func>(),
                    access: Access::new(),
                    pending,
                }
            }
        }
    };
}

/// Per-arity zero-sized markers distinguishing `IntoSystem` impls (query types
/// carry lifetimes and cannot appear in the `'static` `Params` slot).
/// `#[doc(hidden)]`: an inference-only marker, not stable surface.
#[doc(hidden)]
pub struct P0;
#[doc(hidden)]
pub struct P1;
#[doc(hidden)]
pub struct P2;
#[doc(hidden)]
pub struct P3;
#[doc(hidden)]
pub struct P4;

impl<Func> IntoSystem<P0> for Func
where
    Func: FnMut(&mut Commands<'_>) -> EcsResult<()> + Send + Sync + 'static,
{
    type Sys = FunctionSystem;

    fn into_system(mut self) -> FunctionSystem {
        let run: RunFn =
            Box::new(move |_world: &World, commands: &mut Commands<'_>| self(commands));
        FunctionSystem {
            run,
            name: std::any::type_name::<Func>(),
            access: Access::new(),
            pending: Vec::new(),
        }
    }
}

impl_into_system!(P1, A);
impl_into_system!(P2, A, B);
impl_into_system!(P3, A, B, C);
impl_into_system!(P4, A, B, C, D);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::Component;

    struct Pos;
    struct Vel;
    impl Component for Pos {}
    impl Component for Vel {}

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
        boxed.resolve_access(world.registry()).unwrap();
        let pos = world.component_id::<Pos>().unwrap();
        let vel = world.component_id::<Vel>().unwrap();
        assert!(boxed.access().writes().contains(pos));
        assert!(boxed.access().reads().contains(vel));
        assert!(!boxed.access().writes().contains(vel));
    }
}
