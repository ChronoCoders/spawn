//! UDP server: bind, slot management, handshake, poll-driven events.

use std::net::{SocketAddr, UdpSocket};
use std::time::Instant;

use crate::channel::ChannelId;
use crate::connection::{
    seed_from_env, Connection, DenyReason, DisconnectReason, Incoming, RawSend, SaltRng,
    DISCONNECT_REDUNDANCY, HANDSHAKE_TIMEOUT,
};
use crate::error::{NetError, NetResult};
use crate::event::{ClientId, EventRecord, NetEventIter};
use crate::protocol::{
    control_layout, PacketHeader, PacketType, HEADER_SIZE, MAX_PACKET_SIZE, MAX_PAYLOAD_SIZE,
    PROTOCOL_ID,
};
use crate::stats::ConnectionStats;

/// Server configuration.
pub struct ServerConfig {
    pub max_clients: usize,
    pub protocol_id: u32,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            max_clients: 32,
            protocol_id: PROTOCOL_ID,
        }
    }
}

struct Pending {
    addr: SocketAddr,
    client_salt: u64,
    server_salt: u64,
    started: Instant,
}

enum Slot {
    Free,
    Connected(Box<Connection>),
}

struct Closing {
    addr: SocketAddr,
    connect_salt: u64,
    remaining: u32,
}

/// Authoritative transport server. Owns a non-blocking socket and `max_clients` slots,
/// runs the salt handshake (§5.1), and emits a borrowed event stream per `poll`.
pub struct Server {
    socket: UdpSocket,
    config: ServerConfig,
    slots: Vec<Slot>,
    pending: Vec<Pending>,
    closing: Vec<Closing>,
    next_client_id: u32,
    rng: SaltRng,
    recv_buf: Vec<u8>,
    send_scratch: Box<[u8; MAX_PACKET_SIZE]>,
    events: Vec<EventRecord>,
    arena: Vec<u8>,
}

/// Wraps the socket plus destination for `Connection`'s socket-agnostic transmit path.
struct SocketSink<'a> {
    socket: &'a UdpSocket,
}

impl RawSend for SocketSink<'_> {
    fn raw_send(&mut self, bytes: &[u8], addr: SocketAddr) -> NetResult<()> {
        match self.socket.send_to(bytes, addr) {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
            Err(e) => Err(NetError::Io(e)),
        }
    }
}

impl Server {
    /// Bind a non-blocking UDP socket and prepare `max_clients` connection slots.
    pub fn bind(addr: SocketAddr, config: ServerConfig) -> NetResult<Self> {
        let socket = UdpSocket::bind(addr)?;
        socket.set_nonblocking(true)?;
        let mut slots = Vec::with_capacity(config.max_clients);
        for _ in 0..config.max_clients {
            slots.push(Slot::Free);
        }
        let disc = addr.port() as u64;
        Ok(Self {
            socket,
            config,
            slots,
            pending: Vec::new(),
            closing: Vec::new(),
            next_client_id: 1,
            rng: SaltRng::new(seed_from_env(disc)),
            recv_buf: vec![0u8; MAX_PACKET_SIZE],
            send_scratch: Box::new([0u8; MAX_PACKET_SIZE]),
            events: Vec::new(),
            arena: Vec::with_capacity(MAX_PACKET_SIZE),
        })
    }

    /// Drain the socket, advance handshakes/timers/resends, and return this poll's events.
    /// Returned events borrow internal buffers and are valid until the next `poll`.
    pub fn poll(&mut self) -> NetResult<NetEventIter<'_>> {
        self.events.clear();
        self.arena.clear();
        let now = Instant::now();

        self.drain_socket(now)?;
        self.expire_pending(now);
        self.expire_connections(now);
        self.flush_connections(now)?;
        self.advance_closing(now)?;

