//! Common types used across this crate

use crate::{socket_connection::peer::Peer, ReactiveMessagingSerializer, ReactiveMessagingDeserializer};
use std::{
    fmt::Debug,
    sync::Arc,
};
use std::future::Future;
use futures::future::BoxFuture;
use futures::Stream;
use reactive_mutiny::prelude::{FullDuplexUniChannel, GenericUni, MutinyStream};
use tokio::net::TcpStream;
use crate::socket_connection::connection_provider::ConnectionChannel;


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
                         StateType:                                                                                   Send + Sync             + Debug + 'static = ()> {
    /// Happens when the remote party acknowledges that a connection has been established
    PeerConnected            { peer: Arc<Peer<CONFIG, LocalMessages, SenderChannel, StateType>> },
    /// Happens when the remote party disconnects
    PeerDisconnected         { peer: Arc<Peer<CONFIG, LocalMessages, SenderChannel, StateType>>, stream_stats: Arc<dyn reactive_mutiny::stream_executor::StreamExecutorStats + Sync + Send> },
    /// Happens when the local code has commanded the service (and all opened connections) to stop
    LocalServiceTermination,
}


/// Base trait for services running servers and clients
pub trait MessagingService<const CONFIG: u64> {
    type RemoteMessages:      ReactiveMessagingDeserializer<Self::RemoteMessages>                                     + Send + Sync + PartialEq + Debug + 'static;
    type LocalMessages:       ReactiveMessagingSerializer<Self::LocalMessages>                                        + Send + Sync + PartialEq + Debug + 'static;
    type ProcessorUniType:    GenericUni<ItemType=Self::RemoteMessages>                                               + Send + Sync                     + 'static;
    type SenderChannel:       FullDuplexUniChannel<ItemType=Self::LocalMessages, DerivedItemType=Self::LocalMessages> + Send + Sync;
    type StateType:                                                                                                     Send + Sync + Default   + Debug + 'static;

