//! Common types used across this crate

use crate::{socket_connection::peer::Peer};
use std::{
    fmt::Debug,
    sync::Arc,
};
use futures::Stream;
use reactive_mutiny::prelude::{FullDuplexUniChannel, GenericUni, MutinyStream};
use crate::config::ConstConfig;
use crate::prelude::SocketConnection;
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


/// Event issued by Composite Protocol Clients & Servers when connections are made or dropped
#[derive(Debug)]
pub enum ConnectionEvent<'a, StateType: Send + Sync + Clone + Debug> {
    /// Happens when a connection is established with a remote party
    Connected(&'a SocketConnection<StateType>),
    /// Happens as soon as a disconnection is detected
    Disconnected(&'a SocketConnection<StateType>),
    /// Happens when the local code has commanded the service (and all opened connections) to stop
    LocalServiceTermination,
}


/// Event issued by Composite Protocol Clients & Servers to their Reactive Processors.\
/// The user code may use those events to maintain a list of connected parties, be notified of stop/close/quit requests, init/de-init sessions, etc.
/// Note that the `Peer` objects received in those events may be used, at any time, to send messages to the remote party -- like "Shutting down. Goodbye".
/// IMPLEMENTATION NOTE: GAT traits (to reduce the number of generic parameters) couldn't be used here -- even after applying this compiler bug workaround https://github.com/rust-lang/rust/issues/102211#issuecomment-1513931928
///                      -- the "error: implementation of `std::marker::Send` is not general enough" bug kept on popping up in user provided closures that called other async functions.
#[derive(Debug)]
pub enum ProtocolEvent<const CONFIG:  u64,
                       LocalMessages: ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
                       SenderChannel: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync,
                       StateType:                                                                                   Send + Sync + Clone     + Debug + 'static = ()> {
    /// Happens when a remote party is first made available to the reactive processor
    /// (caused either by a new connection or by a reactive protocol transition)
    PeerArrived { peer: Arc<Peer<CONFIG, LocalMessages, SenderChannel, StateType>> },
    /// Happens when the remote party leaves the reactive processor
    /// (caused either by a dropped connection or by a reactive protocol transition)
    PeerLeft { peer: Arc<Peer<CONFIG, LocalMessages, SenderChannel, StateType>>, stream_stats: Arc<dyn reactive_mutiny::stream_executor::StreamExecutorStats + Sync + Send> },
    /// Happens when the local code has commanded the service (and all opened connections) to stop
    LocalServiceTermination,
}

/// Implementers to add a new functionality to `Stream`s, allowing the yielded items to be sent back to the peer
pub trait ResponsiveStream<const CONFIG:        u64,
                           LocalMessagesType:   ReactiveMessagingSerializer<LocalMessagesType>                                      + Send + Sync + PartialEq + Debug,
                           SenderChannel:       FullDuplexUniChannel<ItemType=LocalMessagesType, DerivedItemType=LocalMessagesType> + Send + Sync,
                           StateType:                                                                                                 Send + Sync + Clone     + Debug> {

    /// upgrades the self `Stream` (of non-fallible & non-future input items of the `LocalMessagesType`) to another `Stream` that will send all input items to `peer`
    fn to_responsive_stream<YieldedItemType>

                           (self,
                            peer:        Arc<Peer<CONFIG, LocalMessagesType, SenderChannel, StateType>>,
                            item_mapper: impl FnMut(&LocalMessagesType, &Arc<Peer<CONFIG, LocalMessagesType, SenderChannel, StateType>>) -> YieldedItemType)

                           -> impl Stream<Item = YieldedItemType>

                           where Self: Sized + Stream<Item = LocalMessagesType>;

}