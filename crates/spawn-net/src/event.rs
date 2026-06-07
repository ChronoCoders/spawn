//! Poll-driven event stream surfaced by `Server` and `Client`.

use crate::channel::ChannelId;
use crate::connection::DisconnectReason;

/// Stable per-server identity of a connected peer. On the client this is the local id
/// assigned by the server; on the server it identifies the sender.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientId(pub u32);

/// A network event produced by `poll`.
///
/// `Message::bytes` borrows the engine's internal receive buffer and is only valid
/// until the next `poll`. Reliable-ordered messages are emitted in message-id order.
#[derive(Debug)]
pub enum NetEvent<'a> {
    Connected {
        client: ClientId,
    },
    Disconnected {
        client: ClientId,
        reason: DisconnectReason,
    },
    Message {
        client: ClientId,
        channel: ChannelId,
        bytes: &'a [u8],
    },
}

/// Owned descriptor of a pending event. The borrowed payload is resolved against the
/// shared receive arena when the iterator yields, keeping `poll` allocation-free for
/// control events and bounded to one arena for messages.
#[derive(Debug, Clone, Copy)]
pub(crate) enum EventRecord {
    Connected {
        client: ClientId,
    },
    Disconnected {
        client: ClientId,
        reason: DisconnectReason,
    },
    Message {
        client: ClientId,
        channel: ChannelId,
        offset: usize,
        len: usize,
    },
}

/// Iterator over the events produced by a single `poll`. Borrows the engine's event
/// records and message arena; valid until the next `poll`.
pub struct NetEventIter<'a> {
    records: &'a [EventRecord],
    arena: &'a [u8],
    index: usize,
}

impl<'a> NetEventIter<'a> {
    pub(crate) fn new(records: &'a [EventRecord], arena: &'a [u8]) -> Self {
        Self {
            records,
            arena,
            index: 0,
        }
    }
}

impl<'a> Iterator for NetEventIter<'a> {
    type Item = NetEvent<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let rec = self.records.get(self.index)?;
        self.index += 1;
        Some(match *rec {
            EventRecord::Connected { client } => NetEvent::Connected { client },
            EventRecord::Disconnected { client, reason } => {
                NetEvent::Disconnected { client, reason }
            }
            EventRecord::Message {
                client,
                channel,
                offset,
                len,
            } => NetEvent::Message {
                client,
                channel,
                bytes: &self.arena[offset..offset + len],
            },
        })
    }
}
