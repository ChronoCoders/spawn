//! The client-side replication driver. Applies inbound snapshots into the local
//! `World` (delta-decoded against its baseline ring), and each tick sends an input
//! packet carrying the snapshot ack plus the redundant input window. Surfaces inbound
//! RPCs and the server's last-processed input sequence (for reconciliation).

use std::collections::VecDeque;

use spawn_ecs::World;
use spawn_net::{ChannelId, Client, ClientState, NetEvent};
use spawn_serialize::{BitWriter, Serialize};

use crate::config::ReplicationConfig;
use crate::error::{ReplError, ReplResult};
use crate::id::ReplIdMap;
use crate::registry::ReplicationRegistry;
use crate::rpc::{decode_rpc_header, encode_rpc, Rpc, RpcHeader, RpcRegistry};
use crate::snapshot::{decode_snapshot, peek_snapshot_header, SnapshotState, SNAPSHOT_HISTORY};
use crate::wire::{TAG_INPUT, TAG_RPC, TAG_SNAPSHOT};

/// Redundant input window: the last N inputs are resent every packet so a single loss
/// self-heals without head-of-line latency.
const INPUT_WINDOW: usize = 8;

/// One inbound RPC (the game decodes the payload via its registry).
pub struct ClientRpc {
    /// The routing header.
    pub header: RpcHeader,
    /// The full RPC bytes (header + payload).
    pub bytes: Vec<u8>,
}

/// What a client [`tick`](ReplicationClient::tick) surfaced this tick.
#[derive(Default)]
pub struct ClientEvents {
    /// Inbound RPCs (`Client`/`Multicast`).
    pub rpcs: Vec<ClientRpc>,
    /// The snapshot tick applied this poll, if any.
    pub applied_tick: Option<u32>,
    /// The server's last-processed input sequence (reconciliation ack), if a snapshot
    /// was applied.
    pub last_input_seq: Option<u16>,
}

struct RingSlot {
    tick: u32,
    state: SnapshotState,
    used: bool,
}

/// The client replication driver.
pub struct ReplicationClient {
    registry: ReplicationRegistry,
    rpcs: RpcRegistry,
    ids: ReplIdMap,
    ring: Vec<RingSlot>,
    last_received_tick: Option<u32>,
    input_window: VecDeque<(u16, Vec<u8>)>,
    next_input_seq: u16,
    config: ReplicationConfig,
    scratch: Vec<u8>,
}

impl ReplicationClient {
    /// A new client driver.
    pub fn new(config: ReplicationConfig) -> Self {
        let mut ring = Vec::with_capacity(SNAPSHOT_HISTORY);
        for _ in 0..SNAPSHOT_HISTORY {
            ring.push(RingSlot {
                tick: 0,
                state: SnapshotState::default(),
                used: false,
            });
        }
        Self {
            registry: ReplicationRegistry::new(),
            rpcs: RpcRegistry::new(),
            ids: ReplIdMap::new(),
            ring,
            last_received_tick: None,
            input_window: VecDeque::with_capacity(INPUT_WINDOW),
            next_input_seq: 0,
            config,
            scratch: Vec::new(),
        }
    }

    /// The component registry — the game registers the same components, in the same
    /// order, as the server (the manifest agreement).
    pub fn registry_mut(&mut self) -> &mut ReplicationRegistry {
        &mut self.registry
    }

    /// The RPC registry.
    pub fn rpcs_mut(&mut self) -> &mut RpcRegistry {
        &mut self.rpcs
    }

    /// The local `ReplId` ↔ `Entity` map (for the game to resolve replicated entities,
    /// e.g. to feed interpolation or read the predicted pawn).
    pub fn ids(&self) -> &ReplIdMap {
        &self.ids
    }

    /// Record an input to send (and resend in the redundant window), returning its
    /// sequence. The game also applies it locally (prediction) and keeps its own copy
    /// for reconciliation replay.
    pub fn push_input<I: Serialize>(&mut self, input: &mut I) -> ReplResult<u16> {
        self.scratch.resize(self.config.send_budget.max(256), 0);
        let mut bw = BitWriter::new(&mut self.scratch);
        input.serialize(&mut bw)?;
        let n = bw.finish();
        let seq = self.next_input_seq;
        self.next_input_seq = self.next_input_seq.wrapping_add(1);
        self.input_window
            .push_back((seq, self.scratch[..n].to_vec()));
        while self.input_window.len() > INPUT_WINDOW {
            self.input_window.pop_front();
        }
        Ok(seq)
    }

