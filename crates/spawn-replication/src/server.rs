//! The server-side replication driver. Owns the interest state, the per-client
//! snapshot rings, and the entity→`ReplId` map; pumped once per snapshot tick to build
//! and send each client an interest-filtered, delta-compressed snapshot, and to surface
//! inbound inputs and (ownership-gated) RPC requests for the game.

use std::collections::HashMap;

use spawn_ecs::{Entity, World};
use spawn_net::{ChannelId, ClientId, NetEvent, Server};
use spawn_serialize::BitReader;

use crate::config::ReplicationConfig;
use crate::error::{ReplError, ReplResult};
use crate::id::{ReplId, ReplIdMap};
use crate::interest::{ReplicationVisibility, VisibilityConfig};
use crate::markers::{OwnerOnly, Replicated};
use crate::registry::ReplicationRegistry;
use crate::rpc::{
    decode_rpc_header, encode_rpc, server_rpc_authorized, Rpc, RpcHeader, RpcKind, RpcRegistry,
};
use crate::snapshot::{encode_snapshot, SnapshotState, SNAPSHOT_HISTORY};
use crate::wire::{TAG_INPUT, TAG_RPC, TAG_SNAPSHOT};

/// One inbound input from a client (raw payload; the game decodes its own input type).
pub struct ServerInput {
    /// The sender.
    pub client: ClientId,
    /// The input's sequence number.
    pub seq: u16,
    /// The encoded input payload.
    pub bytes: Vec<u8>,
}

/// One inbound RPC request from a client (already ownership-gated for `Server` RPCs).
pub struct ServerRpc {
    /// The sender.
    pub client: ClientId,
    /// The decoded routing header.
    pub header: RpcHeader,
    /// The full RPC bytes (header + payload) for the game to decode via its registry.
    pub bytes: Vec<u8>,
}

/// What a server [`tick`](Replicator::tick) surfaced to the game this tick.
#[derive(Default)]
pub struct ServerEvents {
    /// Clients that connected this tick.
    pub connected: Vec<ClientId>,
    /// Clients that disconnected this tick.
    pub disconnected: Vec<ClientId>,
    /// Inbound inputs.
    pub inputs: Vec<ServerInput>,
    /// Inbound, ownership-gated RPC requests.
    pub rpcs: Vec<ServerRpc>,
}

struct RingSlot {
    tick: u32,
    state: SnapshotState,
    used: bool,
}

struct ServerClient {
    client: ClientId,
    ring: Vec<RingSlot>,
    acked_tick: Option<u32>,
    last_input_seq: u16,
    /// Per-entity tick of last send, for the staleness priority accumulator.
    last_sent: HashMap<ReplId, u32>,
}

impl ServerClient {
    fn new(client: ClientId) -> Self {
        let mut ring = Vec::with_capacity(SNAPSHOT_HISTORY);
        for _ in 0..SNAPSHOT_HISTORY {
            ring.push(RingSlot {
                tick: 0,
                state: SnapshotState::default(),
                used: false,
            });
        }
        Self {
            client,
            ring,
            acked_tick: None,
            last_input_seq: 0,
            last_sent: HashMap::new(),
        }
    }

    fn ring_get(&self, tick: u32) -> Option<&SnapshotState> {
        let slot = &self.ring[tick as usize % SNAPSHOT_HISTORY];
        (slot.used && slot.tick == tick).then_some(&slot.state)
    }

    fn ring_put(&mut self, tick: u32, state: SnapshotState) {
        self.ring[tick as usize % SNAPSHOT_HISTORY] = RingSlot {
            tick,
            state,
            used: true,
        };
    }
}

/// The server replication driver.
pub struct Replicator {
    registry: ReplicationRegistry,
    rpcs: RpcRegistry,
    vis: ReplicationVisibility,
    ids: ReplIdMap,
    clients: Vec<ServerClient>,
    config: ReplicationConfig,
    scratch: Vec<u8>,
    visible: Vec<ReplId>,
    dead: Vec<(Entity, ReplId)>,
}

