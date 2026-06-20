#![deny(warnings)]

//! Entity-component-system core of the Spawn engine: archetype storage,
//! allocation-free queries, deferred [`Commands`], and a deterministic
//! [`Schedule`] with `std::thread::scope` parallelism.
//!
//! Determinism conventions: system registration order is the canonical total
//! order used for conflict-resolved batching and for command-buffer application
//! at stage boundaries, so observable results never depend on thread
//! scheduling. Query iteration is allocation-free and visits archetypes in
//! `ArchetypeId` order, rows in column order.

pub mod archetype;
pub mod bundle;
pub mod change;
pub mod commands;
pub mod component;
pub mod entity;
pub mod error;
pub mod events;
pub mod query;
pub mod reflect;
pub mod resource;
pub mod schedule;
pub mod serialize;
pub mod system;
pub mod world;

pub use archetype::ArchetypeId;
pub use bundle::Bundle;
pub use change::Tick;
pub use commands::Commands;
pub use component::{Component, ComponentId};
pub use entity::Entity;
pub use error::{EcsError, EcsResult};
pub use events::{Event, EventReader, EventWriter, Events};
pub use query::filter::{Added, Changed, QueryFilter, With, Without};
pub use query::{Query, QueryData, QueryIter, QueryIterMut};
pub use reflect::{
    FieldDesc, FieldKind, FieldValue, Reflect, ReflectError, ReflectResult, ReflectedComponent,
};
pub use resource::{Res, ResMut, Resource, ResourceId};
pub use schedule::{Schedule, Stage};
pub use serialize::{serialize_entity, EntityMap, MapEntities, SerializeComponent, WireId};
pub use system::{Access, ComponentMask, IntoSystem, System, SystemParam};
pub use world::World;
