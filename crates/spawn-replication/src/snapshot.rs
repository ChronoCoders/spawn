//! Snapshot serialization and acked-baseline delta compression (the Quake3 model).
//!
//! A snapshot is, for one client: the visibility delta (despawns/spawns) plus a state
//! delta for visible entities, encoded with `spawn-serialize` and delta-compressed
//! against the most recent snapshot the client acked. The delta is **per serialize
//! unit** (each `serialize_bits`/`serialize_bool` call — i.e. each field): a 1-bit
//! changed marker, and the value only when it differs from the baseline. Spawns carry
//! the full (absolute) component set; updates carry deltas. Section membership uses a
//! 1-bit continue marker per entry so a budget-truncated section ends cleanly.
//!
//! Both peers keep, per snapshot tick, a [`SnapshotState`] (each visible entity's
//! components serialized absolutely). The server deltas against the client's acked
//! state; the client decodes against its copy of that same state. The states match
//! because they are absolute serializations of the same values.

use std::collections::HashMap;
use std::time::Duration;

use spawn_ecs::{Entity, World};
use spawn_serialize::{BitReader, BitWriter, SerializeResult, Stream};

use crate::error::{ReplError, ReplResult};
use crate::id::{ReplId, ReplIdMap};
use crate::registry::{ReplComponentId, ReplicationRegistry};

/// Snapshot send rate (decision 8).
pub const SNAPSHOT_HZ: u32 = 20;
/// Snapshot send interval (`1 / SNAPSHOT_HZ`).
pub const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(50);
/// Per-tick replication send budget in bytes (decision 8). Bounds the updates section.
pub const SEND_BUDGET_BYTES: usize = 1400;
/// Per-client snapshot ring depth (≥ the `spawn-net` 32-packet ack window).
pub const SNAPSHOT_HISTORY: usize = 32;

/// Scratch size for serializing one component absolutely (for baseline recording). A
/// component larger than this yields a clean `EndOfStream` error, not a panic.
const COMPONENT_SCRATCH: usize = 256;

/// Upper bound on a spawn's `ReplId` accepted from the wire. The server keeps the id
/// space dense (it tracks peak concurrent live), so a legitimate id is far below this;
/// the cap stops a malformed/hostile snapshot from driving an unbounded client-side
/// `ReplIdMap` allocation (a `ReplId` near `u32::MAX` would otherwise grow the id table
/// to multiple GB).
const MAX_SPAWN_REPL_ID: u32 = 1 << 20;

/// Peek a snapshot's tick and the baseline tick it was encoded against, without
/// decoding the body — the client uses the baseline tick to select the baseline state
/// to pass to [`decode_snapshot`].
pub fn peek_snapshot_header(input: &[u8]) -> ReplResult<(u32, Option<u32>)> {
    let mut r = BitReader::new(input);
    let tick = r.read_bits(32)? as u32;
    let baseline_tick = if r.read_bool()? {
        Some(r.read_bits(32)? as u32)
    } else {
        None
    };
    Ok((tick, baseline_tick))
}

/// One tick's recorded absolute component bytes, keyed by `(ReplId, ReplComponentId)`.
/// A peer keeps a ring of these as candidate delta baselines.
#[derive(Default)]
pub struct SnapshotState {
    components: HashMap<(u32, u16), Vec<u8>>,
}

impl SnapshotState {
    /// The recorded absolute bytes for one component of one entity, if present.
    pub fn get(&self, id: ReplId, component: ReplComponentId) -> Option<&[u8]> {
        self.components.get(&(id.0, component.0)).map(Vec::as_slice)
    }

    fn put(&mut self, id: ReplId, component: ReplComponentId, bytes: Vec<u8>) {
        self.components.insert((id.0, component.0), bytes);
    }

    /// Whether nothing has been recorded.
    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }
}

/// Delta-or-absolute writer. With `base = None` it writes each unit's bits directly
/// (absolute). With `base = Some(reader over the baseline's bytes)` it writes a 1-bit
/// changed marker per unit and the value only when it differs from the baseline.
struct DeltaWriter<'w, 'buf, 'base> {
    out: &'w mut BitWriter<'buf>,
    base: Option<BitReader<'base>>,
}