    /// Spawns a task dedicated to the given "unresponsive protocol processor", returning immediately.\
    /// The given `dialog_processor_builder_fn` will be called for each new connection and should return a `Stream`
    /// that will produce non-futures & non-fallible items that **won't be sent to the client**:
    ///   - `connection_events_callback`: -- a generic function (or closure) to handle connected, disconnected and termination events (possibly to manage sessions). Sign it as:
    ///     ```nocompile
    ///     async fn connection_events_handler<const CONFIG:  u64,
    ///                                        LocalMessages: ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
    ///                                        SenderChannel: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync>
    ///                                       (_event: ConnectionEvent<CONFIG, LocalMessages, SenderChannel>) {...}
    ///     ```
    ///   - `dialog_processor_builder_fn` -- the generic function (or closure) that receives the `Stream` of client messages and returns another `Stream`, which won't
    ///                                      be sent out to clients -- called once for each connection. Sign it as:
    ///     ```nocompile
    ///     fn unresponsive_processor<const CONFIG:   u64,
    ///                               LocalMessages:  ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
    ///                               SenderChannel:  FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync,
    ///                               StreamItemType: Deref<Target=[your type for messages produced by the CLIENT]>>
    ///                              (client_addr:            String,
    ///                               connected_port:         u16,
    ///                               peer:                   Arc<Peer<CONFIG, LocalMessages, SenderChannel>>,
    ///                               client_messages_stream: impl Stream<Item=StreamItemType>)
    ///                              -> impl Stream<Item=()> {...}
    ///     ```
    /// -- if you want the processor to produce answer messages of type `LocalMessages` to be sent to clients, see [Self::spawn_responsive_processor()]:
    async fn spawn_unresponsive_processor<OutputStreamItemsType:                                                                                                                                                                                                                                                                      Send + Sync + Debug       + 'static,
                                          ServerStreamType:               Stream<Item=OutputStreamItemsType>                                                                                                                                                                                                                        + Send                      + 'static,
                                          ConnectionEventsCallbackFuture: Future<Output=()>                                                                                                                                                                                                                                         + Send                      + 'static,
                                          ConnectionEventsCallback:       Fn(/*event: */ConnectionEvent<CONFIG, Self::LocalMessages, Self::SenderChannel, Self::StateType>)                                                                                                                       -> ConnectionEventsCallbackFuture + Send + Sync               + 'static,
                                          ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<CONFIG, Self::LocalMessages, Self::SenderChannel, Self::StateType>>, /*client_messages_stream: */MessagingMutinyStream<Self::ProcessorUniType>) -> ServerStreamType               + Send + Sync               + 'static>

                                         (&mut self,
                                          connection_events_callback:  ConnectionEventsCallback,
                                          dialog_processor_builder_fn: ProcessorBuilderFn)

                                         -> Result<ConnectionChannel, Box<dyn std::error::Error + Sync + Send>>;

    /// Spawns a task dedicated to the given "responsive protocol processor", returning immediately,
    /// The given `dialog_processor_builder_fn` will be called for each new connection and will return a `Stream`
    /// that will produce non-futures & non-fallible items that *will be sent to the client*:
    ///   - `connection_events_callback`: -- a generic function (or closure) to handle connected, disconnected and termination events (possibly to manage sessions). Sign it as:
    ///     ```nocompile
    ///     async fn connection_events_handler<const CONFIG:  u64,
    ///                                        LocalMessages: ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
    ///                                        SenderChannel: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync>
    ///                                       (_event: ConnectionEvent<CONFIG, LocalMessages, SenderChannel>) {...}
    ///     ```
    ///   - `dialog_processor_builder_fn` -- the generic function (or closure) that receives the `Stream` of remote messages and returns the `Stream` of local messages to
    ///                                      be sent to the peer -- called once for each connection. Sign it as:
    ///     ```nocompile
    ///     fn responsive_processor<const CONFIG:   u64,
    ///                             LocalMessages:  ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
    ///                             SenderChannel:  FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync,
    ///                             StreamItemType: Deref<Target=[your type for messages produced by the CLIENT]>>
    ///                            (client_addr:            String,
    ///                             connected_port:         u16,
    ///                             peer:                   Arc<Peer<CONFIG, LocalMessages, SenderChannel>>,
    ///                             client_messages_stream: impl Stream<Item=StreamItemType>)
    ///                            -> impl Stream<Item=LocalMessages> {...}
    ///     ```
    /// Notice that this method requires that `LocalMessages` implements, additionally, [ResponsiveMessages<>].\
    /// -- if you don't want the processor to produce answer messages, see [Self::spawn_unresponsive_processor()].
    async fn spawn_responsive_processor<ServerStreamType:                Stream<Item=Self::LocalMessages>                                                                                                                                                                                                                          + Send        + 'static,
                                        ConnectionEventsCallbackFuture:  Future<Output=()>                                                                                                                                                                                                                                         + Send        + 'static,
                                        ConnectionEventsCallback:        Fn(/*event: */ConnectionEvent<CONFIG, Self::LocalMessages, Self::SenderChannel, Self::StateType>)                                                                                                                       -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                        ProcessorBuilderFn:              Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<CONFIG, Self::LocalMessages, Self::SenderChannel, Self::StateType>>, /*client_messages_stream: */MessagingMutinyStream<Self::ProcessorUniType>) -> ServerStreamType               + Send + Sync + 'static>

                                       (&mut self,
                                        connection_events_callback:  ConnectionEventsCallback,
                                        dialog_processor_builder_fn: ProcessorBuilderFn)

                                       -> Result<ConnectionChannel, Box<dyn std::error::Error + Sync + Send>>

                                       where Self::LocalMessages: ResponsiveMessages<Self::LocalMessages>;

    /// Start the service with a single processor (after calling either [Self::spawn_unresponsive_processor()]
    /// or [Self::spawn_responsive_processor()] once) -- A.K.A. "The Single Protocol Mode".\
    /// See [Self::start_with_routing_closure()] if you want a service that shares connections among
    /// different protocol processors.
    ///
    /// Starts the service using the provided `connection_channel` to distribute the connections.
    async fn start_with_single_protocol(&mut self, connection_channel: ConnectionChannel)
                                       -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
        // this closure will cause incoming or just-opened connections to be sent to `connection_channel` and returned connections to be dropped
        let connection_routing_closure = move |_connection: &TcpStream, last_state: Option<Self::StateType>|
            last_state.map_or_else(|| Some(connection_channel.clone_sender()),
                                   |_| None);
        self.start_with_routing_closure(connection_routing_closure).await

    }

    /// Starts the service using the provided `connection_routing_closure` to
    /// distribute the connections among the configured processors -- previously fed in by [Self::spawn_responsive_processor()] &
    /// [Self::spawn_unresponsive_processor()].
    ///
    /// `protocol_stacking_closure := FnMut(connection: &tokio::net::TcpStream, last_state: Option<StateType>) -> connection_receiver: Option<tokio::sync::mpsc::Sender<TcpStream>>`
    ///
    /// -- this closure "decides what to do" with available connections, routing them to the appropriate processors:
    ///   - Newly received connections will have `last_state` set to `None` -- otherwise, this will either be set by the processor
    ///     before the [Peer] is closed -- see [Peer::set_state()] -- or will have the `Default` value.
    ///   - The returned value must be one of the "handles" returned by [Self::spawn_responsive_processor()] or
    ///     [Self::spawn_unresponsive_processor()].
    ///   - If `None` is returned, the connection will be closed.
    ///
    /// This method returns an error in the following cases:
    ///   1) if the connecting/binding process fails;
    ///   2) if no processors were configured.
    async fn start_with_routing_closure(&mut self,
                                        connection_routing_closure: impl FnMut(/*connection: */& tokio::net::TcpStream, /*last_state: */Option<Self::StateType>) -> Option<tokio::sync::mpsc::Sender<TcpStream>> + Send + 'static)
                                       -> Result<(), Box<dyn std::error::Error + Sync + Send>>;

    /// Returns an async closure that blocks until [Self::terminate()] is called.
    /// Example:
    /// ```no_compile
    ///     self.start_the_service();
    ///     self.termination_waiter()().await;
    fn termination_waiter(&mut self) -> Box< dyn FnOnce() -> BoxFuture<'static, Result<(), Box<dyn std::error::Error + Send + Sync>>> >;

    /// Notifies the service it is time to stop / shutdown / terminate.\
    /// It is a recommended practice that the `connection_events_handler()` you provided (when starting each dialog processor)
    /// inform all clients that a remote-initiated disconnection (due to the call to this function) is happening -- the protocol must support that, 'though.
    async fn terminate(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

}
