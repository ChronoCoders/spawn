//! Component and whole-world serialization over the `spawn-serialize` bit codec.
//!
//! A component opts into [`SerializeComponent`] and is registered with the world,
//! which assigns it a dense [`WireId`] in registration order — the wire contract a
//! saved world and the world loading it must agree on. Entity-valued fields are
//! written as raw `(index, generation)` via [`serialize_entity`] and remapped after
//! load through [`MapEntities`] against an [`EntityMap`] keyed by the original
//! entity, since the codec's `serialize` is context-free and cannot resolve a
//! save-time forward map.

use std::collections::HashMap;

use spawn_serialize::{BitReader, BitWriter, Serialize, SerializeError, SerializeResult, Stream};

use crate::component::{Component, ComponentId};
use crate::entity::Entity;
use crate::error::{EcsError, EcsResult};
use crate::world::World;

/// A component the game opts into world serialization.
///
/// The read/write-symmetric [`Serialize`] drives both encode and decode; `Clone`
/// lets the encoder serialize a component it holds only by shared reference, and
/// `Default` lets the decoder materialize an instance to read into.
pub trait SerializeComponent: Component + Serialize + Clone + Default {
    /// Stable label for diagnostics; not the wire key (the [`WireId`] is).
    fn type_name() -> &'static str
    where
        Self: Sized;
}

/// A component holding [`Entity`] references that must be remapped after a load.
pub trait MapEntities {
    /// Rewrites every [`Entity`] this value holds through `map`, in place.
    fn map_entities(&mut self, map: &EntityMap);
}

/// Maps each original (pre-save) entity to the fresh entity allocated for it on
/// load. Built by [`World::deserialize_world`] and passed to every loaded
/// component's [`MapEntities::map_entities`].
#[derive(Debug)]
pub struct EntityMap {
    map: HashMap<Entity, Entity>,
}

impl EntityMap {
    pub(crate) fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub(crate) fn insert(&mut self, original: Entity, loaded: Entity) {
        self.map.insert(original, loaded);
    }

    /// The loaded entity for `original`, or `None` if it was not in the save set.
    pub fn get(&self, original: Entity) -> Option<Entity> {
        self.map.get(&original).copied()
    }

    /// The number of entities in the loaded set.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the loaded set is empty.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Reads or writes an [`Entity`] reference: the raw `(index, generation)` pair.
///
/// On a writer the entity is emitted as-is; on a reader the original entity is
/// reconstructed, to be remapped to its loaded counterpart by
/// [`MapEntities::map_entities`]. This is the only supported way to serialize an
/// entity field, keeping [`Entity`] construction internal to the crate.
pub fn serialize_entity<S: Stream>(stream: &mut S, entity: &mut Entity) -> SerializeResult<()> {
    let mut index = u64::from(entity.index());
    stream.serialize_bits(&mut index, 32)?;
    let mut generation = u64::from(entity.generation());
    stream.serialize_bits(&mut generation, 32)?;
    *entity = Entity::from_raw(index as u32, generation as u32);
    Ok(())
}

/// Dense on-wire component index, assigned in registration order.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WireId(u16);

type PresentFn = fn(&World, Entity) -> bool;
type WriteFn = fn(&World, Entity, &mut BitWriter) -> SerializeResult<()>;
pub(crate) type ReadFn = fn(&mut World, Entity, &mut BitReader) -> SerializeResult<()>;
pub(crate) type MapFn = fn(&mut World, Entity, &EntityMap);

struct SerializeVtable {
    wire_id: WireId,
    present: PresentFn,
    write: WriteFn,
}

pub(crate) struct SerializeRegistry {
    vtables: Vec<Option<SerializeVtable>>,
    decoders: Vec<(ReadFn, Option<MapFn>)>,
}

impl SerializeRegistry {
    pub(crate) fn new() -> Self {
        Self {
            vtables: Vec::new(),
            decoders: Vec::new(),
        }
    }

    pub(crate) fn insert<T: SerializeComponent>(&mut self, id: ComponentId) -> WireId {
        self.insert_impl::<T>(id, None)
    }

    pub(crate) fn insert_mapped<T: SerializeComponent>(
        &mut self,
        id: ComponentId,
        map: MapFn,
    ) -> WireId {
        self.insert_impl::<T>(id, Some(map))
    }

