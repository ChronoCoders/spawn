//! RPC taxonomy: `Server` / `Client` / `Multicast` remote calls carried over the
//! `spawn-net` `ReliableOrdered` channel (the driver, commit 7, multiplexes them with
//! snapshots via the message-kind tag).
//!
//! - **Server** (client→server): only valid on an entity the sender **owns** — the
//!   ownership gate ([`server_rpc_authorized`]) is the first line of anti-cheat. The
//!   game still **validates** the payload server-side (the engine cannot know game
//!   rules); this module provides routing + the gate, not gameplay validation.
//! - **Client** (server→owning client) and **Multicast** (server→all relevant clients).
//!
//! Payloads are `spawn-serialize` types; ids are assigned densely by registration order
//! (a manifest, like components), so both peers agree on `RpcId` ↔ type.

use spawn_net::ClientId;
use spawn_serialize::{BitReader, BitWriter, Serialize};

use crate::error::{ReplError, ReplResult};
use crate::id::ReplId;
use crate::registry::fold_name;

/// The direction/fan-out of an RPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcKind {
    /// Client → server; permitted only on an entity the sender owns.
    Server,
    /// Server → the owning client of an entity.
    Client,
    /// Server → all clients to whom the target is relevant.
    Multicast,
}

impl RpcKind {
    fn to_bits(self) -> u64 {
        match self {
            RpcKind::Server => 0,
            RpcKind::Client => 1,
            RpcKind::Multicast => 2,
        }
    }
    fn from_bits(v: u64) -> ReplResult<Self> {
        match v {
            0 => Ok(RpcKind::Server),
            1 => Ok(RpcKind::Client),
            2 => Ok(RpcKind::Multicast),
            _ => Err(ReplError::Rpc {
                context: "rpc: invalid kind on the wire",
            }),
        }
    }
}

/// Dense on-wire RPC id, assigned by registration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RpcId(pub u16);

/// The framing that precedes an RPC payload: kind, id, and the optional target entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RpcHeader {
    /// Direction / fan-out.
    pub kind: RpcKind,
    /// Which RPC (the registered id).
    pub id: RpcId,
    /// The entity the call concerns (ownership gating, targeting), if any.
    pub target: Option<ReplId>,
}

impl RpcHeader {
    fn encode(&self, bw: &mut BitWriter) -> ReplResult<()> {
        bw.write_bits(self.kind.to_bits(), 2)?;
        bw.write_bits(u64::from(self.id.0), 16)?;
        match self.target {
            Some(t) => {
                bw.write_bool(true)?;
                bw.write_bits(u64::from(t.0), 32)?;
            }
            None => bw.write_bool(false)?,
        }
        Ok(())
    }

    fn decode(br: &mut BitReader) -> ReplResult<Self> {
        let kind = RpcKind::from_bits(br.read_bits(2)?)?;
        let id = RpcId(br.read_bits(16)? as u16);
        let target = if br.read_bool()? {
            Some(ReplId(br.read_bits(32)? as u32))
        } else {
            None
        };
        Ok(Self { kind, id, target })
    }
}

/// An RPC payload type the game opts in. Materialised via `Default` on decode.
pub trait Rpc: Serialize + Default {
    /// Stable name for the registration manifest (not sent on the wire).
    fn rpc_name() -> &'static str;
}

/// Assigns dense [`RpcId`]s by registration order and folds a manifest hash. Both peers
/// must register the same RPCs in the same order.
#[derive(Default)]
pub struct RpcRegistry {
    names: Vec<&'static str>,
    manifest: u64,
}

impl RpcRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register RPC type `P`, returning its dense id.
    pub fn register<P: Rpc>(&mut self) -> RpcId {
        let id = RpcId(self.names.len() as u16);
        self.names.push(P::rpc_name());
        self.manifest = fold_name(self.manifest, P::rpc_name());
        id
    }

    /// Number of registered RPC types.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether no RPCs are registered.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// The build-time manifest hash (decision 3).
    pub fn manifest(&self) -> u64 {
        self.manifest
    }

    /// Whether `id` is a registered RPC.
    pub fn contains(&self, id: RpcId) -> bool {
        (id.0 as usize) < self.names.len()
    }
}

/// Encode an RPC (header + payload) into `out`, returning the byte length.
pub fn encode_rpc<P: Rpc>(out: &mut [u8], header: RpcHeader, payload: &mut P) -> ReplResult<usize> {
    let mut bw = BitWriter::new(out);
    header.encode(&mut bw)?;
    payload.serialize(&mut bw)?;
    Ok(bw.finish())
}

