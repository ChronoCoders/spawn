//! Wire constants and the byte-precise packet header.

use crate::error::{NetError, NetResult};

/// Magic / version guard (`"SPN1"`). A packet whose `protocol_id` differs is dropped.
pub const PROTOCOL_ID: u32 = 0x5350_4E31;
/// Fixed header length in bytes preceding every payload.
pub const HEADER_SIZE: usize = 14;
/// MTU-budgeted UDP payload ceiling (header + body), conservative vs. the 1280-byte
/// IPv6 minimum MTU.
pub const MAX_PACKET_SIZE: usize = 1200;
/// Largest single application message accepted by `send` (`MAX_PACKET_SIZE - HEADER_SIZE`).
pub const MAX_PAYLOAD_SIZE: usize = MAX_PACKET_SIZE - HEADER_SIZE;
/// Number of prior sequences acknowledged by the `ack_bits` field.
pub const ACK_BITS: u32 = 32;

/// Control-packet payload layouts (offsets relative to the start of the body, i.e. byte
/// `HEADER_SIZE` of the datagram). Every field is little-endian. Encode and decode sites
/// reference these constants so the two stay in agreement by construction.
///
/// - `ConnectRequest`: `client_salt: u64` @ `SALT_OFFSET` (len `CONNECT_REQUEST_LEN`).
/// - `Challenge`: `client_salt: u64` @ `SALT_OFFSET`, `server_salt: u64` @
///   `SERVER_SALT_OFFSET` (len `CHALLENGE_LEN`).
/// - `ChallengeResponse`: `connect_salt: u64` @ `SALT_OFFSET` (len `CHALLENGE_RESPONSE_LEN`).
/// - `ConnectAccepted`: `connect_salt: u64` @ `SALT_OFFSET`, `client_id: u32` @
///   `CLIENT_ID_OFFSET` (len `CONNECT_ACCEPTED_LEN`).
/// - `ConnectDenied`: `reason: u8` @ `REASON_OFFSET` (len `CONNECT_DENIED_LEN`).
/// - `KeepAlive`: `connect_salt: u64` @ `SALT_OFFSET` (len `KEEP_ALIVE_LEN`).
/// - `Disconnect`: `connect_salt: u64` @ `SALT_OFFSET` (len `DISCONNECT_LEN`).
pub(crate) mod control_layout {
    /// Offset of the leading salt field (`client_salt`/`connect_salt`) in every salted body.
    pub(crate) const SALT_OFFSET: usize = 0;
    /// Offset of `server_salt` in a `Challenge` body.
    pub(crate) const SERVER_SALT_OFFSET: usize = 8;
    /// Offset of `client_id` in a `ConnectAccepted` body.
    pub(crate) const CLIENT_ID_OFFSET: usize = 8;
    /// Offset of `reason` in a `ConnectDenied` body.
    pub(crate) const REASON_OFFSET: usize = 0;

    /// `ConnectRequest` body length: one `u64` salt.
    pub(crate) const CONNECT_REQUEST_LEN: usize = 8;
    /// `Challenge` body length: two `u64` salts.
    pub(crate) const CHALLENGE_LEN: usize = 16;
    /// `ChallengeResponse` body length: one `u64` salt.
    pub(crate) const CHALLENGE_RESPONSE_LEN: usize = 8;
    /// `ConnectAccepted` body length: one `u64` salt + one `u32` client id.
    pub(crate) const CONNECT_ACCEPTED_LEN: usize = 12;
    /// `ConnectDenied` body length: one `u8` reason.
    pub(crate) const CONNECT_DENIED_LEN: usize = 1;
    /// `KeepAlive` body length: one `u64` salt.
    pub(crate) const KEEP_ALIVE_LEN: usize = 8;
    /// `Disconnect` body length: one `u64` salt.
    pub(crate) const DISCONNECT_LEN: usize = 8;
}

/// Packet kind discriminant carried at header offset 4.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PacketType {
    ConnectRequest = 0,
    Challenge = 1,
    ChallengeResponse = 2,
    ConnectAccepted = 3,
    ConnectDenied = 4,
    KeepAlive = 5,
    Payload = 6,
    Disconnect = 7,
}

impl TryFrom<u8> for PacketType {
    type Error = NetError;

    fn try_from(value: u8) -> NetResult<Self> {
        match value {
            0 => Ok(Self::ConnectRequest),
            1 => Ok(Self::Challenge),
            2 => Ok(Self::ChallengeResponse),
            3 => Ok(Self::ConnectAccepted),
            4 => Ok(Self::ConnectDenied),
            5 => Ok(Self::KeepAlive),
            6 => Ok(Self::Payload),
            7 => Ok(Self::Disconnect),
            _ => Err(NetError::MalformedPacket),
        }
    }
}

/// Logical packet header, serialized field-by-field in little-endian. Never transmuted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketHeader {
    pub protocol_id: u32,
    pub packet_type: PacketType,
    pub sequence: u16,
    pub ack: u16,
    pub ack_bits: u32,
    pub channel: u8,
}

