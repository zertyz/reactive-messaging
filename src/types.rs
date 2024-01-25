//! Common types used across this crate

use crate::{socket_connection::peer::Peer};
use std::{
    fmt::Debug,
    sync::Arc,
};
use reactive_mutiny::prelude::{FullDuplexUniChannel, GenericUni, MutinyStream};
use crate::serde::ReactiveMessagingSerializer;


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


/// Adherents will, typically, also implement [ReactiveMessagingUnresponsiveSerializer].\
/// By upgrading your type with this trait, it is possible to build a "Responsive Processor", where the returned `Stream`
/// contains the messages to be sent as an answer to the remote peer.\
/// This trait, therefore, specifies (to the internal sender) how to handle special response cases, like "no answer" and "disconnection" messages.
pub trait ResponsiveMessages<LocalPeerMessages: ResponsiveMessages<LocalPeerMessages> + Send + PartialEq + Debug> {

    /// Informs the internal sender if the given `processor_answer` is a "disconnect" message & command (issued by the messages processor logic)\
    /// -- in which case, the network processor will send it and, immediately, close the connection.\
    /// IMPLEMENTORS: #[inline(always)]
    fn is_disconnect_message(processor_answer: &LocalPeerMessages) -> bool;

    /// Tells if internal sender if the given `processor_answer` represents a "no message" -- a message that should produce no answer to the peer.\
    /// IMPLEMENTORS: #[inline(always)]
    fn is_no_answer_message(processor_answer: &LocalPeerMessages) -> bool;
}


/// The internal events a reactive processor (for a server or client) shares with the user code.\
/// The user code may use those events to maintain a list of connected clients, be notified of stop/close/quit requests, init/de-init sessions, etc.
/// Note that the `Peer` objects received in those events may be used, at any time, to send messages to the clients -- like "Shutting down. Goodbye".
/// IMPLEMENTATION NOTE: GAT traits (to reduce the number of generic parameters) couldn't be used here -- even after applying this compiler bug workaround https://github.com/rust-lang/rust/issues/102211#issuecomment-1513931928
///                      -- the "error: implementation of `std::marker::Send` is not general enough" bug kept on popping up in user provided closures that called other async functions.
#[derive(Debug)]
pub enum ConnectionEvent<const CONFIG:  u64,
                         LocalMessages: ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
                         SenderChannel: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync,
                         StateType:                                                                                   Send + Sync + Clone     + Debug + 'static = ()> {
    /// Happens when the remote party acknowledges that a connection has been established
    PeerConnected            { peer: Arc<Peer<CONFIG, LocalMessages, SenderChannel, StateType>> },
    /// Happens when the remote party disconnects
    PeerDisconnected         { peer: Arc<Peer<CONFIG, LocalMessages, SenderChannel, StateType>>, stream_stats: Arc<dyn reactive_mutiny::stream_executor::StreamExecutorStats + Sync + Send> },
    /// Happens when the local code has commanded the service (and all opened connections) to stop
    LocalServiceTermination,
}


