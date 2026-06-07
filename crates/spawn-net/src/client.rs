//! UDP client: handshake driver, connection state machine, poll-driven events.

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
    PacketHeader, PacketType, HEADER_SIZE, MAX_PACKET_SIZE, MAX_PAYLOAD_SIZE, PROTOCOL_ID,
};
use crate::stats::ConnectionStats;

/// Client connection state. `send` is only valid in `Connected`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientState {
    Disconnected,
    Connecting,
    Connected,
}

struct Handshake {
    server: SocketAddr,
    client_salt: u64,
    server_salt: Option<u64>,
    connect_salt: Option<u64>,
    started: Instant,
}

/// Transport client. Binds an ephemeral local socket and drives the salt handshake
/// (§5.1) before promoting to a full `Connection` on `ConnectAccepted`.
pub struct Client {
    socket: UdpSocket,
    state: ClientState,
    handshake: Option<Handshake>,
    conn: Option<Connection>,
    closing_remaining: u32,
    closing_addr: Option<SocketAddr>,
    closing_salt: u64,
    rng: SaltRng,
    recv_buf: Vec<u8>,
    send_scratch: Box<[u8; MAX_PACKET_SIZE]>,
    events: Vec<EventRecord>,
    arena: Vec<u8>,
    pending_stats: ConnectionStats,
}

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

const EMPTY_STATS: ConnectionStats = ConnectionStats {
    rtt: std::time::Duration::ZERO,
    packet_loss: 0.0,
    bytes_sent: 0,
    bytes_received: 0,
    packets_sent: 0,
    packets_received: 0,
};