        Ok(NetEventIter::new(&self.events, &self.arena))
    }

    fn drain_socket(&mut self, now: Instant) -> NetResult<()> {
        loop {
            let (len, from) = match self.socket.recv_from(&mut self.recv_buf) {
                Ok(v) => v,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(NetError::Io(e)),
            };
            if len < HEADER_SIZE {
                continue;
            }
            let header = match PacketHeader::decode(&self.recv_buf[..len]) {
                Ok(h) => h,
                Err(_) => continue,
            };
            if header.protocol_id != self.config.protocol_id {
                continue;
            }
            self.handle_packet(header, from, len, now)?;
        }
        Ok(())
    }

    fn handle_packet(
        &mut self,
        header: PacketHeader,
        from: SocketAddr,
        len: usize,
        now: Instant,
    ) -> NetResult<()> {
        let payload_start = HEADER_SIZE;
        match header.packet_type {
            PacketType::ConnectRequest => self.on_connect_request(from, len, now)?,
            PacketType::ChallengeResponse => self.on_challenge_response(from, len, now)?,
            PacketType::Payload => {
                self.on_connected_packet(header, from, len, payload_start, now, false)?
            }
            PacketType::KeepAlive => {
                self.on_connected_packet(header, from, len, payload_start, now, true)?
            }
            PacketType::Disconnect => self.on_disconnect_packet(from, len),
            // Server never receives Challenge/ConnectAccepted/ConnectDenied.
            _ => {}
        }
        Ok(())
    }

    fn slot_index_of(&self, addr: SocketAddr) -> Option<usize> {
        self.slots.iter().position(|s| match s {
            Slot::Connected(c) => c.addr == addr,
            Slot::Free => false,
        })
    }

    fn free_slot(&self) -> Option<usize> {
        self.slots.iter().position(|s| matches!(s, Slot::Free))
    }

    fn on_connect_request(&mut self, from: SocketAddr, len: usize, now: Instant) -> NetResult<()> {
        if len < HEADER_SIZE + control_layout::CONNECT_REQUEST_LEN {
            return Ok(());
        }
        // Already connected from this address: ignore duplicate request.
        if self.slot_index_of(from).is_some() {
            return Ok(());
        }
        let client_salt = read_u64(&self.recv_buf[HEADER_SIZE + control_layout::SALT_OFFSET..]);

        if self.free_slot().is_none() {
            return self.send_denied(from, DenyReason::ServerFull);
        }

        let server_salt = match self.pending.iter().find(|p| p.addr == from) {
            Some(p) => p.server_salt,
            None => {
                let s = self.rng.next_u64();
                self.pending.push(Pending {
                    addr: from,
                    client_salt,
                    server_salt: s,
                    started: now,
                });
                s
            }
        };
        self.send_challenge(from, client_salt, server_salt)
    }

    fn on_challenge_response(
        &mut self,
        from: SocketAddr,
        len: usize,
        now: Instant,
    ) -> NetResult<()> {
        if len < HEADER_SIZE + control_layout::CHALLENGE_RESPONSE_LEN {
            return Ok(());
        }
        // Idempotent: a resent response from an already-connected peer is harmless.
        if self.slot_index_of(from).is_some() {
            return Ok(());
        }
        let connect_salt = read_u64(&self.recv_buf[HEADER_SIZE + control_layout::SALT_OFFSET..]);
        let Some(pos) = self.pending.iter().position(|p| p.addr == from) else {
            return Ok(());
        };
        let pend = &self.pending[pos];
        let expected = pend.client_salt ^ pend.server_salt;
        if connect_salt != expected {
            self.pending.swap_remove(pos);
            return self.send_denied(from, DenyReason::InvalidResponse);
        }
        let Some(slot) = self.free_slot() else {
            self.pending.swap_remove(pos);
            return self.send_denied(from, DenyReason::ServerFull);
        };
        self.pending.swap_remove(pos);

        let client_id = ClientId(self.next_client_id);
        self.next_client_id = self.next_client_id.wrapping_add(1);
        let conn = Box::new(Connection::new(from, client_id, connect_salt, now));
        self.slots[slot] = Slot::Connected(conn);
        self.send_accepted(from, connect_salt, client_id)?;
        self.events
            .push(EventRecord::Connected { client: client_id });
        Ok(())
    }

    fn on_connected_packet(
        &mut self,
        header: PacketHeader,
        from: SocketAddr,
        len: usize,
        payload_start: usize,
        now: Instant,
        is_keep_alive: bool,
    ) -> NetResult<()> {
        let Some(slot) = self.slot_index_of(from) else {
            return Ok(());
        };
        if is_keep_alive {
            // Validate the carried `connect_salt` BEFORE refreshing liveness, mirroring
            // Disconnect (§5.3): a spoofed-source KeepAlive with the wrong salt must not
            // refresh the victim's timeout deadline. Length guarded by the caller path.
            if len < HEADER_SIZE + control_layout::KEEP_ALIVE_LEN {
                return Ok(());
            }
            let salt_at = payload_start + control_layout::SALT_OFFSET;
            let salt = read_u64(&self.recv_buf[salt_at..]);
            let Slot::Connected(conn) = &mut self.slots[slot] else {
                return Ok(());
            };
            if conn.connect_salt != salt {
                return Ok(());
            }
            conn.mark_recv(now, len);
            conn.on_control(&header, now);
            return Ok(());
        }

        let Slot::Connected(conn) = &mut self.slots[slot] else {
            return Ok(());
        };
        conn.mark_recv(now, len);

        let Ok(channel) = ChannelId::try_from(header.channel) else {
            return Ok(());
        };
        let client = conn.client_id;
        let payload_len = len - payload_start;
        // Copy payload out of recv_buf so we can borrow the arena mutably.
        let mut tmp = [0u8; MAX_PAYLOAD_SIZE];
        tmp[..payload_len].copy_from_slice(&self.recv_buf[payload_start..len]);

        let outcome = conn.on_payload(&header, channel, &tmp[..payload_len], &mut self.arena, now);
        match outcome {
            Incoming::Message {
                channel,
                offset,
                len,
            } => {
                self.events.push(EventRecord::Message {
                    client,
                    channel,
                    offset,
                    len,
                });
            }
            Incoming::None => {}
        }
        // Drain any newly-deliverable reliable messages in order.
        if let Slot::Connected(conn) = &mut self.slots[slot] {
            while let Some((offset, mlen)) = conn.drain_reliable(&mut self.arena) {
                self.events.push(EventRecord::Message {
                    client,
                    channel: ChannelId::ReliableOrdered,
                    offset,
                    len: mlen,
                });
            }
        }
        Ok(())
    }

    fn on_disconnect_packet(&mut self, from: SocketAddr, len: usize) {
        if len < HEADER_SIZE + control_layout::DISCONNECT_LEN {
            return;
        }
        let salt = read_u64(&self.recv_buf[HEADER_SIZE + control_layout::SALT_OFFSET..]);
        if let Some(slot) = self.slot_index_of(from) {
            if let Slot::Connected(conn) = &self.slots[slot] {
                // Reject a spoofed-source Disconnect: the carried salt must match the
                // connection identity (§5.3). Mismatches are ignored silently.
                if conn.connect_salt != salt {
                    return;
                }
                let client = conn.client_id;
                self.slots[slot] = Slot::Free;
                self.events.push(EventRecord::Disconnected {
                    client,
                    reason: DisconnectReason::Disconnected,
                });
            }
        }
    }

    fn expire_pending(&mut self, now: Instant) {
        self.pending
            .retain(|p| now.duration_since(p.started) < HANDSHAKE_TIMEOUT);
    }

    fn expire_connections(&mut self, now: Instant) {
        for slot in 0..self.slots.len() {
            if let Slot::Connected(conn) = &self.slots[slot] {
                if conn.timed_out(now) {
                    let client = conn.client_id;
                    self.slots[slot] = Slot::Free;
                    self.events.push(EventRecord::Disconnected {
                        client,
                        reason: DisconnectReason::TimedOut,
                    });
                }
            }
        }
    }

    fn flush_connections(&mut self, now: Instant) -> NetResult<()> {
        let mut sink = SocketSink {
            socket: &self.socket,
        };
        for slot in self.slots.iter_mut() {
            if let Slot::Connected(conn) = slot {
                conn.flush(&mut self.send_scratch, now, &mut sink)?;
            }
        }
        Ok(())
    }

    fn advance_closing(&mut self, now: Instant) -> NetResult<()> {
        let mut i = 0;
        while i < self.closing.len() {
            let c = &mut self.closing[i];
            send_disconnect(&self.socket, &mut self.send_scratch, c.addr, c.connect_salt)?;
            c.remaining -= 1;
            if c.remaining == 0 {
                self.closing.swap_remove(i);
            } else {
                i += 1;
            }
        }
        let _ = now;
        Ok(())
    }

    fn send_challenge(
        &mut self,
        to: SocketAddr,
        client_salt: u64,
        server_salt: u64,
    ) -> NetResult<()> {
        let mut body = [0u8; control_layout::CHALLENGE_LEN];
        body[control_layout::SALT_OFFSET..control_layout::SALT_OFFSET + 8]
            .copy_from_slice(&client_salt.to_le_bytes());
        body[control_layout::SERVER_SALT_OFFSET..control_layout::SERVER_SALT_OFFSET + 8]
            .copy_from_slice(&server_salt.to_le_bytes());
        self.send_control_to(to, PacketType::Challenge, &body)
    }

    fn send_accepted(
        &mut self,
        to: SocketAddr,
        connect_salt: u64,
        client_id: ClientId,
    ) -> NetResult<()> {
        let mut body = [0u8; control_layout::CONNECT_ACCEPTED_LEN];
        body[control_layout::SALT_OFFSET..control_layout::SALT_OFFSET + 8]
            .copy_from_slice(&connect_salt.to_le_bytes());
        body[control_layout::CLIENT_ID_OFFSET..control_layout::CLIENT_ID_OFFSET + 4]
            .copy_from_slice(&client_id.0.to_le_bytes());
        self.send_control_to(to, PacketType::ConnectAccepted, &body)
    }

    fn send_denied(&mut self, to: SocketAddr, reason: DenyReason) -> NetResult<()> {
        let mut body = [0u8; control_layout::CONNECT_DENIED_LEN];
        body[control_layout::REASON_OFFSET] = reason.to_u8();
        self.send_control_to(to, PacketType::ConnectDenied, &body)
    }

    fn send_control_to(&mut self, to: SocketAddr, ty: PacketType, body: &[u8]) -> NetResult<()> {
        let header = PacketHeader {
            protocol_id: self.config.protocol_id,
            packet_type: ty,
            sequence: 0,
            ack: 0,
            ack_bits: 0,
            channel: PacketHeader::NO_CHANNEL,
        };
        let buf = &mut self.send_scratch;
        header.encode(buf.as_mut_slice())?;
        buf[HEADER_SIZE..HEADER_SIZE + body.len()].copy_from_slice(body);
        let total = HEADER_SIZE + body.len();
        match self.socket.send_to(&buf[..total], to) {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
            Err(e) => Err(NetError::Io(e)),
        }
    }

    /// Queue `bytes` to `client` on `channel`. Errors: `NoSuchClient`, `PayloadTooLarge`,
    /// `ChannelFull` (reliable backpressure — the message is never silently dropped).
    pub fn send(&mut self, client: ClientId, channel: ChannelId, bytes: &[u8]) -> NetResult<()> {
        if bytes.len() > MAX_PAYLOAD_SIZE {
            return Err(NetError::PayloadTooLarge {
                size: bytes.len(),
                max: MAX_PAYLOAD_SIZE,
            });
        }
        for slot in self.slots.iter_mut() {
            if let Slot::Connected(conn) = slot {
                if conn.client_id == client {
                    return conn.enqueue(channel, bytes);
                }
            }
        }
        Err(NetError::NoSuchClient)
    }

    /// Send to every connected client. A per-client `ChannelFull` is returned as `Err`
    /// for the first failing client; the others still receive the message.
    pub fn broadcast(&mut self, channel: ChannelId, bytes: &[u8]) -> NetResult<()> {
        if bytes.len() > MAX_PAYLOAD_SIZE {
            return Err(NetError::PayloadTooLarge {
                size: bytes.len(),
                max: MAX_PAYLOAD_SIZE,
            });
        }
        let mut first_err = None;
        for slot in self.slots.iter_mut() {
            if let Slot::Connected(conn) = slot {
                if let Err(e) = conn.enqueue(channel, bytes) {
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Gracefully disconnect one client: schedules redundant `Disconnect` packets and
    /// frees the slot immediately so no further traffic is accepted.
    pub fn disconnect(&mut self, client: ClientId) -> NetResult<()> {
        for slot in 0..self.slots.len() {
            if let Slot::Connected(conn) = &self.slots[slot] {
                if conn.client_id == client {
                    let addr = conn.addr;
                    let salt = conn.connect_salt;
                    self.slots[slot] = Slot::Free;
                    self.closing.push(Closing {
                        addr,
                        connect_salt: salt,
                        remaining: DISCONNECT_REDUNDANCY,
                    });
                    return Ok(());
                }
            }
        }
        Err(NetError::NoSuchClient)
    }

    /// Statistics for `client`, or `None` if not connected.
    pub fn stats(&self, client: ClientId) -> Option<ConnectionStats> {
        for slot in self.slots.iter() {
            if let Slot::Connected(conn) = slot {
                if conn.client_id == client {
                    return Some(conn.stats());
                }
            }
        }
        None
    }

    /// Number of currently connected clients.
    pub fn connected_clients(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| matches!(s, Slot::Connected(_)))
            .count()
    }

    /// The bound local address.
    pub fn local_addr(&self) -> NetResult<SocketAddr> {
        self.socket.local_addr().map_err(NetError::Io)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Best-effort graceful shutdown notice to each connected client.
        for slot in 0..self.slots.len() {
            if let Slot::Connected(conn) = &self.slots[slot] {
                let addr = conn.addr;
                let salt = conn.connect_salt;
                let _ = send_disconnect(&self.socket, &mut self.send_scratch, addr, salt);
            }
        }
    }
}

fn send_disconnect(
    socket: &UdpSocket,
    scratch: &mut [u8; MAX_PACKET_SIZE],
    addr: SocketAddr,
    connect_salt: u64,
) -> NetResult<()> {
    let header = PacketHeader {
        protocol_id: PROTOCOL_ID,
        packet_type: PacketType::Disconnect,
        sequence: 0,
        ack: 0,
        ack_bits: 0,
        channel: PacketHeader::NO_CHANNEL,
    };
    header.encode(scratch.as_mut_slice())?;
    let salt_at = HEADER_SIZE + control_layout::SALT_OFFSET;
    scratch[salt_at..salt_at + 8].copy_from_slice(&connect_salt.to_le_bytes());
    match socket.send_to(
        &scratch[..HEADER_SIZE + control_layout::DISCONNECT_LEN],
        addr,
    ) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
        Err(e) => Err(NetError::Io(e)),
    }
}

fn read_u64(src: &[u8]) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&src[..8]);
    u64::from_le_bytes(b)
}
