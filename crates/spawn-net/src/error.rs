//! Error and result types for the transport layer.

use std::error::Error;
use std::fmt;

use spawn_core::SpawnError;

use crate::connection::DenyReason;

/// Transport-layer error.
///
/// `#[non_exhaustive]`: later phases (fragmentation, encryption) add variants
/// via approved specs, so external matches must carry a wildcard arm. `WouldBlock`
/// from the non-blocking socket is the idle path and is never surfaced as a variant.
#[derive(Debug)]
#[non_exhaustive]
pub enum NetError {
    Io(std::io::Error),
    MalformedPacket,
    PayloadTooLarge { size: usize, max: usize },
    ChannelFull,
    ConnectionDenied(DenyReason),
    ConnectionTimedOut,
    NoSuchClient,
    BufferTooSmall,
    InvalidState { context: &'static str },
    NotConnected,
}

/// Result alias for transport operations.
pub type NetResult<T> = Result<T, NetError>;

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::MalformedPacket => write!(f, "malformed packet"),
            Self::PayloadTooLarge { size, max } => {
                write!(f, "payload too large: {size} bytes (max {max})")
            }
            Self::ChannelFull => write!(f, "reliable channel send window full"),
            Self::ConnectionDenied(reason) => write!(f, "connection denied: {reason:?}"),
            Self::ConnectionTimedOut => write!(f, "connection timed out"),
            Self::NoSuchClient => write!(f, "no such client"),
            Self::BufferTooSmall => write!(f, "buffer too small"),
            Self::InvalidState { context } => write!(f, "invalid state: {context}"),
            Self::NotConnected => write!(f, "not connected"),
        }
    }
}

impl Error for NetError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for NetError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<NetError> for SpawnError {
    fn from(err: NetError) -> Self {
        match err {
            NetError::Io(io) => SpawnError::Io(io),
            NetError::MalformedPacket => SpawnError::Parse {
                context: "net: malformed packet",
            },
            NetError::PayloadTooLarge { .. } => SpawnError::InvalidArgument {
                context: "net: payload too large",
            },
            NetError::ChannelFull => SpawnError::InvalidState {
                context: "net: channel full",
            },
            NetError::ConnectionDenied(_) => SpawnError::InvalidState {
                context: "net: connection denied",
            },
            NetError::ConnectionTimedOut => SpawnError::InvalidState {
                context: "net: connection timed out",
            },
            NetError::NoSuchClient => SpawnError::NotFound {
                context: "net: no such client",
            },
            NetError::BufferTooSmall => SpawnError::InvalidArgument {
                context: "net: buffer too small",
            },
            NetError::InvalidState { context } => SpawnError::InvalidState { context },
            NetError::NotConnected => SpawnError::InvalidState {
                context: "net: not connected",
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_non_empty_for_every_variant() {
        let variants = [
            NetError::Io(std::io::Error::other("x")),
            NetError::MalformedPacket,
            NetError::PayloadTooLarge { size: 2, max: 1 },
            NetError::ChannelFull,
            NetError::ConnectionDenied(DenyReason::ServerFull),
            NetError::ConnectionTimedOut,
            NetError::NoSuchClient,
            NetError::BufferTooSmall,
            NetError::InvalidState { context: "c" },
            NetError::NotConnected,
        ];
        for v in &variants {
            assert!(!v.to_string().is_empty());
        }
    }

    #[test]
    fn source_some_only_for_io() {
        let io = NetError::Io(std::io::Error::other("x"));
        assert!(io.source().is_some());
        assert!(NetError::MalformedPacket.source().is_none());
    }

    #[test]
    fn maps_to_spawn_error() {
        let io: SpawnError = NetError::Io(std::io::Error::other("x")).into();
        assert!(matches!(io, SpawnError::Io(_)));
        let nf: SpawnError = NetError::NoSuchClient.into();
        assert!(matches!(nf, SpawnError::NotFound { .. }));
        let st: SpawnError = NetError::ChannelFull.into();
        assert!(matches!(st, SpawnError::InvalidState { .. }));
        let arg: SpawnError = NetError::PayloadTooLarge { size: 2, max: 1 }.into();
        assert!(matches!(arg, SpawnError::InvalidArgument { .. }));
    }
}