    fn insert_impl<T: SerializeComponent>(
        &mut self,
        id: ComponentId,
        map: Option<MapFn>,
    ) -> WireId {
        let index = id.index();
        if index >= self.vtables.len() {
            self.vtables.resize_with(index + 1, || None);
        }
        if let Some(existing) = &self.vtables[index] {
            return existing.wire_id;
        }
        let wire_id = WireId(self.decoders.len() as u16);
        self.vtables[index] = Some(SerializeVtable {
            wire_id,
            present: |world, entity| world.get::<T>(entity).is_some(),
            write: |world, entity, writer| match world.get::<T>(entity) {
                Some(component) => component.clone().serialize(writer),
                None => Ok(()),
            },
        });
        let read: ReadFn = |world, entity, reader| {
            let mut value = T::default();
            value.serialize(reader)?;
            let _ = world.insert(entity, value);
            Ok(())
        };
        self.decoders.push((read, map));
        wire_id
    }

    pub(crate) fn is_serializable(&self, id: ComponentId) -> bool {
        self.vtables
            .get(id.index())
            .map(Option::is_some)
            .unwrap_or(false)
    }

    pub(crate) fn decoders(&self) -> Vec<(ReadFn, Option<MapFn>)> {
        self.decoders.clone()
    }

    pub(crate) fn encode(&self, world: &World) -> SerializeResult<Vec<u8>> {
        let mut capacity = 1024usize;
        loop {
            let mut buf = vec![0u8; capacity];
            match self.encode_into(world, &mut buf) {
                Ok(len) => {
                    buf.truncate(len);
                    return Ok(buf);
                }
                Err(SerializeError::EndOfStream) => {
                    capacity = capacity.checked_mul(2).ok_or(SerializeError::EndOfStream)?;
                }
                Err(other) => return Err(other),
            }
        }
    }

    fn encode_into(&self, world: &World, buf: &mut [u8]) -> SerializeResult<usize> {
        let mut writer = BitWriter::new(buf);
        let entities: Vec<Entity> = world.query::<Entity>().iter().collect();
        writer.write_bits(entities.len() as u64, 32)?;
        for &entity in &entities {
            writer.write_bits(u64::from(entity.index()), 32)?;
            writer.write_bits(u64::from(entity.generation()), 32)?;
            let present: Vec<&SerializeVtable> = self
                .vtables
                .iter()
                .flatten()
                .filter(|vtable| (vtable.present)(world, entity))
                .collect();
            writer.write_bits(present.len() as u64, 16)?;
            for vtable in present {
                writer.write_bits(u64::from(vtable.wire_id.0), 16)?;
                (vtable.write)(world, entity, &mut writer)?;
            }
        }
        Ok(writer.finish())
    }
}

pub(crate) fn decode(
    world: &mut World,
    decoders: &[(ReadFn, Option<MapFn>)],
    bytes: &[u8],
) -> EcsResult<EntityMap> {
    world.clear_entities();
    let mut reader = BitReader::new(bytes);
    let count = reader.read_bits(32)? as usize;
    let mut map = EntityMap::new();
    let mut to_remap: Vec<(Entity, MapFn)> = Vec::new();
    for _ in 0..count {
        let index = reader.read_bits(32)? as u32;
        let generation = reader.read_bits(32)? as u32;
        let original = Entity::from_raw(index, generation);
        let entity = world.spawn();
        map.insert(original, entity);
        let component_count = reader.read_bits(16)? as usize;
        for _ in 0..component_count {
            let wire = reader.read_bits(16)? as usize;
            let (read, map_fn) = decoders
                .get(wire)
                .copied()
                .ok_or(EcsError::UnknownWireId { wire: wire as u16 })?;
            read(world, entity, &mut reader)?;
            if let Some(map_fn) = map_fn {
                to_remap.push((entity, map_fn));
            }
        }
    }
    for (entity, map_fn) in to_remap {
        map_fn(world, entity, &map);
    }
    Ok(map)
}

impl SerializeComponent for spawn_core::Transform3D {
    fn type_name() -> &'static str {
        "Transform3D"
    }
}

impl SerializeComponent for spawn_core::Transform2D {
    fn type_name() -> &'static str {
        "Transform2D"
    }
}