impl PacketHeader {
    /// Sentinel `channel` value for control packets that carry no application channel.
    pub const NO_CHANNEL: u8 = 0xFF;

    /// Serialize into `out`. Multi-byte fields are little-endian per the wire spec.
    /// `Err(BufferTooSmall)` if `out.len() < HEADER_SIZE`.
    pub fn encode(self, out: &mut [u8]) -> NetResult<()> {
        if out.len() < HEADER_SIZE {
            return Err(NetError::BufferTooSmall);
        }
        out[0..4].copy_from_slice(&self.protocol_id.to_le_bytes());
        out[4] = self.packet_type as u8;
        out[5..7].copy_from_slice(&self.sequence.to_le_bytes());
        out[7..9].copy_from_slice(&self.ack.to_le_bytes());
        out[9..13].copy_from_slice(&self.ack_bits.to_le_bytes());
        out[13] = self.channel;
        Ok(())
    }

    /// Parse a header from the front of `src`. `Err(MalformedPacket)` if `src` is
    /// shorter than `HEADER_SIZE` or carries an unknown `packet_type`.
    pub fn decode(src: &[u8]) -> NetResult<Self> {
        if src.len() < HEADER_SIZE {
            return Err(NetError::MalformedPacket);
        }
        let protocol_id = u32::from_le_bytes([src[0], src[1], src[2], src[3]]);
        let packet_type = PacketType::try_from(src[4])?;
        let sequence = u16::from_le_bytes([src[5], src[6]]);
        let ack = u16::from_le_bytes([src[7], src[8]]);
        let ack_bits = u32::from_le_bytes([src[9], src[10], src[11], src[12]]);
        let channel = src[13];
        Ok(Self {
            protocol_id,
            packet_type,
            sequence,
            ack,
            ack_bits,
            channel,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(h: PacketHeader) {
        let mut buf = [0u8; HEADER_SIZE];
        h.encode(&mut buf).unwrap();
        let decoded = PacketHeader::decode(&buf).unwrap();
        assert_eq!(h, decoded);
    }

    #[test]
    fn roundtrip_representative_and_boundary() {
        let types = [
            PacketType::ConnectRequest,
            PacketType::Challenge,
            PacketType::ChallengeResponse,
            PacketType::ConnectAccepted,
            PacketType::ConnectDenied,
            PacketType::KeepAlive,
            PacketType::Payload,
            PacketType::Disconnect,
        ];
        for t in types {
            roundtrip(PacketHeader {
                protocol_id: PROTOCOL_ID,
                packet_type: t,
                sequence: 0,
                ack: 0,
                ack_bits: 0,
                channel: PacketHeader::NO_CHANNEL,
            });
            roundtrip(PacketHeader {
                protocol_id: PROTOCOL_ID,
                packet_type: t,
                sequence: 0xFFFF,
                ack: 0xFFFF,
                ack_bits: 0xFFFF_FFFF,
                channel: 2,
            });
        }
    }

    #[test]
    fn little_endian_layout() {
        let h = PacketHeader {
            protocol_id: PROTOCOL_ID,
            packet_type: PacketType::Payload,
            sequence: 0x0102,
            ack: 0x0304,
            ack_bits: 0x0506_0708,
            channel: 0x09,
        };
        let mut buf = [0u8; HEADER_SIZE];
        h.encode(&mut buf).unwrap();
        assert_eq!(&buf[0..4], &PROTOCOL_ID.to_le_bytes());
        assert_eq!(buf[4], PacketType::Payload as u8);
        assert_eq!(&buf[5..7], &[0x02, 0x01]);
        assert_eq!(&buf[7..9], &[0x04, 0x03]);
        assert_eq!(&buf[9..13], &[0x08, 0x07, 0x06, 0x05]);
        assert_eq!(buf[13], 0x09);
    }

    #[test]
    fn encode_rejects_small_buffer() {
        let h = PacketHeader {
            protocol_id: PROTOCOL_ID,
            packet_type: PacketType::KeepAlive,
            sequence: 0,
            ack: 0,
            ack_bits: 0,
            channel: PacketHeader::NO_CHANNEL,
        };
        let mut buf = [0u8; HEADER_SIZE - 1];
        assert!(matches!(h.encode(&mut buf), Err(NetError::BufferTooSmall)));
    }

    #[test]
    fn decode_rejects_short_buffer() {
        let buf = [0u8; HEADER_SIZE - 1];
        assert!(matches!(
            PacketHeader::decode(&buf),
            Err(NetError::MalformedPacket)
        ));
    }

    #[test]
    fn decode_rejects_unknown_type() {
        let mut buf = [0u8; HEADER_SIZE];
        buf[4] = 200;
        assert!(matches!(
            PacketHeader::decode(&buf),
            Err(NetError::MalformedPacket)
        ));
    }

    #[test]
    fn packet_type_try_from() {
        assert_eq!(PacketType::try_from(6).unwrap(), PacketType::Payload);
        assert!(PacketType::try_from(8).is_err());
    }
}