impl Replicator {
    /// A new driver with the given configuration.
    pub fn new(config: ReplicationConfig) -> Self {
        Self {
            registry: ReplicationRegistry::new(),
            rpcs: RpcRegistry::new(),
            vis: ReplicationVisibility::new(VisibilityConfig::from_view_radius(
                config.default_view_radius,
            )),
            ids: ReplIdMap::new(),
            clients: Vec::new(),
            config,
            scratch: Vec::new(),
            visible: Vec::new(),
            dead: Vec::new(),
        }
    }

    /// The component registry, for the game to register replicated component types.
    pub fn registry_mut(&mut self) -> &mut ReplicationRegistry {
        &mut self.registry
    }

    /// The RPC registry, for the game to register RPC types.
    pub fn rpcs_mut(&mut self) -> &mut RpcRegistry {
        &mut self.rpcs
    }

    /// Set (or update) a client's viewer entity and interest radius. The game calls this
    /// once it has assigned the client's pawn.
    pub fn set_viewer(&mut self, client: ClientId, viewer: Entity, view_radius: f32) {
        self.vis.add_client(client, viewer, view_radius);
    }

    /// Send a server-originated RPC (`Client` or `Multicast`) to `client` over the
    /// reliable channel.
    pub fn send_rpc<P: Rpc>(
        &mut self,
        net: &mut Server,
        client: ClientId,
        header: RpcHeader,
        payload: &mut P,
    ) -> ReplResult<()> {
        self.scratch.resize(self.config.send_budget + 1024, 0);
        self.scratch[0] = TAG_RPC;
        let n = encode_rpc(&mut self.scratch[1..], header, payload)?;
        net.send(client, ChannelId::ReliableOrdered, &self.scratch[..1 + n])
            .map_err(|_| ReplError::Transport {
                context: "server: rpc send failed",
            })
    }

    fn client_mut(&mut self, client: ClientId) -> Option<&mut ServerClient> {
        self.clients.iter_mut().find(|c| c.client == client)
    }

    /// Advance one tick: drain `net`, run interest management over `world`, send each
    /// client its snapshot, and return the inbound events.
    pub fn tick(
        &mut self,
        world: &mut World,
        net: &mut Server,
        now_tick: u32,
    ) -> ReplResult<ServerEvents> {
        let mut events = ServerEvents::default();
        {
            let iter = net.poll().map_err(|_| ReplError::Transport {
                context: "server: poll failed",
            })?;
            for ev in iter {
                match ev {
                    NetEvent::Connected { client } => {
                        if self.client_mut(client).is_none() {
                            self.clients.push(ServerClient::new(client));
                        }
                        events.connected.push(client);
                    }
                    NetEvent::Disconnected { client, .. } => {
                        self.clients.retain(|c| c.client != client);
                        self.vis.remove_client(client);
                        events.disconnected.push(client);
                    }
                    NetEvent::Message { client, bytes, .. } => {
                        Self::on_message(
                            &mut self.clients,
                            world,
                            &self.ids,
                            client,
                            bytes,
                            &mut events,
                        )?;
                    }
                }
            }
        }

        self.sync_ids(world);
        self.vis.update(world, &self.ids);
        for ci in 0..self.clients.len() {
            self.send_snapshot(world, net, ci, now_tick)?;
        }
        Ok(events)
    }

    fn on_message(
        clients: &mut [ServerClient],
        world: &World,
        ids: &ReplIdMap,
        client: ClientId,
        bytes: &[u8],
        events: &mut ServerEvents,
    ) -> ReplResult<()> {
        let Some((&tag, rest)) = bytes.split_first() else {
            return Ok(());
        };
        match tag {
            TAG_INPUT => Self::on_input(clients, client, rest, events),
            TAG_RPC => Self::on_rpc(world, ids, client, rest, events),
            _ => Ok(()), // a snapshot tag is never inbound at the server
        }
    }

