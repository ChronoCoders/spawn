//! The replicated-component registry: the `Replicate` opt-in trait and the
//! fn-pointer vtables (the `spawn-ecs::reflect` pattern) that drive a component's
//! `spawn-serialize` `serialize` in either direction without per-field dynamic
//! dispatch beyond the one stream indirection.

use spawn_ecs::{Component, Entity, World};
use spawn_serialize::{Serialize, Stream};

use crate::error::{ReplError, ReplResult};

/// A component the game opts into replication. The same read/write-symmetric
/// `serialize` drives encode (server) and decode (client). `Default` lets the client
/// materialise an instance to decode a spawn into; `Clone` lets the server serialize a
/// component it only holds by shared reference.
pub trait Replicate: Component + Serialize + Clone + Default {
    /// Stable name for diagnostics / the registration manifest (not sent on the wire).
    fn replicate_name() -> &'static str;
}

/// Dense on-wire component index, assigned in registration order. Both peers must
/// register the same components in the same order (the manifest agreement).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReplComponentId(pub u16);

impl ReplComponentId {
    fn index(self) -> usize {
        self.0 as usize
    }
}

/// Forwards a `&mut dyn Stream` as a concrete `Stream` so a monomorphized
/// `T::serialize::<S>` can run against a type-erased stream. Local newtype: keeps the
/// vtable fn pointers free of stream lifetime/type parameters and avoids touching
/// `spawn-serialize`.
struct DynStream<'a> {
    inner: &'a mut dyn Stream,
}

impl Stream for DynStream<'_> {
    fn is_writing(&self) -> bool {
        self.inner.is_writing()
    }
    fn serialize_bits(
        &mut self,
        value: &mut u64,
        width: u32,
    ) -> spawn_serialize::SerializeResult<()> {
        self.inner.serialize_bits(value, width)
    }
    fn serialize_bool(&mut self, value: &mut bool) -> spawn_serialize::SerializeResult<()> {
        self.inner.serialize_bool(value)
    }
}

type PresentFn = fn(&World, Entity) -> bool;
type WriteFn = fn(&World, Entity, &mut dyn Stream) -> ReplResult<()>;
type ReadFn = fn(&mut World, Entity, &mut dyn Stream) -> ReplResult<()>;

struct Vtable {
    present: PresentFn,
    write: WriteFn,
    read: ReadFn,
}

fn present_of<T: Replicate>(world: &World, entity: Entity) -> bool {
    world.get::<T>(entity).is_some()
}

fn write_of<T: Replicate>(world: &World, entity: Entity, s: &mut dyn Stream) -> ReplResult<()> {
    let mut tmp = world
        .get::<T>(entity)
        .ok_or(ReplError::Component {
            context: "replicate: component absent on write",
        })?
        .clone();
    let mut d = DynStream { inner: s };
    tmp.serialize(&mut d)?;
    Ok(())
}

fn read_of<T: Replicate>(world: &mut World, entity: Entity, s: &mut dyn Stream) -> ReplResult<()> {
    let mut value = T::default();
    let mut d = DynStream { inner: s };
    value.serialize(&mut d)?;
    match world.get_mut::<T>(entity) {
        Some(slot) => *slot = value,
        None => world
            .insert(entity, value)
            .map_err(|_| ReplError::Component {
                context: "replicate: insert failed on apply",
            })?,
    }
    Ok(())
}

/// Maps registered component types to their (de)serialization vtables. Built once at
/// registration; the per-component dispatch is a fn-pointer call.
#[derive(Default)]
pub struct ReplicationRegistry {
    /// Indexed by [`ReplComponentId`] (registration order).
    vtables: Vec<Vtable>,
    /// Hash of the registered set, for the build-time manifest agreement (decision 3).
    manifest: u64,
}