/// Decode just the RPC header (to peek the kind/id/target before dispatching to the
/// payload type the id maps to).
pub fn decode_rpc_header(input: &[u8]) -> ReplResult<RpcHeader> {
    let mut br = BitReader::new(input);
    RpcHeader::decode(&mut br)
}

/// Decode an RPC's header and its `P` payload (the caller resolved `id` → `P`).
pub fn decode_rpc<P: Rpc>(input: &[u8]) -> ReplResult<(RpcHeader, P)> {
    let mut br = BitReader::new(input);
    let header = RpcHeader::decode(&mut br)?;
    let mut payload = P::default();
    payload.serialize(&mut br)?;
    Ok((header, payload))
}

/// The ownership gate for a `Server` RPC: the sender must own the target entity. The
/// driver applies this on receipt before the game handler runs (research §2.4: the
/// first line of anti-cheat). Returns `Err(Rpc)` when unauthorized.
pub fn server_rpc_authorized(sender: ClientId, target_owner: Option<ClientId>) -> ReplResult<()> {
    if target_owner == Some(sender) {
        Ok(())
    } else {
        Err(ReplError::Rpc {
            context: "rpc: server RPC on an entity the sender does not own",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_serialize::{serialize_int, SerializeResult, Stream};

    #[derive(Debug, Default, PartialEq, Clone)]
    struct Move {
        dx: i32,
        dy: i32,
        sprint: bool,
    }
    impl Serialize for Move {
        fn serialize<S: Stream>(&mut self, s: &mut S) -> SerializeResult<()> {
            let mut dx = i64::from(self.dx);
            serialize_int(s, &mut dx, 32)?;
            self.dx = dx as i32;
            let mut dy = i64::from(self.dy);
            serialize_int(s, &mut dy, 32)?;
            self.dy = dy as i32;
            s.serialize_bool(&mut self.sprint)?;
            Ok(())
        }
    }
    impl Rpc for Move {
        fn rpc_name() -> &'static str {
            "Move"
        }
    }

    #[test]
    fn header_roundtrips_each_kind_and_target() {
        for (kind, target) in [
            (RpcKind::Server, Some(ReplId(7))),
            (RpcKind::Client, None),
            (RpcKind::Multicast, Some(ReplId(0))),
        ] {
            let h = RpcHeader {
                kind,
                id: RpcId(5),
                target,
            };
            let mut buf = [0u8; 16];
            let mut bw = BitWriter::new(&mut buf);
            h.encode(&mut bw).unwrap();
            let n = bw.finish();
            assert_eq!(decode_rpc_header(&buf[..n]).unwrap(), h);
        }
    }

    #[test]
    fn invalid_kind_on_wire_is_an_error() {
        // kind bits = 3 (invalid) in the first two bits.
        let mut buf = [0u8; 8];
        let mut bw = BitWriter::new(&mut buf);
        bw.write_bits(3, 2).unwrap();
        bw.write_bits(0, 16).unwrap();
        bw.write_bool(false).unwrap();
        let n = bw.finish();
        assert!(matches!(
            decode_rpc_header(&buf[..n]),
            Err(ReplError::Rpc { .. })
        ));
    }

    #[test]
    fn rpc_payload_roundtrips() {
        let mut reg = RpcRegistry::new();
        let id = reg.register::<Move>();
        assert_eq!(id, RpcId(0));
        assert!(reg.contains(id));
        assert!(!reg.contains(RpcId(1)));
        assert!(reg.manifest() != 0);

        let header = RpcHeader {
            kind: RpcKind::Server,
            id,
            target: Some(ReplId(3)),
        };
        let mut payload = Move {
            dx: -4,
            dy: 9,
            sprint: true,
        };
        let mut buf = [0u8; 32];
        let n = encode_rpc(&mut buf, header, &mut payload).unwrap();
        let (h, p) = decode_rpc::<Move>(&buf[..n]).unwrap();
        assert_eq!(h, header);
        assert_eq!(p, payload);
    }

    #[test]
    fn ownership_gate_admits_owner_rejects_others() {
        let owner = ClientId(1);
        let other = ClientId(2);
        assert!(server_rpc_authorized(owner, Some(owner)).is_ok());
        assert!(matches!(
            server_rpc_authorized(other, Some(owner)),
            Err(ReplError::Rpc { .. })
        ));
        assert!(matches!(
            server_rpc_authorized(other, None),
            Err(ReplError::Rpc { .. })
        ));
    }
}
