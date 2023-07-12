//! Common types used across this crate

use crate::{
    config::*,
    socket_connection_handler::Peer,
    SocketServerSerializer,
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
pub(crate) type SenderChannel<ItemType, const BUFFER_SIZE: usize> = reactive_mutiny::uni::channels::movable::atomic::Atomic::<'static, ItemType, BUFFER_SIZE, 1>;

// Uni types for handling socket connections
pub(crate) type SocketProcessorUniType<MessagesType>     = UniZeroCopyAtomic<MessagesType, RECEIVER_BUFFER_SIZE, 1, SOCKET_PROCESSOR_INSTRUMENTS>;
pub(crate) type SocketProcessorChannelType<MessagesType> = ChannelUniZeroCopyAtomic<MessagesType, RECEIVER_BUFFER_SIZE, 1>;
pub        type SocketProcessorDerivedType<MessagesType> = OgreUnique<MessagesType, AllocatorAtomicArray<MessagesType, RECEIVER_BUFFER_SIZE>>;

/// The internal events a reactive processor (for a server or client) shares with the user code.\
/// The user code may use those events to maintain a list of connected clients, be notified of stop/close/quit requests, init/deinit sessions, etc.
/// Note that the `Peer` objects received in those events may be used, at any time, to send messages to the clients -- like "Shutting down. Goodbye".
/// *When doing this on other occasions, make sure you won't break your own protocol.*
#[derive(Debug)]
pub enum ConnectionEvent<LocalPeerMessages:  'static + Send + Sync + PartialEq + Debug + SocketServerSerializer<LocalPeerMessages>> {
    PeerConnected {peer: Arc<Peer<LocalPeerMessages>>},
    PeerDisconnected {peer: Arc<Peer<LocalPeerMessages>>, stream_stats: Arc<reactive_mutiny::stream_executor::StreamExecutor>},
    ApplicationShutdown {timeout_ms: u32},
}