impl ReplicationRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `T` as replicated, also registering it as an ECS component in `world`.
    /// Returns its dense on-wire id. Registration order defines the wire ids and must
    /// match on both peers.
    pub fn register<T: Replicate>(&mut self, world: &mut World) -> ReplComponentId {
        world.register::<T>();
        let id = ReplComponentId(self.vtables.len() as u16);
        self.vtables.push(Vtable {
            present: present_of::<T>,
            write: write_of::<T>,
            read: read_of::<T>,
        });
        // Fold the type name into the manifest hash (order-sensitive).
        self.manifest = fold_name(self.manifest, T::replicate_name());
        id
    }

    /// Number of registered replicated component types.
    pub fn len(&self) -> usize {
        self.vtables.len()
    }

    /// Whether no component types are registered.
    pub fn is_empty(&self) -> bool {
        self.vtables.is_empty()
    }

    /// The build-time manifest hash. Peers compare this at connect time; a mismatch is
    /// a clean rejection rather than a silent desync (decision 3).
    pub fn manifest(&self) -> u64 {
        self.manifest
    }

    fn vtable(&self, id: ReplComponentId) -> Option<&Vtable> {
        self.vtables.get(id.index())
    }

    pub(crate) fn present(&self, world: &World, entity: Entity, id: ReplComponentId) -> bool {
        self.vtable(id).is_some_and(|v| (v.present)(world, entity))
    }

    pub(crate) fn write(
        &self,
        world: &World,
        entity: Entity,
        id: ReplComponentId,
        s: &mut dyn Stream,
    ) -> ReplResult<()> {
        let v = self
            .vtable(id)
            .ok_or(ReplError::UnknownComponent { wire_id: id.0 })?;
        (v.write)(world, entity, s)
    }

    pub(crate) fn read(
        &self,
        world: &mut World,
        entity: Entity,
        id: ReplComponentId,
        s: &mut dyn Stream,
    ) -> ReplResult<()> {
        let v = self
            .vtable(id)
            .ok_or(ReplError::UnknownComponent { wire_id: id.0 })?;
        (v.read)(world, entity, s)
    }
}

/// Order-sensitive FNV-1a fold of a component name into the manifest hash.
fn fold_name(mut hash: u64, name: &str) -> u64 {
    if hash == 0 {
        hash = 0xcbf2_9ce4_8422_2325;
    }
    for b in name.bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    // Separator so [AB][C] and [A][BC] differ.
    hash ^= 0xFF;
    hash.wrapping_mul(0x0000_0100_0000_01B3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testcomp::TPos;

    #[test]
    fn register_assigns_dense_ids_and_is_idempotent_per_order() {
        let mut w = World::new();
        let mut r = ReplicationRegistry::new();
        assert!(r.is_empty());
        let a = r.register::<TPos>(&mut w);
        assert_eq!(a, ReplComponentId(0));
        assert_eq!(r.len(), 1);
        assert!(r.manifest() != 0);
    }

    #[test]
    fn present_write_read_roundtrip_for_a_component() {
        use spawn_serialize::{BitReader, BitWriter};
        let mut w = World::new();
        let mut r = ReplicationRegistry::new();
        let cid = r.register::<TPos>(&mut w);
        let e = w.spawn_with((TPos {
            x: 1.5,
            y: -2.0,
            flag: true,
        },));
        assert!(r.present(&w, e, cid));

        let mut buf = [0u8; 64];
        let mut bw = BitWriter::new(&mut buf);
        r.write(&w, e, cid, &mut bw).unwrap();
        let n = bw.finish();

        // Decode into a fresh entity in another world.
        let mut w2 = World::new();
        let mut r2 = ReplicationRegistry::new();
        r2.register::<TPos>(&mut w2);
        let e2 = w2.spawn();
        let mut br = BitReader::new(&buf[..n]);
        r2.read(&mut w2, e2, cid, &mut br).unwrap();
        assert_eq!(
            *w2.get::<TPos>(e2).unwrap(),
            TPos {
                x: 1.5,
                y: -2.0,
                flag: true
            }
        );
    }
}
