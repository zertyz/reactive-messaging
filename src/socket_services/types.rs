//! Common types used across this submodule

use crate::serde::{ReactiveMessagingDeserializer, ReactiveMessagingSerializer};
use crate::prelude::Peer;
use crate::socket_connection::connection_provider::ConnectionChannel;
use crate::types::{ProtocolEvent, MessagingMutinyStream, ResponsiveMessages};
use crate::socket_connection::connection::SocketConnection;
use std::fmt::Debug;
use std::future::Future;
use std::sync::Arc;
use futures::future::BoxFuture;
use futures::Stream;
use reactive_mutiny::prelude::{FullDuplexUniChannel, GenericUni};


/// Base trait for services running servers and clients
pub trait MessagingService<const CONFIG: u64> {
    type StateType: Send + Sync + Clone + Debug + 'static;


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
    async fn spawn_unresponsive_processor<RemoteMessages:                 ReactiveMessagingDeserializer<RemoteMessages>                                                                                                                                                                                           + Send + Sync + PartialEq + Debug + 'static,
                                          LocalMessages:                  ReactiveMessagingSerializer<LocalMessages>                                                                                                                                                                                              + Send + Sync + PartialEq + Debug + 'static,
                                          ProcessorUniType:               GenericUni<ItemType=RemoteMessages>                                                                                                                                                                                                     + Send + Sync                     + 'static,
                                          SenderChannel:                  FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages>                                                                                                                                                             + Send + Sync                     + 'static,
                                          OutputStreamItemsType:                                                                                                                                                                                                                                                    Send + Sync             + Debug + 'static,
                                          ServerStreamType:               Stream<Item=OutputStreamItemsType>                                                                                                                                                                                                      + Send                            + 'static,
                                          ConnectionEventsCallbackFuture: Future<Output=()>                                                                                                                                                                                                                       + Send                            + 'static,
                                          ConnectionEventsCallback:       Fn(/*event: */ProtocolEvent<CONFIG, LocalMessages, SenderChannel, Self::StateType>)                                                                                                                 -> ConnectionEventsCallbackFuture + Send + Sync                     + 'static,
                                          ProcessorBuilderFn:             Fn(/*server_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<CONFIG, LocalMessages, SenderChannel, Self::StateType>>, /*server_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> ServerStreamType               + Send + Sync                     + 'static>

                                         (&mut self,
                                          connection_events_callback:  ConnectionEventsCallback,
                                          dialog_processor_builder_fn: ProcessorBuilderFn)

                                         -> Result<ConnectionChannel<Self::StateType>, Box<dyn std::error::Error + Sync + Send>>;

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
    async fn spawn_responsive_processor<RemoteMessages:                  ReactiveMessagingDeserializer<RemoteMessages>                                                                                                                                                                                           + Send + Sync + PartialEq + Debug + 'static,
                                        LocalMessages:                   ReactiveMessagingSerializer<LocalMessages>                                                                                                                                                                                              + Send + Sync + PartialEq + Debug + 'static,
                                        ProcessorUniType:                GenericUni<ItemType=RemoteMessages>                                                                                                                                                                                                     + Send + Sync                     + 'static,
                                        SenderChannel:                   FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages>                                                                                                                                                             + Send + Sync                     + 'static,
                                        ServerStreamType:                Stream<Item=LocalMessages>                                                                                                                                                                                                              + Send                            + 'static,
                                        ConnectionEventsCallbackFuture:  Future<Output=()>                                                                                                                                                                                                                       + Send                            + 'static,
                                        ConnectionEventsCallback:        Fn(/*event: */ProtocolEvent<CONFIG, LocalMessages, SenderChannel, Self::StateType>)                                                                                                                 -> ConnectionEventsCallbackFuture + Send + Sync                     + 'static,
                                        ProcessorBuilderFn:              Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<CONFIG, LocalMessages, SenderChannel, Self::StateType>>, /*client_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> ServerStreamType               + Send + Sync                     + 'static>

                                       (&mut self,
                                        connection_events_callback:  ConnectionEventsCallback,
                                        dialog_processor_builder_fn: ProcessorBuilderFn)

                                       -> Result<ConnectionChannel<Self::StateType>, Box<dyn std::error::Error + Sync + Send>>

                                       where LocalMessages: ResponsiveMessages<LocalMessages>;

    /// Start the service with a single processor (after calling either [Self::spawn_unresponsive_processor()]
    /// or [Self::spawn_responsive_processor()] once) -- A.K.A. "The Single Protocol Mode".\
    /// See [Self::start_multi_protocol()] if you want a service that shares connections among
    /// different protocol processors.
    ///
    /// Starts the service using the provided `connection_channel` to distribute the connections.
    async fn start_single_protocol(&mut self, connection_channel: ConnectionChannel<Self::StateType>)
                                  -> Result<(), Box<dyn std::error::Error + Sync + Send>>
                                  where Self::StateType: Default {
        // this closure will cause incoming or just-opened connections to be sent to `connection_channel` and returned connections to be dropped
        let connection_routing_closure = move |_socket_connection: &SocketConnection<Self::StateType>, is_reused: bool|
            if is_reused {
                None
            } else {
                Some(connection_channel.clone_sender())
            };
        self.start_multi_protocol(Self::StateType::default(), connection_routing_closure).await

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
    async fn start_multi_protocol(&mut self,
                                  initial_connection_state:   Self::StateType,
                                  connection_routing_closure: impl FnMut(/*socket_connection: */&SocketConnection<Self::StateType>, /*is_reused: */bool) -> Option<tokio::sync::mpsc::Sender<SocketConnection<Self::StateType>>> + Send + 'static)
                                 -> Result<(), Box<dyn std::error::Error + Sync + Send>>;

    /// Returns an async closure that blocks until [Self::terminate()] is called.
    /// Example:
    /// ```no_compile
    ///     self.start_the_service();
    ///     self.termination_waiter()().await;
    fn termination_waiter(&mut self) -> Box< dyn FnOnce() -> BoxFuture<'static, Result<(), Box<dyn std::error::Error + Send + Sync>>> >;

    /// Notifies the service it is time to stop / shutdown / terminate.\
    /// It is a recommended practice that the `connection_events_handler()` you provided (when starting each dialog processor)
    /// inform all clients that a remote-initiated disconnection (due to the call to this function) is happening -- the protocol must support that, though.
    async fn terminate(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

}


