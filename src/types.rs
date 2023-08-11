//! Common types used across this crate

use crate::socket_connection_handler::Peer;
use std::{
    fmt::Debug,
    sync::Arc,
};
use reactive_mutiny::prelude::advanced::{UniZeroCopyAtomic, OgreUnique, AllocatorAtomicArray, ChannelUniMoveAtomic};
use reactive_mutiny::prelude::{FullDuplexUniChannel, GenericUni, MutinyStream};


pub type MessagingMoveChannelType  <const MESSAGES_BUFFER_SIZE: usize, MessagesType>                               = ChannelUniMoveAtomic<MessagesType, MESSAGES_BUFFER_SIZE, 1>;
pub type MessagingAtomicUniType    <const MESSAGES_BUFFER_SIZE: usize, const UNI_INSTRUMENTS: usize, MessagesType> = UniZeroCopyAtomic<MessagesType, MESSAGES_BUFFER_SIZE, 1, UNI_INSTRUMENTS>;
pub type MessagingAtomicDerivedType<const MESSAGES_BUFFER_SIZE: usize, MessagesType>                               = OgreUnique<MessagesType, AllocatorAtomicArray<MessagesType, MESSAGES_BUFFER_SIZE>>;
/// Concrete type of the `Stream`s this crate produces.\
/// Type for the `Stream` we create when reading from the remote peer.\
/// This type is intended to be used only for the first level of `dialog_processor_builder()`s you pass to
/// the [SocketClient] or [SocketServer], as Rust Generics isn't able to infer a generic `Stream` type
/// in this situation (in which the `Stream` is created inside the generic function itself).\
/// If your logic uses functions that receive `Stream`s, you'll want flexibility to do whatever you want
/// with the `Stream` (which would no longer be a `MutinyStream`), so declare such functions as:
/// ```no_compile
///     fn dialog_processor<RemoteStreamType: Stream<Item=SocketProcessorDerivedType<RemoteMessages>>>
///                        (remote_messages_stream: RemoteStreamType) -> impl Stream<Item=LocalMessages> { ... }
pub type MessagingMutinyStream<GenericUniType: GenericUni> = MutinyStream<'static, GenericUniType::ItemType, GenericUniType::UniChannelType, GenericUniType::DerivedItemType>;

/// The internal events a reactive processor (for a server or client) shares with the user code.\
/// The user code may use those events to maintain a list of connected clients, be notified of stop/close/quit requests, init/deinit sessions, etc.
/// Note that the `Peer` objects received in those events may be used, at any time, to send messages to the clients -- like "Shutting down. Goodbye".
/// *When doing this on other occasions, make sure you won't break your own protocol.*
#[derive(Debug)]
pub enum ConnectionEvent<SenderChannelType: FullDuplexUniChannel + Sync + Send> {
    PeerConnected       {peer: Arc<Peer<SenderChannelType>>},
    PeerDisconnected    {peer: Arc<Peer<SenderChannelType>>, stream_stats: Arc<dyn reactive_mutiny::stream_executor::StreamExecutorStats + Sync + Send>},
    ApplicationShutdown {timeout_ms: u32},
}