impl Stream for DeltaWriter<'_, '_, '_> {
    fn is_writing(&self) -> bool {
        true
    }
    fn serialize_bits(&mut self, value: &mut u64, width: u32) -> SerializeResult<()> {
        match &mut self.base {
            None => self.out.write_bits(*value, width),
            Some(base) => {
                let b = base.read_bits(width)?;
                if *value == b {
                    self.out.write_bool(false)
                } else {
                    self.out.write_bool(true)?;
                    self.out.write_bits(*value, width)
                }
            }
        }
    }
    fn serialize_bool(&mut self, value: &mut bool) -> SerializeResult<()> {
        match &mut self.base {
            None => self.out.write_bool(*value),
            Some(base) => {
                let b = base.read_bool()?;
                if *value == b {
                    self.out.write_bool(false)
                } else {
                    self.out.write_bool(true)?;
                    self.out.write_bool(*value)
                }
            }
        }
    }
}

/// The decode mirror of [`DeltaWriter`]: absolute reads the unit directly; delta reads
/// a changed marker, then the value (changed) or the baseline's unit (unchanged).
struct DeltaReader<'i, 'ibuf, 'base> {
    input: &'i mut BitReader<'ibuf>,
    base: Option<BitReader<'base>>,
}

impl Stream for DeltaReader<'_, '_, '_> {
    fn is_writing(&self) -> bool {
        false
    }
    fn serialize_bits(&mut self, value: &mut u64, width: u32) -> SerializeResult<()> {
        match &mut self.base {
            None => *value = self.input.read_bits(width)?,
            Some(base) => {
                // Always consume the baseline unit to stay aligned with the writer,
                // which reads the baseline for every field regardless of the changed bit.
                let b = base.read_bits(width)?;
                *value = if self.input.read_bool()? {
                    self.input.read_bits(width)?
                } else {
                    b
                };
            }
        }
        Ok(())
    }
    fn serialize_bool(&mut self, value: &mut bool) -> SerializeResult<()> {
        match &mut self.base {
            None => *value = self.input.read_bool()?,
            Some(base) => {
                let b = base.read_bool()?;
                *value = if self.input.read_bool()? {
                    self.input.read_bool()?
                } else {
                    b
                };
            }
        }
        Ok(())
    }
}

/// Serialize one component absolutely into a reusable scratch and return its bytes
/// (for recording into a [`SnapshotState`]).
fn record_absolute(
    registry: &ReplicationRegistry,
    world: &World,
    entity: Entity,
    component: ReplComponentId,
    scratch: &mut Vec<u8>,
) -> ReplResult<Vec<u8>> {
    if scratch.len() < COMPONENT_SCRATCH {
        scratch.resize(COMPONENT_SCRATCH, 0);
    }
    let mut bw = BitWriter::new(scratch);
    {
        let mut dw = DeltaWriter {
            out: &mut bw,
            base: None,
        };
        registry.write(world, entity, component, &mut dw)?;
    }
    let n = bw.finish();
    Ok(scratch[..n].to_vec())
}

#[allow(clippy::too_many_arguments)]
fn encode_entity(
    registry: &ReplicationRegistry,
    world: &World,
    entity: Entity,
    id: ReplId,
    bw: &mut BitWriter,
    baseline: Option<&SnapshotState>,
    new_state: &mut SnapshotState,
    scratch: &mut Vec<u8>,
) -> ReplResult<()> {
    for ri in 0..registry.len() {
        let component = ReplComponentId(ri as u16);
        if !registry.present(world, entity, component) {
            continue;
        }
        let abs = record_absolute(registry, world, entity, component, scratch)?;
        bw.write_bool(true)?;
        bw.write_bits(u64::from(component.0), 16)?;
        let base_bytes = baseline.and_then(|b| b.get(id, component));
        {
            let mut dw = DeltaWriter {
                out: &mut *bw,
                base: base_bytes.map(BitReader::new),
            };
            registry.write(world, entity, component, &mut dw)?;
        }
        new_state.put(id, component, abs);
    }
    bw.write_bool(false)?;
    Ok(())
}