/// For internal use: defines `ProcessorUniType` & `SenderChannel` based on the given [Channels] parameter
/// (for use when spawning processors with [MessagingService::spawn_unresponsive_processor()] &
///  [MessagingService::spawn_responsive_processor()].)
#[macro_export]
macro_rules! _define_processor_uni_and_sender_channel_types {
    ($const_config: expr, Atomic, $remote_messages: ty, $local_messages: ty) => {
        const _CONST_CONFIG:              ConstConfig  = $const_config;
        const _PROCESSOR_BUFFER:          usize        = _CONST_CONFIG.receiver_buffer as usize;
        const _PROCESSOR_UNI_INSTRUMENTS: usize        = _CONST_CONFIG.executor_instruments.into();
        const _SENDER_BUFFER:             usize        = _CONST_CONFIG.sender_buffer   as usize;
        type ProcessorUniType = UniZeroCopyAtomic<$remote_messages, _PROCESSOR_BUFFER, 1, _PROCESSOR_UNI_INSTRUMENTS>;
        type SenderChannel = ChannelUniMoveAtomic<$local_messages, _SENDER_BUFFER, 1>;
    };
    ($const_config: expr, FullSync, $remote_messages: ty, $local_messages: ty) => {
        const _CONST_CONFIG:              ConstConfig  = $const_config;
        const _PROCESSOR_BUFFER:          usize        = _CONST_CONFIG.receiver_buffer as usize;
        const _PROCESSOR_UNI_INSTRUMENTS: usize        = _CONST_CONFIG.executor_instruments.into();
        const _SENDER_BUFFER:             usize        = _CONST_CONFIG.sender_buffer   as usize;
        type ProcessorUniType = UniZeroCopyFullSync<$remote_messages, _PROCESSOR_BUFFER, 1, _PROCESSOR_UNI_INSTRUMENTS>;
        type SenderChannel = ChannelUniMoveFullSync<$local_messages, _SENDER_BUFFER, 1>;
    };
    ($const_config: expr, Crossbeam, $remote_messages: ty, $local_messages: ty) => {
        const _CONST_CONFIG:              ConstConfig  = $const_config;
        const _PROCESSOR_BUFFER:          usize        = _CONST_CONFIG.receiver_buffer as usize;
        const _PROCESSOR_UNI_INSTRUMENTS: usize        = _CONST_CONFIG.executor_instruments.into();
        const _SENDER_BUFFER:             usize        = _CONST_CONFIG.sender_buffer   as usize;
        type ProcessorUniType = UniMoveCrossbeam<$remote_messages, _PROCESSOR_BUFFER, 1, _PROCESSOR_UNI_INSTRUMENTS>;
        type SenderChannel = ChannelUniMoveCrossbeam<$local_messages, _SENDER_BUFFER, 1>;
    };
}
pub use _define_processor_uni_and_sender_channel_types;
