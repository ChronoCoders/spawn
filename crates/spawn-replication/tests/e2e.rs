//! Headless end-to-end loopback convergence test: a server `World` with several
//! replicated entities and two connected clients with distinct viewers; after a number
//! of ticks each client's local `World` holds exactly the entities relevant to it
//! (interest-filtered), with matching component state — exercising id allocation,
//! interest management, delta-compressed snapshots over `spawn-net`, the snapshot-ack
//! loop, and the client apply path.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use spawn_core::{Transform3D, Vec3};
use spawn_ecs::{Component, World};
use spawn_net::{Client, ClientState, Server, ServerConfig};
use spawn_replication::{OwnerOnly, Replicated, ReplicationClient, ReplicationConfig, Replicator};
use spawn_serialize::{Serialize, SerializeResult, Stream};

/// A replicated position component (distinct from `Transform3D`, which the server uses
/// only for interest management). Sent bit-exact so the test can assert equality.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct NetPos {
    x: f32,
    y: f32,
}
impl Component for NetPos {}
impl Serialize for NetPos {
    fn serialize<S: Stream>(&mut self, s: &mut S) -> SerializeResult<()> {
        let mut bx = u64::from(self.x.to_bits());
        s.serialize_bits(&mut bx, 32)?;
        self.x = f32::from_bits(bx as u32);
        let mut by = u64::from(self.y.to_bits());
        s.serialize_bits(&mut by, 32)?;
        self.y = f32::from_bits(by as u32);
        Ok(())
    }
}
impl spawn_replication::Replicate for NetPos {
    fn replicate_name() -> &'static str {
        "NetPos"
    }
}

fn at(x: f32, z: f32) -> Transform3D {
    Transform3D::from_translation(Vec3::new(x, 0.0, z))
}

/// Collect every `NetPos` in a world.
fn positions(world: &World) -> Vec<NetPos> {
    world.query::<&NetPos>().iter().copied().collect()
}

#[test]
fn two_clients_converge_with_interest_filtering() {
    let saddr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let mut server_net = Server::bind(saddr, ServerConfig::default()).unwrap();
    let server_addr = server_net.local_addr().unwrap();
    let mut c1_net = Client::new().unwrap();
    let mut c2_net = Client::new().unwrap();
    c1_net.connect(server_addr).unwrap();
    c2_net.connect(server_addr).unwrap();

    let mut sworld = World::new();
    let mut repl = Replicator::new(ReplicationConfig {
        default_view_radius: 32.0,
        ..Default::default()
    });
    repl.registry_mut().register::<NetPos>(&mut sworld);

    for i in 1..=3u32 {
        sworld.spawn_with((
            at(i as f32, 0.0),
            NetPos {
                x: i as f32,
                y: 0.0,
            },
            Replicated,
        ));
    }

    let mut w1 = World::new();
    let mut cl1 = ReplicationClient::new(ReplicationConfig::default());
    cl1.registry_mut().register::<NetPos>(&mut w1);
    let mut w2 = World::new();
    let mut cl2 = ReplicationClient::new(ReplicationConfig::default());
    cl2.registry_mut().register::<NetPos>(&mut w2);

    let mut tick = 0u32;
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut viewers_set = false;

    while Instant::now() < deadline {
        let events = repl.tick(&mut sworld, &mut server_net, tick).unwrap();
        // When each client connects, give it a pawn near its view center and set the
        // viewer. Client 1 looks at the origin; client 2 looks far away.
        for client in events.connected {
            let (vx, vz, owner_x) = if client.0 % 2 == 1 {
                (0.0f32, 0.0f32, 50.0f32)
            } else {
                (1000.0, 1000.0, 1050.0)
            };
            let pawn = sworld.spawn_with((
                at(vx, vz),
                NetPos { x: owner_x, y: 0.0 },
                OwnerOnly(client),
                Replicated,
            ));
            repl.set_viewer(client, pawn, 32.0);
            viewers_set = true;
        }

        let _ = cl1.tick(&mut w1, &mut c1_net).unwrap();
        let _ = cl2.tick(&mut w2, &mut c2_net).unwrap();
        tick = tick.wrapping_add(1);

        // Stop once both connected, viewers are set, and client 1 has converged on the
        // three clustered entities (x = 1,2,3) plus its own pawn.
        if viewers_set
            && c1_net.state() == ClientState::Connected
            && c2_net.state() == ClientState::Connected
        {
            let p1 = positions(&w1);
            let near: Vec<f32> = p1.iter().map(|p| p.x).filter(|&x| x <= 3.5).collect();
            if near.len() == 3 {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    // Client 1 sees the three clustered world entities (x = 1,2,3) — interest-filtered
    // in — plus its own owner-only pawn (x = 50).
    let p1 = positions(&w1);
    let mut near1: Vec<f32> = p1.iter().map(|p| p.x).filter(|&x| x <= 3.5).collect();
    near1.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(
        near1,
        vec![1.0, 2.0, 3.0],
        "client 1 converged on the cluster"
    );
    assert!(
        p1.iter().any(|p| (p.x - 50.0).abs() < 1e-3),
        "client 1 sees its own owner-only pawn"
    );

    // Client 2's viewer is far from the cluster: it must NOT have any of the clustered
    // entities, only (eventually) its own pawn (x = 1050).
    let p2 = positions(&w2);
    assert!(
        !p2.iter().any(|p| p.x <= 3.5),
        "client 2 must not see the distant cluster (interest filtered): {p2:?}"
    );
}