/// Encode one client's snapshot into `out`. `spawns` carry full state; `updates` carry
/// deltas against `baseline` and are packed in the given (priority) order until the
/// running size reaches `budget_bytes`. Returns the bytes written, the recorded current
/// [`SnapshotState`] (the next baseline for the ring), and how many `updates` were
/// actually written before the budget cut them off (so the caller's priority
/// accumulator can reset only the entities that were sent).
#[allow(clippy::too_many_arguments)]
pub fn encode_snapshot(
    registry: &ReplicationRegistry,
    world: &World,
    ids: &ReplIdMap,
    out: &mut [u8],
    tick: u32,
    baseline_tick: Option<u32>,
    baseline: Option<&SnapshotState>,
    last_input_seq: u16,
    spawns: &[ReplId],
    despawns: &[ReplId],
    updates: &[ReplId],
    budget_bytes: usize,
) -> ReplResult<(usize, SnapshotState, usize)> {
    let mut new_state = SnapshotState::default();
    let mut scratch = Vec::new();
    let mut bw = BitWriter::new(out);

    bw.write_bits(u64::from(tick), 32)?;
    match baseline_tick {
        Some(bt) => {
            bw.write_bool(true)?;
            bw.write_bits(u64::from(bt), 32)?;
        }
        None => bw.write_bool(false)?,
    }
    bw.write_bits(u64::from(last_input_seq), 16)?;

    for &id in despawns {
        bw.write_bool(true)?;
        bw.write_bits(u64::from(id.0), 32)?;
    }
    bw.write_bool(false)?;

    // Spawns (absolute, all included — a newly-visible entity needs its full baseline).
    for &id in spawns {
        let Some(entity) = ids.entity(id) else {
            continue;
        };
        bw.write_bool(true)?;
        bw.write_bits(u64::from(id.0), 32)?;
        encode_entity(
            registry,
            world,
            entity,
            id,
            &mut bw,
            None,
            &mut new_state,
            &mut scratch,
        )?;
    }
    bw.write_bool(false)?;

    let mut updates_written = 0usize;
    for &id in updates {
        if bw.bits_written().div_ceil(8) >= budget_bytes {
            break;
        }
        let Some(entity) = ids.entity(id) else {
            continue;
        };
        bw.write_bool(true)?;
        bw.write_bits(u64::from(id.0), 32)?;
        encode_entity(
            registry,
            world,
            entity,
            id,
            &mut bw,
            baseline,
            &mut new_state,
            &mut scratch,
        )?;
        updates_written += 1;
    }
    bw.write_bool(false)?;

    let n = bw.finish();
    Ok((n, new_state, updates_written))
}

/// The result of decoding a snapshot (after it has been applied to the world).
pub struct DecodeOutcome {
    /// The snapshot's tick.
    pub tick: u32,
    /// The baseline tick it was delta-encoded against, if any.
    pub baseline_tick: Option<u32>,
    /// The last client input sequence the server had processed.
    pub last_input_seq: u16,
    /// The post-snapshot recorded state (the client's next baseline for the ring).
    pub state: SnapshotState,
}

#[allow(clippy::too_many_arguments)]
fn decode_entity(
    registry: &ReplicationRegistry,
    world: &mut World,
    entity: Entity,
    id: ReplId,
    r: &mut BitReader,
    baseline: Option<&SnapshotState>,
    new_state: &mut SnapshotState,
    scratch: &mut Vec<u8>,
) -> ReplResult<()> {
    while r.read_bool()? {
        let component = ReplComponentId(r.read_bits(16)? as u16);
        if component.0 as usize >= registry.len() {
            return Err(ReplError::UnknownComponent {
                wire_id: component.0,
            });
        }
        let base_bytes = baseline.and_then(|b| b.get(id, component));
        {
            let mut dr = DeltaReader {
                input: &mut *r,
                base: base_bytes.map(BitReader::new),
            };
            registry.read(world, entity, component, &mut dr)?;
        }
        let abs = record_absolute(registry, world, entity, component, scratch)?;
        new_state.put(id, component, abs);
    }
    Ok(())
}

