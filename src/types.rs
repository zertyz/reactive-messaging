//! Common types used across this crate

use crate::{
    socket_connection_handler::Peer,
    ReactiveMessagingSerializer,
};
use std::{
    fmt::Debug,
    sync::Arc,
};
use reactive_mutiny::prelude::advanced::{
    UniZeroCopyAtomic,
    ChannelUniZeroCopyAtomic,
    OgreUnique,
    AllocatorAtomicArray,
};


/// The fastest channel for sender `Stream`s -- see `benches/streamable_channels.rs`
pub(crate) type SenderChannel<ItemType, const BUFFERED_MESSAGES_PER_PEER_COUNT: usize> = reactive_mutiny::uni::channels::movable::atomic::Atomic::<'static, ItemType, BUFFERED_MESSAGES_PER_PEER_COUNT, 1>;

// Uni types for handling socket connections
pub(crate) type SocketProcessorUniType    <const BUFFERED_MESSAGES_PER_CLIENT_COUNT: usize, const SOCKET_PROCESSOR_INSTRUMENTS: usize, MessagesType> = UniZeroCopyAtomic<MessagesType, BUFFERED_MESSAGES_PER_CLIENT_COUNT, 1, SOCKET_PROCESSOR_INSTRUMENTS>;
pub(crate) type SocketProcessorChannelType<const BUFFERED_MESSAGES_PER_CLIENT_COUNT: usize, MessagesType>                                            = ChannelUniZeroCopyAtomic<MessagesType, BUFFERED_MESSAGES_PER_CLIENT_COUNT, 1>;
pub        type SocketProcessorDerivedType<const BUFFERED_MESSAGES_PER_CLIENT_COUNT: usize, MessagesType>                                            = OgreUnique<MessagesType, AllocatorAtomicArray<MessagesType, BUFFERED_MESSAGES_PER_CLIENT_COUNT>>;

/// The internal events a reactive processor (for a server or client) shares with the user code.\
/// The user code may use those events to maintain a list of connected clients, be notified of stop/close/quit requests, init/deinit sessions, etc.
/// Note that the `Peer` objects received in those events may be used, at any time, to send messages to the clients -- like "Shutting down. Goodbye".
/// *When doing this on other occasions, make sure you won't break your own protocol.*
#[derive(Debug)]
pub enum ConnectionEvent<const BUFFERED_MESSAGES_PER_PEER_COUNT: usize,
                         LocalPeerMessages:                      'static + Send + Sync + PartialEq + Debug + ReactiveMessagingSerializer<LocalPeerMessages>> {
    PeerConnected       {peer: Arc<Peer<BUFFERED_MESSAGES_PER_PEER_COUNT, LocalPeerMessages>>},
    PeerDisconnected    {peer: Arc<Peer<BUFFERED_MESSAGES_PER_PEER_COUNT, LocalPeerMessages>>, stream_stats: Arc<reactive_mutiny::stream_executor::StreamExecutor>},
    ApplicationShutdown {timeout_ms: u32},
}