    fn on_input(
        clients: &mut [ServerClient],
        client: ClientId,
        rest: &[u8],
        events: &mut ServerEvents,
    ) -> ReplResult<()> {
        let mut r = BitReader::new(rest);
        let ack = if r.read_bool()? {
            Some(r.read_bits(32)? as u32)
        } else {
            None
        };
        let Some(sc) = clients.iter_mut().find(|c| c.client == client) else {
            return Ok(());
        };
        if let Some(t) = ack {
            sc.acked_tick = Some(t);
        }
        while r.read_bool()? {
            let seq = r.read_bits(16)? as u16;
            let len = r.read_bits(16)? as usize;
            let mut buf = vec![0u8; len];
            r.read_aligned(&mut buf)?;
            sc.last_input_seq = seq;
            events.inputs.push(ServerInput {
                client,
                seq,
                bytes: buf,
            });
        }
        Ok(())
    }

    fn on_rpc(
        world: &World,
        ids: &ReplIdMap,
        client: ClientId,
        rest: &[u8],
        events: &mut ServerEvents,
    ) -> ReplResult<()> {
        let header = decode_rpc_header(rest)?;
        if header.kind == RpcKind::Server {
            let owner = header
                .target
                .and_then(|t| ids.entity(t))
                .and_then(|e| world.get::<OwnerOnly>(e))
                .map(|o| o.0);
            server_rpc_authorized(client, owner)?;
        }
        events.rpcs.push(ServerRpc {
            client,
            header,
            bytes: rest.to_vec(),
        });
        Ok(())
    }

    fn sync_ids(&mut self, world: &World) {
        for e in world.query::<Entity>().with::<Replicated>().iter() {
            if self.ids.get(e).is_none() {
                self.ids.allocate(e);
            }
        }
        self.dead.clear();
        for (e, id) in self.ids.entities() {
            if !world.contains(e) {
                self.dead.push((e, id));
            }
        }
        for &(e, id) in &self.dead {
            self.ids.release(e);
            self.vis.remove_entity(id);
        }
    }

    fn send_snapshot(
        &mut self,
        world: &World,
        net: &mut Server,
        ci: usize,
        now_tick: u32,
    ) -> ReplResult<()> {
        let client = self.clients[ci].client;
        let spawns: Vec<ReplId> = self.vis.spawns(client).to_vec();
        let despawns: Vec<ReplId> = self.vis.despawns(client).to_vec();
        self.vis.visible_into(client, &mut self.visible);

        let mut updates: Vec<ReplId> = {
            let sc = &self.clients[ci];
            let mut u: Vec<ReplId> = self
                .visible
                .iter()
                .copied()
                .filter(|id| !spawns.contains(id))
                .collect();
            // Highest staleness (ticks since last sent) first — the priority accumulator.
            u.sort_by_key(|id| {
                std::cmp::Reverse(now_tick.wrapping_sub(*sc.last_sent.get(id).unwrap_or(&0)))
            });
            u
        };

        self.scratch.resize(1 + self.config.send_budget + 2048, 0);
        self.scratch[0] = TAG_SNAPSHOT;
        let acked_tick = self.clients[ci].acked_tick;
        let last_input_seq = self.clients[ci].last_input_seq;
        let (n, state, written) = {
            let baseline = acked_tick.and_then(|t| self.clients[ci].ring_get(t));
            encode_snapshot(
                &self.registry,
                world,
                &self.ids,
                &mut self.scratch[1..],
                now_tick,
                acked_tick.filter(|_| baseline.is_some()),
                baseline,
                last_input_seq,
                &spawns,
                &despawns,
                &updates,
                self.config.send_budget,
            )?
        };
        net.send(
            client,
            ChannelId::UnreliableSequenced,
            &self.scratch[..1 + n],
        )
        .map_err(|_| ReplError::Transport {
            context: "server: snapshot send failed",
        })?;

        self.clients[ci].ring_put(now_tick, state);
        let sc = &mut self.clients[ci];
        for id in &spawns {
            sc.last_sent.insert(*id, now_tick);
        }
        for id in updates.drain(..).take(written) {
            sc.last_sent.insert(id, now_tick);
        }
        Ok(())
    }
}