impl Client {
    /// Bind an ephemeral non-blocking local socket without contacting any server.
    pub fn new() -> NetResult<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.set_nonblocking(true)?;
        let disc = socket.local_addr().map(|a| a.port() as u64).unwrap_or(0);
        Ok(Self {
            socket,
            state: ClientState::Disconnected,
            handshake: None,
            conn: None,
            closing_remaining: 0,
            closing_addr: None,
            closing_salt: 0,
            rng: SaltRng::new(seed_from_env(disc ^ 0xA5A5)),
            recv_buf: vec![0u8; MAX_PACKET_SIZE],
            send_scratch: Box::new([0u8; MAX_PACKET_SIZE]),
            events: Vec::new(),
            arena: Vec::with_capacity(MAX_PACKET_SIZE),
            pending_stats: EMPTY_STATS,
        })
    }

    /// Begin the handshake to `server`. Transitions to `Connecting`. `Err(InvalidState)`
    /// unless currently `Disconnected`.
    pub fn connect(&mut self, server: SocketAddr) -> NetResult<()> {
        if self.state != ClientState::Disconnected {
            return Err(NetError::InvalidState {
                context: "connect while not disconnected",
            });
        }
        let client_salt = self.rng.next_u64();
        self.handshake = Some(Handshake {
            server,
            client_salt,
            server_salt: None,
            connect_salt: None,
            started: Instant::now(),
        });
        self.state = ClientState::Connecting;
        self.pending_stats = EMPTY_STATS;
        Ok(())
    }

    /// Drain the socket, advance handshake/keep-alive/resend, and return this poll's
    /// events. Returned events borrow internal buffers; valid until the next `poll`.
    pub fn poll(&mut self) -> NetResult<NetEventIter<'_>> {
        self.events.clear();
        self.arena.clear();
        let now = Instant::now();

        self.drain_socket(now)?;
        self.advance_handshake(now)?;
        self.expire(now);
        self.flush(now)?;
        self.advance_closing()?;

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
            if header.protocol_id != PROTOCOL_ID {
                continue;
            }
            if !self.is_server_addr(from) {
                continue;
            }
            self.handle_packet(header, len, now)?;
        }
        Ok(())
    }

    fn is_server_addr(&self, from: SocketAddr) -> bool {
        if let Some(c) = &self.conn {
            return c.addr == from;
        }
        if let Some(h) = &self.handshake {
            return h.server == from;
        }
        false
    }

    fn handle_packet(&mut self, header: PacketHeader, len: usize, now: Instant) -> NetResult<()> {
        match header.packet_type {
            PacketType::Challenge => self.on_challenge(len),
            PacketType::ConnectAccepted => self.on_accepted(len, now),
            PacketType::ConnectDenied => self.on_denied(len),
            PacketType::Payload => self.on_payload(header, len, now),
            PacketType::KeepAlive => self.on_keep_alive(header, len, now),
            PacketType::Disconnect => self.on_disconnect(len),
            _ => {}
        }
        Ok(())
    }

    fn on_challenge(&mut self, len: usize) {
        if len < HEADER_SIZE + 16 {
            return;
        }
        let Some(h) = &mut self.handshake else {
            return;
        };
        let echoed = read_u64(&self.recv_buf[HEADER_SIZE..]);
        if echoed != h.client_salt {
            return;
        }
        let server_salt = read_u64(&self.recv_buf[HEADER_SIZE + 8..]);
        h.server_salt = Some(server_salt);
        h.connect_salt = Some(h.client_salt ^ server_salt);
    }

    fn on_accepted(&mut self, len: usize, now: Instant) {
        if len < HEADER_SIZE + 12 {
            return;
        }
        let Some(h) = &self.handshake else {
            return;
        };
        let Some(connect_salt) = h.connect_salt else {
            return;
        };
        let echoed = read_u64(&self.recv_buf[HEADER_SIZE..]);
        if echoed != connect_salt {
            return;
        }
        let raw_id = read_u32(&self.recv_buf[HEADER_SIZE + 8..]);
        let client_id = ClientId(raw_id);
        let server = h.server;
        self.conn = Some(Connection::new(server, client_id, connect_salt, now));
        self.handshake = None;
        self.state = ClientState::Connected;
        self.events
            .push(EventRecord::Connected { client: client_id });
    }

    fn on_denied(&mut self, len: usize) {
        if len < HEADER_SIZE + 1 {
            return;
        }
        if self.state != ClientState::Connecting {
            return;
        }
        let reason =
            DenyReason::from_u8(self.recv_buf[HEADER_SIZE]).unwrap_or(DenyReason::ServerFull);
        self.state = ClientState::Disconnected;
        self.handshake = None;
        self.events.push(EventRecord::Disconnected {
            client: ClientId(0),
            reason: DisconnectReason::Denied(reason),
        });
    }

    fn on_payload(&mut self, header: PacketHeader, len: usize, now: Instant) {
        let Some(conn) = &mut self.conn else {
            return;
        };
        conn.mark_recv(now, len);
        let Ok(channel) = ChannelId::try_from(header.channel) else {
            return;
        };
        let client = conn.client_id;
        let payload_len = len - HEADER_SIZE;
        let mut tmp = [0u8; MAX_PAYLOAD_SIZE];
        tmp[..payload_len].copy_from_slice(&self.recv_buf[HEADER_SIZE..len]);
        let outcome = conn.on_payload(&header, channel, &tmp[..payload_len], &mut self.arena, now);
        if let Incoming::Message {
            channel,
            offset,
            len,
        } = outcome
        {
            self.events.push(EventRecord::Message {
                client,
                channel,
                offset,
                len,
            });
        }
        if let Some(conn) = &mut self.conn {
            while let Some((offset, mlen)) = conn.drain_reliable(&mut self.arena) {
                self.events.push(EventRecord::Message {
                    client,
                    channel: ChannelId::ReliableOrdered,
                    offset,
                    len: mlen,
                });
            }
        }
    }

    fn on_keep_alive(&mut self, header: PacketHeader, len: usize, now: Instant) {
        if len < HEADER_SIZE + 8 {
            return;
        }
        if let Some(conn) = &mut self.conn {
            conn.mark_recv(now, len);
            conn.on_control(&header, now);
        }
    }

    fn on_disconnect(&mut self, len: usize) {
        if len < HEADER_SIZE + 8 {
            return;
        }
        let salt = read_u64(&self.recv_buf[HEADER_SIZE..]);
        // Reject a spoofed-source Disconnect: the carried salt must match the connection
        // identity (§5.3). Mismatches are ignored silently.
        if self.conn.as_ref().is_some_and(|c| c.connect_salt != salt) {
            return;
        }
        if let Some(conn) = self.conn.take() {
            let client = conn.client_id;
            self.state = ClientState::Disconnected;
            self.pending_stats = conn.stats();
            self.events.push(EventRecord::Disconnected {
                client,
                reason: DisconnectReason::Disconnected,
            });
        }
    }

    fn advance_handshake(&mut self, now: Instant) -> NetResult<()> {
        let Some(h) = &self.handshake else {
            return Ok(());
        };
        let server = h.server;
        match h.connect_salt {
            None => {
                // Awaiting Challenge: (re)send ConnectRequest each poll.
                let salt = h.client_salt;
                self.send_handshake(server, PacketType::ConnectRequest, &salt.to_le_bytes())?;
            }
            Some(connect_salt) => {
                // Have challenge: (re)send ChallengeResponse until accepted.
                self.send_handshake(
                    server,
                    PacketType::ChallengeResponse,
                    &connect_salt.to_le_bytes(),
                )?;
            }
        }
        let _ = now;
        Ok(())
    }

    fn expire(&mut self, now: Instant) {
        if let Some(h) = &self.handshake {
            if now.duration_since(h.started) >= HANDSHAKE_TIMEOUT {
                self.handshake = None;
                self.state = ClientState::Disconnected;
                self.events.push(EventRecord::Disconnected {
                    client: ClientId(0),
                    reason: DisconnectReason::HandshakeTimeout,
                });
            }
        }
        if let Some(conn) = &self.conn {
            if conn.timed_out(now) {
                let client = conn.client_id;
                if let Some(c) = self.conn.take() {
                    self.pending_stats = c.stats();
                }
                self.state = ClientState::Disconnected;
                self.events.push(EventRecord::Disconnected {
                    client,
                    reason: DisconnectReason::TimedOut,
                });
            }
        }
    }

    fn flush(&mut self, now: Instant) -> NetResult<()> {
        if let Some(conn) = &mut self.conn {
            let mut sink = SocketSink {
                socket: &self.socket,
            };
            conn.flush(&mut self.send_scratch, now, &mut sink)?;
        }
        Ok(())
    }

    fn advance_closing(&mut self) -> NetResult<()> {
        if self.closing_remaining == 0 {
            return Ok(());
        }
        if let Some(addr) = self.closing_addr {
            send_disconnect(
                &self.socket,
                &mut self.send_scratch,
                addr,
                self.closing_salt,
            )?;
        }
        self.closing_remaining -= 1;
        if self.closing_remaining == 0 {
            self.closing_addr = None;
        }
        Ok(())
    }

    fn send_handshake(&mut self, to: SocketAddr, ty: PacketType, body: &[u8]) -> NetResult<()> {
        let header = PacketHeader {
            protocol_id: PROTOCOL_ID,
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

    /// Queue `bytes` to the server on `channel`. Errors: `NotConnected`, `PayloadTooLarge`,
    /// `ChannelFull` (reliable backpressure — the message is never silently dropped).
    pub fn send(&mut self, channel: ChannelId, bytes: &[u8]) -> NetResult<()> {
        if self.state != ClientState::Connected {
            return Err(NetError::NotConnected);
        }
        if bytes.len() > MAX_PAYLOAD_SIZE {
            return Err(NetError::PayloadTooLarge {
                size: bytes.len(),
                max: MAX_PAYLOAD_SIZE,
            });
        }
        match &mut self.conn {
            Some(conn) => conn.enqueue(channel, bytes),
            None => Err(NetError::NotConnected),
        }
    }

    /// Graceful disconnect: schedules redundant `Disconnect` packets and returns to
    /// `Disconnected` immediately.
    pub fn disconnect(&mut self) -> NetResult<()> {
        if let Some(conn) = self.conn.take() {
            self.closing_addr = Some(conn.addr);
            self.closing_salt = conn.connect_salt;
            self.closing_remaining = DISCONNECT_REDUNDANCY;
            self.pending_stats = conn.stats();
        }
        self.handshake = None;
        self.state = ClientState::Disconnected;
        Ok(())
    }

    /// Current connection state.
    pub fn state(&self) -> ClientState {
        self.state
    }

    /// Statistics for the current (or most recent) connection.
    pub fn stats(&self) -> ConnectionStats {
        match &self.conn {
            Some(conn) => conn.stats(),
            None => self.pending_stats,
        }
    }

    /// The bound local address.
    pub fn local_addr(&self) -> NetResult<SocketAddr> {
        self.socket.local_addr().map_err(NetError::Io)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Some(conn) = &self.conn {
            let addr = conn.addr;
            let salt = conn.connect_salt;
            let _ = send_disconnect(&self.socket, &mut self.send_scratch, addr, salt);
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
    scratch[HEADER_SIZE..HEADER_SIZE + 8].copy_from_slice(&connect_salt.to_le_bytes());
    match socket.send_to(&scratch[..HEADER_SIZE + 8], addr) {
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

fn read_u32(src: &[u8]) -> u32 {
    let mut b = [0u8; 4];
    b.copy_from_slice(&src[..4]);
    u32::from_le_bytes(b)
}
