//! Loopback integration tests driven by manual `poll` loops.
//!
//! Determinism: no real-time sleeps gate correctness. Where timeouts must elapse we
//! either rely on the engine's own short resend timeout (100 ms) bounded by a poll cap,
//! or busy-poll until a deadline. A `DropProxy` interposes on the wire to drop packets
//! deterministically (every Nth datagram) so reliable resend can be exercised without
//! flakiness.

use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use spawn_net::{
    ChannelId, Client, ClientState, DenyReason, DisconnectReason, NetEvent, Server, ServerConfig,
    MAX_PAYLOAD_SIZE,
};

fn server_on(max_clients: usize) -> Server {
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    Server::bind(
        addr,
        ServerConfig {
            max_clients,
            ..Default::default()
        },
    )
    .unwrap()
}

/// Drive both ends until `pred` returns true or `polls` is exhausted. Collects events
/// each side produces into the provided closures.
fn pump<FS, FC>(
    server: &mut Server,
    client: &mut Client,
    polls: usize,
    mut on_server: FS,
    mut on_client: FC,
) where
    FS: FnMut(NetEvent<'_>),
    FC: FnMut(NetEvent<'_>),
{
    for _ in 0..polls {
        for ev in server.poll().unwrap() {
            on_server(ev);
        }
        for ev in client.poll().unwrap() {
            on_client(ev);
        }
    }
}

#[test]
fn handshake_completes_and_assigns_client_id() {
    let mut server = server_on(4);
    let saddr = server.local_addr().unwrap();
    let mut client = Client::new().unwrap();
    client.connect(saddr).unwrap();

    let mut got_id = None;
    let mut server_connected = false;
    for _ in 0..50 {
        for ev in server.poll().unwrap() {
            if let NetEvent::Connected { .. } = ev {
                server_connected = true;
            }
        }
        for ev in client.poll().unwrap() {
            if let NetEvent::Connected { client } = ev {
                got_id = Some(client);
            }
        }
        if got_id.is_some() && server_connected {
            break;
        }
    }
    assert_eq!(client.state(), ClientState::Connected);
    assert!(server_connected);
    assert!(got_id.is_some());
    assert_eq!(server.connected_clients(), 1);
}

#[test]
fn server_full_denial() {
    let mut server = server_on(1);
    let saddr = server.local_addr().unwrap();

    let mut a = Client::new().unwrap();
    a.connect(saddr).unwrap();
    // Connect first client fully.
    for _ in 0..50 {
        let _ = server.poll().unwrap().count();
        let _ = a.poll().unwrap().count();
        if a.state() == ClientState::Connected {
            break;
        }
    }
    assert_eq!(a.state(), ClientState::Connected);

    let mut b = Client::new().unwrap();
    b.connect(saddr).unwrap();
    let mut denied = None;
    for _ in 0..50 {
        let _ = server.poll().unwrap().count();
        for ev in b.poll().unwrap() {
            if let NetEvent::Disconnected { reason, .. } = ev {
                denied = Some(reason);
            }
        }
        if denied.is_some() {
            break;
        }
    }
    assert_eq!(
        denied,
        Some(DisconnectReason::Denied(DenyReason::ServerFull))
    );
    assert_eq!(b.state(), ClientState::Disconnected);
}

fn connect(server: &mut Server, client: &mut Client) {
    let saddr = server.local_addr().unwrap();
    client.connect(saddr).unwrap();
    for _ in 0..50 {
        let _ = server.poll().unwrap().count();
        let _ = client.poll().unwrap().count();
        if client.state() == ClientState::Connected {
            return;
        }
    }
    panic!("handshake did not complete");
}

#[test]
fn reliable_ordered_delivery() {
    let mut server = server_on(4);
    let mut client = Client::new().unwrap();
    connect(&mut server, &mut client);

    for i in 0u32..20 {
        client
            .send(ChannelId::ReliableOrdered, &i.to_le_bytes())
            .unwrap();
    }

    let mut received: Vec<u32> = Vec::new();
    pump(
        &mut server,
        &mut client,
        30,
        |ev| {
            if let NetEvent::Message { channel, bytes, .. } = ev {
                assert_eq!(channel, ChannelId::ReliableOrdered);
                received.push(u32::from_le_bytes(bytes.try_into().unwrap()));
            }
        },
        |_| {},
    );
    assert_eq!(received, (0u32..20).collect::<Vec<_>>());
}

#[test]
fn unreliable_sequenced_latest_wins() {
    // Drive the receive filter directly via two connected ends; reorder is induced by
    // sending several frames and confirming the receiver never surfaces an older one
    // after a newer. With loopback there is no real reordering, so assert monotonic
    // non-decreasing delivery (no stale frames slip through).
    let mut server = server_on(4);
    let mut client = Client::new().unwrap();
    connect(&mut server, &mut client);

    for i in 0u32..10 {
        client
            .send(ChannelId::UnreliableSequenced, &i.to_le_bytes())
            .unwrap();
    }
    let mut last: Option<u32> = None;
    pump(
        &mut server,
        &mut client,
        20,
        |ev| {
            if let NetEvent::Message { bytes, channel, .. } = ev {
                assert_eq!(channel, ChannelId::UnreliableSequenced);
                let v = u32::from_le_bytes(bytes.try_into().unwrap());
                if let Some(p) = last {
                    assert!(v > p, "stale frame {v} after {p}");
                }
                last = Some(v);
            }
        },
        |_| {},
    );
    assert!(last.is_some());
}

#[test]
fn oversize_payload_rejected() {
    let mut server = server_on(4);
    let mut client = Client::new().unwrap();
    connect(&mut server, &mut client);

    let max = vec![1u8; MAX_PAYLOAD_SIZE];
    assert!(client.send(ChannelId::Unreliable, &max).is_ok());

    let over = vec![1u8; MAX_PAYLOAD_SIZE + 1];
    let err = client.send(ChannelId::Unreliable, &over).unwrap_err();
    assert!(matches!(
        err,
        spawn_net::NetError::PayloadTooLarge {
            max: MAX_PAYLOAD_SIZE,
            ..
        }
    ));
}

#[test]
fn reliable_backpressure_then_recovers() {
    let mut server = server_on(4);
    let mut client = Client::new().unwrap();
    connect(&mut server, &mut client);

    // Fill the window without polling (no flush yet => nothing acked).
    let window = spawn_net::RELIABLE_SEND_WINDOW;
    for _ in 0..window {
        client.send(ChannelId::ReliableOrdered, b"x").unwrap();
    }
    let err = client.send(ChannelId::ReliableOrdered, b"y").unwrap_err();
    assert!(matches!(err, spawn_net::NetError::ChannelFull));

    // Pump until acks drain the window. Resends are time-gated (100 ms) and the client
    // must poll to ingest the server's acks, so allow wall time to pass and keep retrying
    // the previously-rejected send until it succeeds — proving the window freed via acks
    // with no message dropped (all `window` messages are still delivered exactly once).
    let mut delivered = 0usize;
    let mut freed = false;
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        for ev in server.poll().unwrap() {
            if let NetEvent::Message { .. } = ev {
                delivered += 1;
            }
        }
        let _ = client.poll().unwrap().count();
        if !freed && client.send(ChannelId::ReliableOrdered, b"y").is_ok() {
            freed = true;
        }
        if freed && delivered >= window {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(freed, "reliable window never freed via acks");
    assert!(delivered >= window, "delivered {delivered} of {window}");
}

#[test]
fn graceful_disconnect_emits_disconnected() {
    let mut server = server_on(4);
    let mut client = Client::new().unwrap();
    connect(&mut server, &mut client);
    let saddr = server.local_addr().unwrap();
    let _ = saddr;

    client.disconnect().unwrap();
    let mut reason = None;
    for _ in 0..40 {
        for ev in server.poll().unwrap() {
            if let NetEvent::Disconnected { reason: r, .. } = ev {
                reason = Some(r);
            }
        }
        let _ = client.poll().unwrap().count();
        if reason.is_some() {
            break;
        }
    }
    assert_eq!(reason, Some(DisconnectReason::Disconnected));
    assert_eq!(server.connected_clients(), 0);
}

#[test]
fn timeout_disconnect() {
    let mut server = server_on(4);
    let mut client = Client::new().unwrap();
    connect(&mut server, &mut client);

    // Stop polling the client; advance wall time past CONNECTION_TIMEOUT while the
    // server keeps polling. Busy-poll bounded by a deadline (no fixed sleep race).
    let deadline = Instant::now() + spawn_net::CONNECTION_TIMEOUT + Duration::from_secs(2);
    let mut reason = None;
    while Instant::now() < deadline {
        for ev in server.poll().unwrap() {
            if let NetEvent::Disconnected { reason: r, .. } = ev {
                reason = Some(r);
            }
        }
        if reason.is_some() {
            break;
        }
        std::thread::yield_now();
    }
    assert_eq!(reason, Some(DisconnectReason::TimedOut));
}

/// Deterministic on-path drop filter: forwards datagrams between client and server but
/// drops every `drop_every`-th datagram in each direction. Driven inside the poll loop.
struct DropProxy {
    front: UdpSocket, // faces the client
    back: UdpSocket,  // faces the server
    server_addr: SocketAddr,
    client_addr: Option<SocketAddr>,
    counter_fwd: usize,
    counter_bwd: usize,
    drop_every: usize,
    buf: [u8; 2048],
}

impl DropProxy {
    fn new(server_addr: SocketAddr, drop_every: usize) -> Self {
        let front = UdpSocket::bind("127.0.0.1:0").unwrap();
        let back = UdpSocket::bind("127.0.0.1:0").unwrap();
        front.set_nonblocking(true).unwrap();
        back.set_nonblocking(true).unwrap();
        Self {
            front,
            back,
            server_addr,
            client_addr: None,
            counter_fwd: 0,
            counter_bwd: 0,
            drop_every,
            buf: [0u8; 2048],
        }
    }

    fn front_addr(&self) -> SocketAddr {
        self.front.local_addr().unwrap()
    }

    fn pump(&mut self) {
        // Client -> server.
        loop {
            match self.front.recv_from(&mut self.buf) {
                Ok((len, from)) => {
                    self.client_addr = Some(from);
                    self.counter_fwd += 1;
                    if !self.counter_fwd.is_multiple_of(self.drop_every) {
                        let _ = self.back.send_to(&self.buf[..len], self.server_addr);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
        // Server -> client.
        loop {
            match self.back.recv_from(&mut self.buf) {
                Ok((len, _from)) => {
                    self.counter_bwd += 1;
                    if !self.counter_bwd.is_multiple_of(self.drop_every) {
                        if let Some(c) = self.client_addr {
                            let _ = self.front.send_to(&self.buf[..len], c);
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
    }
}

#[test]
fn reliable_delivery_under_simulated_drop() {
    let mut server = server_on(4);
    let saddr = server.local_addr().unwrap();
    let mut proxy = DropProxy::new(saddr, 3); // drop every 3rd datagram each way
    let proxy_addr = proxy.front_addr();

    let mut client = Client::new().unwrap();
    client.connect(proxy_addr).unwrap();

    // Handshake through the lossy proxy.
    let mut connected = false;
    for _ in 0..200 {
        proxy.pump();
        let _ = server.poll().unwrap().count();
        proxy.pump();
        for ev in client.poll().unwrap() {
            if let NetEvent::Connected { .. } = ev {
                connected = true;
            }
        }
        if connected {
            break;
        }
    }
    assert!(connected, "handshake failed under drop");

    for i in 0u32..50 {
        client
            .send(ChannelId::ReliableOrdered, &i.to_le_bytes())
            .unwrap();
    }

    let mut received: Vec<u32> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        proxy.pump();
        for ev in server.poll().unwrap() {
            if let NetEvent::Message { channel, bytes, .. } = ev {
                assert_eq!(channel, ChannelId::ReliableOrdered);
                received.push(u32::from_le_bytes(bytes.try_into().unwrap()));
            }
        }
        proxy.pump();
        let _ = client.poll().unwrap().count();
        if received.len() == 50 {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(received, (0u32..50).collect::<Vec<_>>());

    // Loss detection: with a 1-in-3 drop filter, retransmits reclaimed packet-sequence
    // slots whose originals were never acked, so the loss EWMA must be strictly positive.
    let server_loss = (0..16)
        .find_map(|i| server.stats(spawn_net::ClientId(i)).map(|s| s.packet_loss))
        .unwrap_or(0.0);
    let client_loss = client.stats().packet_loss;
    assert!(
        server_loss > 0.0 || client_loss > 0.0,
        "expected nonzero packet_loss under drops: server={server_loss} client={client_loss}"
    );
}

#[test]
fn invalid_challenge_response_denied() {
    // Hand-roll a client that sends a wrong ChallengeResponse and assert ConnectDenied.
    use std::net::UdpSocket;
    const HEADER_SIZE: usize = 14;
    const PROTOCOL_ID: u32 = 0x5350_4E31;

    let mut server = server_on(4);
    let saddr = server.local_addr().unwrap();
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();

    let mut pkt = [0u8; 64];
    let write_header = |pkt: &mut [u8], ty: u8| {
        pkt[0..4].copy_from_slice(&PROTOCOL_ID.to_le_bytes());
        pkt[4] = ty;
        pkt[5..7].copy_from_slice(&0u16.to_le_bytes());
        pkt[7..9].copy_from_slice(&0u16.to_le_bytes());
        pkt[9..13].copy_from_slice(&0u32.to_le_bytes());
        pkt[13] = 0xFF;
    };

    let client_salt: u64 = 0xDEAD_BEEF_CAFE_F00D;
    write_header(&mut pkt, 0); // ConnectRequest
    pkt[HEADER_SIZE..HEADER_SIZE + 8].copy_from_slice(&client_salt.to_le_bytes());
    sock.send_to(&pkt[..HEADER_SIZE + 8], saddr).unwrap();

    // Pump server to produce a Challenge.
    let mut server_salt = None;
    for _ in 0..50 {
        let _ = server.poll().unwrap().count();
        let mut rbuf = [0u8; 64];
        if let Ok((len, _)) = sock.recv_from(&mut rbuf) {
            if len >= HEADER_SIZE + 16 && rbuf[4] == 1 {
                let mut b = [0u8; 8];
                b.copy_from_slice(&rbuf[HEADER_SIZE + 8..HEADER_SIZE + 16]);
                server_salt = Some(u64::from_le_bytes(b));
                break;
            }
        }
        std::thread::yield_now();
    }
    assert!(server_salt.is_some());

    // Send a WRONG connect_salt.
    write_header(&mut pkt, 2); // ChallengeResponse
    let wrong = 0u64;
    pkt[HEADER_SIZE..HEADER_SIZE + 8].copy_from_slice(&wrong.to_le_bytes());
    sock.send_to(&pkt[..HEADER_SIZE + 8], saddr).unwrap();

    let mut denied = false;
    for _ in 0..50 {
        let _ = server.poll().unwrap().count();
        let mut rbuf = [0u8; 64];
        if let Ok((len, _)) = sock.recv_from(&mut rbuf) {
            if len > HEADER_SIZE && rbuf[4] == 4 {
                // ConnectDenied with InvalidResponse (1).
                assert_eq!(rbuf[HEADER_SIZE], 1);
                denied = true;
                break;
            }
        }
        std::thread::yield_now();
    }
    assert!(denied, "expected ConnectDenied(InvalidResponse)");
    assert_eq!(server.connected_clients(), 0);
}

#[test]
fn disconnect_salt_is_validated() {
    // Hand-roll a client so we control its source address: a wrong-salt Disconnect from
    // the connected peer's address must be ignored; the correct salt tears down (§5.3).
    const HEADER_SIZE: usize = 14;
    const PROTOCOL_ID: u32 = 0x5350_4E31;

    let mut server = server_on(4);
    let saddr = server.local_addr().unwrap();
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();

    let mut pkt = [0u8; 64];
    let write_header = |pkt: &mut [u8], ty: u8| {
        pkt[0..4].copy_from_slice(&PROTOCOL_ID.to_le_bytes());
        pkt[4] = ty;
        pkt[5..7].copy_from_slice(&0u16.to_le_bytes());
        pkt[7..9].copy_from_slice(&0u16.to_le_bytes());
        pkt[9..13].copy_from_slice(&0u32.to_le_bytes());
        pkt[13] = 0xFF;
    };

    // Handshake: ConnectRequest -> Challenge -> ChallengeResponse(correct) -> Accepted.
    let client_salt: u64 = 0x0123_4567_89AB_CDEF;
    write_header(&mut pkt, 0);
    pkt[HEADER_SIZE..HEADER_SIZE + 8].copy_from_slice(&client_salt.to_le_bytes());
    sock.send_to(&pkt[..HEADER_SIZE + 8], saddr).unwrap();

    let mut server_salt = None;
    for _ in 0..50 {
        let _ = server.poll().unwrap().count();
        let mut rbuf = [0u8; 64];
        if let Ok((len, _)) = sock.recv_from(&mut rbuf) {
            if len >= HEADER_SIZE + 16 && rbuf[4] == 1 {
                let mut b = [0u8; 8];
                b.copy_from_slice(&rbuf[HEADER_SIZE + 8..HEADER_SIZE + 16]);
                server_salt = Some(u64::from_le_bytes(b));
                break;
            }
        }
        std::thread::yield_now();
    }
    let server_salt = server_salt.expect("no Challenge");
    let connect_salt = client_salt ^ server_salt;

    write_header(&mut pkt, 2); // ChallengeResponse
    pkt[HEADER_SIZE..HEADER_SIZE + 8].copy_from_slice(&connect_salt.to_le_bytes());
    sock.send_to(&pkt[..HEADER_SIZE + 8], saddr).unwrap();

    for _ in 0..50 {
        let _ = server.poll().unwrap().count();
        if server.connected_clients() == 1 {
            break;
        }
        std::thread::yield_now();
    }
    assert_eq!(server.connected_clients(), 1, "handshake did not complete");

    // Wrong-salt Disconnect from the same source: must be ignored.
    write_header(&mut pkt, 7); // Disconnect
    pkt[HEADER_SIZE..HEADER_SIZE + 8].copy_from_slice(&(!connect_salt).to_le_bytes());
    sock.send_to(&pkt[..HEADER_SIZE + 8], saddr).unwrap();
    for _ in 0..20 {
        let _ = server.poll().unwrap().count();
    }
    assert_eq!(
        server.connected_clients(),
        1,
        "spoofed-salt Disconnect must not tear down the connection"
    );

    // Correct-salt Disconnect: tears the connection down.
    write_header(&mut pkt, 7);
    pkt[HEADER_SIZE..HEADER_SIZE + 8].copy_from_slice(&connect_salt.to_le_bytes());
    sock.send_to(&pkt[..HEADER_SIZE + 8], saddr).unwrap();
    for _ in 0..20 {
        let _ = server.poll().unwrap().count();
        if server.connected_clients() == 0 {
            break;
        }
    }
    assert_eq!(
        server.connected_clients(),
        0,
        "correct salt must disconnect"
    );
}