/// Decode a snapshot from `input` and apply it to the client `world`/`ids`, delta-
/// decoding updates against `baseline` (the client's copy of the acked snapshot state).
pub fn decode_snapshot(
    registry: &ReplicationRegistry,
    world: &mut World,
    ids: &mut ReplIdMap,
    input: &[u8],
    baseline: Option<&SnapshotState>,
) -> ReplResult<DecodeOutcome> {
    let mut new_state = SnapshotState::default();
    let mut scratch = Vec::new();
    let mut r = BitReader::new(input);

    let tick = r.read_bits(32)? as u32;
    let baseline_tick = if r.read_bool()? {
        Some(r.read_bits(32)? as u32)
    } else {
        None
    };
    let last_input_seq = r.read_bits(16)? as u16;

    while r.read_bool()? {
        let id = ReplId(r.read_bits(32)? as u32);
        if let Some(e) = ids.entity(id) {
            let _ = world.despawn(e);
            ids.release_id(id);
        }
    }

    while r.read_bool()? {
        let id = ReplId(r.read_bits(32)? as u32);
        if id.0 >= MAX_SPAWN_REPL_ID {
            return Err(ReplError::Desync {
                context: "decode: spawn id out of range",
            });
        }
        let entity = match ids.entity(id) {
            Some(e) => e,
            None => {
                let e = world.spawn();
                ids.bind(id, e);
                e
            }
        };
        decode_entity(
            registry,
            world,
            entity,
            id,
            &mut r,
            None,
            &mut new_state,
            &mut scratch,
        )?;
    }

    // Updates: delta-decode against the baseline onto an entity the client must have.
    while r.read_bool()? {
        let id = ReplId(r.read_bits(32)? as u32);
        let entity = ids.entity(id).ok_or(ReplError::Desync {
            context: "decode: update for an unknown entity",
        })?;
        decode_entity(
            registry,
            world,
            entity,
            id,
            &mut r,
            baseline,
            &mut new_state,
            &mut scratch,
        )?;
    }

    Ok(DecodeOutcome {
        tick,
        baseline_tick,
        last_input_seq,
        state: new_state,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testcomp::TPos;

    fn registry(world: &mut World) -> ReplicationRegistry {
        let mut r = ReplicationRegistry::new();
        r.register::<TPos>(world);
        r
    }

    #[test]
    fn spawn_absolute_roundtrips_to_a_client() {
        let mut sw = World::new();
        let sr = registry(&mut sw);
        let mut ids = ReplIdMap::new();
        let e = sw.spawn_with((TPos {
            x: 3.0,
            y: -7.5,
            flag: true,
        },));
        let id = ids.allocate(e);

        let mut out = [0u8; 256];
        let (n, _state, _) = encode_snapshot(
            &sr,
            &sw,
            &ids,
            &mut out,
            1,
            None,
            None,
            0,
            &[id],
            &[],
            &[],
            SEND_BUDGET_BYTES,
        )
        .unwrap();

        let mut cw = World::new();
        let cr = registry(&mut cw);
        let mut cids = ReplIdMap::new();
        let outcome = decode_snapshot(&cr, &mut cw, &mut cids, &out[..n], None).unwrap();
        assert_eq!(outcome.tick, 1);
        let ce = cids.entity(id).expect("client spawned the entity");
        assert_eq!(
            *cw.get::<TPos>(ce).unwrap(),
            TPos {
                x: 3.0,
                y: -7.5,
                flag: true
            }
        );
    }

    #[test]
    fn delta_update_only_resends_changed_fields() {
        let mut sw = World::new();
        let sr = registry(&mut sw);
        let mut ids = ReplIdMap::new();
        let e = sw.spawn_with((TPos {
            x: 1.0,
            y: 2.0,
            flag: false,
        },));
        let id = ids.allocate(e);

        let mut out = [0u8; 256];
        let (n, server_base, _) = encode_snapshot(
            &sr,
            &sw,
            &ids,
            &mut out,
            1,
            None,
            None,
            0,
            &[id],
            &[],
            &[],
            SEND_BUDGET_BYTES,
        )
        .unwrap();
        let mut cw = World::new();
        let cr = registry(&mut cw);
        let mut cids = ReplIdMap::new();
        let client_base = decode_snapshot(&cr, &mut cw, &mut cids, &out[..n], None)
            .unwrap()
            .state;

        sw.get_mut::<TPos>(e).unwrap().x = 9.0;

        let mut delta_buf = [0u8; 256];
        let (dn, _, _) = encode_snapshot(
            &sr,
            &sw,
            &ids,
            &mut delta_buf,
            2,
            Some(1),
            Some(&server_base),
            0,
            &[],
            &[],
            &[id],
            SEND_BUDGET_BYTES,
        )
        .unwrap();

        let outcome = decode_snapshot(
            &cr,
            &mut cw,
            &mut cids,
            &delta_buf[..dn],
            Some(&client_base),
        )
        .unwrap();
        assert_eq!(outcome.baseline_tick, Some(1));
        let ce = cids.entity(id).unwrap();
        assert_eq!(
            *cw.get::<TPos>(ce).unwrap(),
            TPos {
                x: 9.0,
                y: 2.0,
                flag: false
            }
        );
    }

    #[test]
    fn unchanged_fields_cost_one_bit_each() {
        // At the component level (isolating the delta benefit from snapshot framing): a
        // component delta'd against an identical baseline costs one changed-bit per
        // serialize unit, far less than the absolute encoding.
        let mut w = World::new();
        let r = registry(&mut w);
        let e = w.spawn_with((TPos {
            x: 1.0,
            y: 2.0,
            flag: true,
        },));
        let mut scratch = Vec::new();
        let abs = record_absolute(&r, &w, e, ReplComponentId(0), &mut scratch).unwrap();
        assert_eq!(
            abs.len() * 8,
            72,
            "absolute = 32+32+1 bits, byte-padded to 9 bytes"
        );

        let mut buf = [0u8; 64];
        let mut bw = BitWriter::new(&mut buf);
        {
            let mut dw = DeltaWriter {
                out: &mut bw,
                base: Some(BitReader::new(&abs)),
            };
            r.write(&w, e, ReplComponentId(0), &mut dw).unwrap();
        }
        assert_eq!(
            bw.bits_written(),
            3,
            "three serialize units, each an unchanged-bit, when nothing changed"
        );
    }

    #[test]
    fn despawn_removes_the_client_entity() {
        let mut sw = World::new();
        let sr = registry(&mut sw);
        let mut ids = ReplIdMap::new();
        let e = sw.spawn_with((TPos::default(),));
        let id = ids.allocate(e);

        let mut out = [0u8; 256];
        let (n, _, _) = encode_snapshot(
            &sr,
            &sw,
            &ids,
            &mut out,
            1,
            None,
            None,
            0,
            &[id],
            &[],
            &[],
            SEND_BUDGET_BYTES,
        )
        .unwrap();
        let mut cw = World::new();
        let cr = registry(&mut cw);
        let mut cids = ReplIdMap::new();
        decode_snapshot(&cr, &mut cw, &mut cids, &out[..n], None).unwrap();
        assert!(cids.entity(id).is_some());

        let mut out2 = [0u8; 64];
        let (n2, _, _) = encode_snapshot(
            &sr,
            &sw,
            &ids,
            &mut out2,
            2,
            None,
            None,
            0,
            &[],
            &[id],
            &[],
            SEND_BUDGET_BYTES,
        )
        .unwrap();
        decode_snapshot(&cr, &mut cw, &mut cids, &out2[..n2], None).unwrap();
        assert!(
            cids.entity(id).is_none(),
            "despawn removed the client entity"
        );
    }

    #[test]
    fn update_budget_truncates_by_priority() {
        let mut sw = World::new();
        let sr = registry(&mut sw);
        let mut ids = ReplIdMap::new();
        let n_ent = 10usize;
        let mut ents = Vec::new();
        let mut idv = Vec::new();
        for i in 0..n_ent {
            let e = sw.spawn_with((TPos {
                x: i as f32,
                y: 0.0,
                flag: false,
            },));
            ents.push(e);
            idv.push(ids.allocate(e));
        }

        let mut out = [0u8; 1024];
        let (n, server_base, _) = encode_snapshot(
            &sr,
            &sw,
            &ids,
            &mut out,
            1,
            None,
            None,
            0,
            &idv,
            &[],
            &[],
            SEND_BUDGET_BYTES,
        )
        .unwrap();
        let mut cw = World::new();
        let cr = registry(&mut cw);
        let mut cids = ReplIdMap::new();
        let client_base = decode_snapshot(&cr, &mut cw, &mut cids, &out[..n], None)
            .unwrap()
            .state;

        for (i, &e) in ents.iter().enumerate() {
            sw.get_mut::<TPos>(e).unwrap().x = 100.0 + i as f32;
        }
        let mut ub = [0u8; 1024];
        let tiny_budget = 24usize;
        let (un, _, _) = encode_snapshot(
            &sr,
            &sw,
            &ids,
            &mut ub,
            2,
            Some(1),
            Some(&server_base),
            0,
            &[],
            &[],
            &idv,
            tiny_budget,
        )
        .unwrap();
        decode_snapshot(&cr, &mut cw, &mut cids, &ub[..un], Some(&client_base)).unwrap();

        // Some entities updated (x >= 100), some kept the baseline (x < 100): truncated.
        let mut updated = 0usize;
        for &id in &idv {
            let ce = cids.entity(id).unwrap();
            if cw.get::<TPos>(ce).unwrap().x >= 100.0 {
                updated += 1;
            }
        }
        assert!(updated >= 1, "at least one update fit");
        assert!(
            updated < n_ent,
            "the budget truncated the rest ({updated}/{n_ent})"
        );
    }
}
