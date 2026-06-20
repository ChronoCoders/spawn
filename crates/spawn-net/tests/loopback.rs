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
    MAX_FRAGMENTED_PAYLOAD, MAX_PAYLOAD_SIZE,
};

/// A deterministic, patterned blob of `n` bytes for fragmentation round-trips.
fn blob(n: usize) -> Vec<u8> {
    (0..n)
        .map(|i| (i.wrapping_mul(31).wrapping_add(7)) as u8)
        .collect()
}

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

    let fragmented = vec![1u8; MAX_PAYLOAD_SIZE + 1];
    assert!(client.send(ChannelId::Unreliable, &fragmented).is_ok());

    let over = vec![1u8; MAX_FRAGMENTED_PAYLOAD + 1];
    let err = client.send(ChannelId::Unreliable, &over).unwrap_err();
    assert!(matches!(
        err,
        spawn_net::NetError::PayloadTooLarge {
            max: MAX_FRAGMENTED_PAYLOAD,
            ..
        }
    ));
}

#[test]
fn reliable_backpressure_then_recovers() {
    let mut server = server_on(4);
    let mut client = Client::new().unwrap();
    connect(&mut server, &mut client);

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
    let mut proxy = DropProxy::new(saddr, 3);
    let proxy_addr = proxy.front_addr();

    let mut client = Client::new().unwrap();
    client.connect(proxy_addr).unwrap();

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
    assert!(server_salt.is_some());

    write_header(&mut pkt, 2);
    let wrong = 0u64;
    pkt[HEADER_SIZE..HEADER_SIZE + 8].copy_from_slice(&wrong.to_le_bytes());
    sock.send_to(&pkt[..HEADER_SIZE + 8], saddr).unwrap();

    let mut denied = false;
    for _ in 0..50 {
        let _ = server.poll().unwrap().count();
        let mut rbuf = [0u8; 64];
        if let Ok((len, _)) = sock.recv_from(&mut rbuf) {
            if len > HEADER_SIZE && rbuf[4] == 4 {
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

    write_header(&mut pkt, 2);
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

    write_header(&mut pkt, 7);
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

/// Hand-roll a raw client through the handshake and return (server, socket, connect_salt).
/// Mirrors the setup in `disconnect_salt_is_validated` so KeepAlive validation can be
/// exercised against a real `Server` with full control over packet contents.
fn raw_connected(client_salt: u64) -> (Server, UdpSocket, u64) {
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

    write_header(&mut pkt, 2);
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
    (server, sock, connect_salt)
}

#[test]
fn spoofed_keepalive_does_not_refresh_timeout() {
    // A KeepAlive carrying the WRONG salt from the connected peer's address must be
    // ignored: it must not refresh the victim's timeout, so the connection still times
    // out on schedule (mirrors the Disconnect salt-validation guard, §5.2/§5.3).
    const HEADER_SIZE: usize = 14;
    const PROTOCOL_ID: u32 = 0x5350_4E31;

    let (mut server, sock, connect_salt) = raw_connected(0x0123_4567_89AB_CDEF);

    let mut pkt = [0u8; 64];
    let write_keepalive = |pkt: &mut [u8], salt: u64| {
        pkt[0..4].copy_from_slice(&PROTOCOL_ID.to_le_bytes());
        pkt[4] = 5;
        pkt[5..7].copy_from_slice(&0u16.to_le_bytes());
        pkt[7..9].copy_from_slice(&0u16.to_le_bytes());
        pkt[9..13].copy_from_slice(&0u32.to_le_bytes());
        pkt[13] = 0xFF;
        pkt[HEADER_SIZE..HEADER_SIZE + 8].copy_from_slice(&salt.to_le_bytes());
    };

    // Spam wrong-salt KeepAlives while waiting past CONNECTION_TIMEOUT. If the salt check
    // were missing, these would refresh `last_recv` and the connection would never drop.
    let deadline = Instant::now() + spawn_net::CONNECTION_TIMEOUT + Duration::from_secs(2);
    let mut timed_out = false;
    while Instant::now() < deadline {
        write_keepalive(&mut pkt, !connect_salt);
        let _ = sock.send_to(&pkt[..HEADER_SIZE + 8], server.local_addr().unwrap());
        for ev in server.poll().unwrap() {
            if let NetEvent::Disconnected { reason, .. } = ev {
                assert_eq!(reason, DisconnectReason::TimedOut);
                timed_out = true;
            }
        }
        if timed_out {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        timed_out,
        "spoofed-salt KeepAlive must not keep the connection alive"
    );
    assert_eq!(server.connected_clients(), 0);
}

#[test]
fn correct_keepalive_refreshes_timeout() {
    // The dual of the spoofed case: a correct-salt KeepAlive sent steadily keeps the
    // connection alive across what would otherwise be the timeout window.
    const HEADER_SIZE: usize = 14;
    const PROTOCOL_ID: u32 = 0x5350_4E31;

    let (mut server, sock, connect_salt) = raw_connected(0x0FED_CBA9_8765_4321);

    let mut pkt = [0u8; 64];
    let write_keepalive = |pkt: &mut [u8], salt: u64| {
        pkt[0..4].copy_from_slice(&PROTOCOL_ID.to_le_bytes());
        pkt[4] = 5;
        pkt[5..7].copy_from_slice(&0u16.to_le_bytes());
        pkt[7..9].copy_from_slice(&0u16.to_le_bytes());
        pkt[9..13].copy_from_slice(&0u32.to_le_bytes());
        pkt[13] = 0xFF;
        pkt[HEADER_SIZE..HEADER_SIZE + 8].copy_from_slice(&salt.to_le_bytes());
    };

    // Drive correct-salt KeepAlives steadily for longer than CONNECTION_TIMEOUT.
    let deadline = Instant::now() + spawn_net::CONNECTION_TIMEOUT + Duration::from_secs(1);
    while Instant::now() < deadline {
        write_keepalive(&mut pkt, connect_salt);
        let _ = sock.send_to(&pkt[..HEADER_SIZE + 8], server.local_addr().unwrap());
        for ev in server.poll().unwrap() {
            if let NetEvent::Disconnected { .. } = ev {
                panic!("correct-salt KeepAlive must keep the connection alive");
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        server.connected_clients(),
        1,
        "connection should remain alive under correct-salt KeepAlive"
    );
}

#[test]
fn fragmented_unreliable_delivers_intact() {
    let mut server = server_on(4);
    let mut client = Client::new().unwrap();
    connect(&mut server, &mut client);

    let msg = blob(3000);
    assert!(msg.len() > MAX_PAYLOAD_SIZE);
    server
        .broadcast(ChannelId::UnreliableSequenced, &msg)
        .unwrap();

    let mut got: Option<Vec<u8>> = None;
    pump(
        &mut server,
        &mut client,
        40,
        |_| {},
        |ev| {
            if let NetEvent::Message { channel, bytes, .. } = ev {
                assert_eq!(channel, ChannelId::UnreliableSequenced);
                got = Some(bytes.to_vec());
            }
        },
    );
    assert_eq!(
        got.as_deref(),
        Some(msg.as_slice()),
        "fragmented unreliable message reassembles intact over lossless loopback"
    );
}

#[test]
fn fragmented_reliable_delivers_intact() {
    let mut server = server_on(4);
    let mut client = Client::new().unwrap();
    connect(&mut server, &mut client);

    let msg = blob(5000);
    server.broadcast(ChannelId::ReliableOrdered, &msg).unwrap();

    let mut got: Option<Vec<u8>> = None;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let _ = server.poll().unwrap().count();
        for ev in client.poll().unwrap() {
            if let NetEvent::Message { channel, bytes, .. } = ev {
                assert_eq!(channel, ChannelId::ReliableOrdered);
                got = Some(bytes.to_vec());
            }
        }
        if got.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(got.as_deref(), Some(msg.as_slice()));
}

#[test]
fn fragmented_reliable_delivers_intact_under_drop() {
    let mut server = server_on(4);
    let saddr = server.local_addr().unwrap();
    let mut proxy = DropProxy::new(saddr, 3);
    let proxy_addr = proxy.front_addr();

    let mut client = Client::new().unwrap();
    client.connect(proxy_addr).unwrap();

    let mut connected = false;
    for _ in 0..300 {
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

    let msg = blob(6000);
    server.broadcast(ChannelId::ReliableOrdered, &msg).unwrap();

    let mut got: Option<Vec<u8>> = None;
    let mut deliveries = 0usize;
    let deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < deadline {
        proxy.pump();
        let _ = server.poll().unwrap().count();
        proxy.pump();
        for ev in client.poll().unwrap() {
            if let NetEvent::Message { channel, bytes, .. } = ev {
                assert_eq!(channel, ChannelId::ReliableOrdered);
                got = Some(bytes.to_vec());
                deliveries += 1;
            }
        }
        // Keep pumping a little past first delivery to prove no double-delivery from
        // the sender's continued resends of the same fragment id.
        if deliveries > 0 && Instant::now() + Duration::from_millis(400) < deadline {
            for _ in 0..50 {
                proxy.pump();
                let _ = server.poll().unwrap().count();
                proxy.pump();
                for ev in client.poll().unwrap() {
                    if let NetEvent::Message { .. } = ev {
                        deliveries += 1;
                    }
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        got.as_deref(),
        Some(msg.as_slice()),
        "reliable fragmented message arrives intact under loss"
    );
    assert_eq!(
        deliveries, 1,
        "reliable fragmented message delivered exactly once"
    );
}

#[test]
fn fragmented_backpressure_and_ceiling() {
    let mut server = server_on(4);
    let mut client = Client::new().unwrap();
    connect(&mut server, &mut client);

    // A second in-flight reliable fragmented message before the first completes is
    // refused with ChannelFull (single in-flight per connection).
    server
        .broadcast(ChannelId::ReliableOrdered, &blob(5000))
        .unwrap();
    let saddr_clients: Vec<_> = (0..16)
        .filter(|&i| server.stats(spawn_net::ClientId(i)).is_some())
        .map(spawn_net::ClientId)
        .collect();
    let cid = *saddr_clients.first().expect("one connected client");
    let err = server
        .send(cid, ChannelId::ReliableOrdered, &blob(5000))
        .unwrap_err();
    assert!(matches!(err, spawn_net::NetError::ChannelFull));

    let over = vec![0u8; MAX_FRAGMENTED_PAYLOAD + 1];
    let err = server.send(cid, ChannelId::Unreliable, &over).unwrap_err();
    assert!(matches!(
        err,
        spawn_net::NetError::PayloadTooLarge {
            max: MAX_FRAGMENTED_PAYLOAD,
            ..
        }
    ));
}