    /// Send a client-originated `Server` RPC over the reliable channel.
    pub fn send_rpc<P: Rpc>(
        &mut self,
        net: &mut Client,
        header: RpcHeader,
        payload: &mut P,
    ) -> ReplResult<()> {
        self.scratch.resize(self.config.send_budget + 1024, 0);
        self.scratch[0] = TAG_RPC;
        let n = encode_rpc(&mut self.scratch[1..], header, payload)?;
        net.send(ChannelId::ReliableOrdered, &self.scratch[..1 + n])
            .map_err(|_| ReplError::Transport {
                context: "client: rpc send failed",
            })
    }

    fn ring_put(&mut self, tick: u32, state: SnapshotState) {
        self.ring[tick as usize % SNAPSHOT_HISTORY] = RingSlot {
            tick,
            state,
            used: true,
        };
    }

    /// Advance one tick: drain `net`, applying inbound snapshots/RPCs to `world`, then
    /// send the input packet (snapshot ack + redundant input window).
    pub fn tick(&mut self, world: &mut World, net: &mut Client) -> ReplResult<ClientEvents> {
        let mut events = ClientEvents::default();
        {
            let iter = net.poll().map_err(|_| ReplError::Transport {
                context: "client: poll failed",
            })?;
            for ev in iter {
                if let NetEvent::Message { bytes, .. } = ev {
                    self.on_message(world, bytes, &mut events)?;
                }
            }
        }
        // Only send once the handshake has completed.
        if net.state() == ClientState::Connected {
            self.send_input(net)?;
        }
        Ok(events)
    }

    fn on_message(
        &mut self,
        world: &mut World,
        bytes: &[u8],
        events: &mut ClientEvents,
    ) -> ReplResult<()> {
        let Some((&tag, rest)) = bytes.split_first() else {
            return Ok(());
        };
        match tag {
            TAG_SNAPSHOT => self.on_snapshot(world, rest, events),
            TAG_RPC => {
                let header = decode_rpc_header(rest)?;
                events.rpcs.push(ClientRpc {
                    header,
                    bytes: rest.to_vec(),
                });
                Ok(())
            }
            _ => Ok(()), // an input tag is never inbound at the client
        }
    }

    fn on_snapshot(
        &mut self,
        world: &mut World,
        rest: &[u8],
        events: &mut ClientEvents,
    ) -> ReplResult<()> {
        let (_tick, baseline_tick) = peek_snapshot_header(rest)?;
        let outcome = {
            // Inline the ring lookup (a direct `self.ring` field borrow) so it does not
            // borrow all of `self`, leaving `&mut self.ids` free for the decode.
            let baseline = baseline_tick.and_then(|t| {
                let slot = &self.ring[t as usize % SNAPSHOT_HISTORY];
                (slot.used && slot.tick == t).then_some(&slot.state)
            });
            decode_snapshot(&self.registry, world, &mut self.ids, rest, baseline)?
        };
        self.ring_put(outcome.tick, outcome.state);
        self.last_received_tick = Some(outcome.tick);
        events.applied_tick = Some(outcome.tick);
        events.last_input_seq = Some(outcome.last_input_seq);
        Ok(())
    }

    fn send_input(&mut self, net: &mut Client) -> ReplResult<()> {
        self.scratch.resize(self.config.send_budget + 1024, 0);
        let n = {
            let mut bw = BitWriter::new(&mut self.scratch[1..]);
            match self.last_received_tick {
                Some(t) => {
                    bw.write_bool(true)?;
                    bw.write_bits(u64::from(t), 32)?;
                }
                None => bw.write_bool(false)?,
            }
            for (seq, bytes) in &self.input_window {
                bw.write_bool(true)?;
                bw.write_bits(u64::from(*seq), 16)?;
                bw.write_bits(bytes.len() as u64, 16)?;
                bw.write_aligned(bytes)?;
            }
            bw.write_bool(false)?;
            bw.finish()
        };
        self.scratch[0] = TAG_INPUT;
        net.send(ChannelId::UnreliableSequenced, &self.scratch[..1 + n])
            .map_err(|_| ReplError::Transport {
                context: "client: input send failed",
            })
    }
}
