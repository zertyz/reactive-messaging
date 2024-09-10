//! Common types used across this submodule

use std::error::Error;
use crate::serde::{ReactiveMessagingDeserializer, ReactiveMessagingSerializer};
use crate::prelude::Peer;
use crate::socket_connection::connection_provider::ConnectionChannel;
use crate::types::{ProtocolEvent, MessagingMutinyStream, ConnectionEvent};
use crate::socket_connection::connection::SocketConnection;
use std::fmt::Debug;
use std::future;
use std::future::Future;
use std::sync::Arc;
use futures::future::BoxFuture;
use futures::Stream;
use reactive_mutiny::prelude::{FullDuplexUniChannel, GenericUni};


/// Base trait for server and client services functionalities
pub trait MessagingService<const CONFIG: u64> {
    type StateType: Send + Sync + Clone + Debug + 'static;


    /// Spawns a task dedicated to the given "protocol processor", returning immediately.\
    /// The given `dialog_processor_builder_fn` will be called for each new connection and should return a `Stream`
    /// that will produce non-futures & non-fallible items that **may be, optionally, sent to the remote party** (see [crate::prelude::ResponsiveStream]):
    ///   - `protocol_events_callback`: -- a generic function (or closure) to handle "new peer", "peer left" and "service termination" events (possibly to manage sessions). Sign it as:
    ///     ```nocompile
    ///     async fn protocol_events_handler<const CONFIG:  u64,
    ///                                      LocalMessages: ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
    ///                                      SenderChannel: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync>
    ///                                     (_event: ProtocolEvent<CONFIG, LocalMessages, SenderChannel, StateType>) {...}
    ///     ```
    ///   - `dialog_processor_builder_fn` -- the generic function (or closure) that receives the `Stream` of remote messages and returns another `Stream`, possibly yielding
    ///                                      messages of the "local" type to be sent to the remote party -- see [crate::prelude::ResponsiveStream]. Sign the processor as:
    ///     ```nocompile
    ///     fn processor<const CONFIG:   u64,
    ///                  LocalMessages:  ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
    ///                  SenderChannel:  FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync,
    ///                  StreamItemType: Deref<Target=[your type for messages produced by the REMOTE party]>>
    ///                 (remote_addr:            String,
    ///                  connected_port:         u16,
    ///                  peer:                   Arc<Peer<CONFIG, LocalMessages, SenderChannel, StateType>>,
    ///                  remote_messages_stream: impl Stream<Item=StreamItemType>)
    ///                 -> impl Stream<Item=ANY_TYPE> {...}
    ///     ```
    /// -- if you want the processor to produce answer messages of type `LocalMessages` to be sent to clients, see [Self::spawn_responsive_processor()]:
    fn spawn_processor<RemoteMessages:                ReactiveMessagingDeserializer<RemoteMessages>                                                                                                                                                                                         + Send + Sync + PartialEq + Debug + 'static,
                       LocalMessages:                 ReactiveMessagingSerializer<LocalMessages>                                                                                                                                                                                            + Send + Sync + PartialEq + Debug + 'static,
                       ProcessorUniType:              GenericUni<ItemType=RemoteMessages>                                                                                                                                                                                                   + Send + Sync                     + 'static,
                       SenderChannel:                 FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages>                                                                                                                                                           + Send + Sync                     + 'static,
                       OutputStreamItemsType:                                                                                                                                                                                                                                                 Send + Sync             + Debug + 'static,
                       RemoteStreamType:              Stream<Item=OutputStreamItemsType>                                                                                                                                                                                                    + Send                            + 'static,
                       ProtocolEventsCallbackFuture:  Future<Output=()>                                                                                                                                                                                                                     + Send                            + 'static,
                       ProtocolEventsCallback:        Fn(/*event: */ProtocolEvent<CONFIG, LocalMessages, SenderChannel, Self::StateType>)                                                                                                                   -> ProtocolEventsCallbackFuture + Send + Sync                     + 'static,
                       ProcessorBuilderFn:            Fn(/*remote_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<CONFIG, LocalMessages, SenderChannel, Self::StateType>>, /*remote_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> RemoteStreamType             + Send + Sync                     + 'static>

                      (&mut self,
                       connection_events_callback:  ProtocolEventsCallback,
                       dialog_processor_builder_fn: ProcessorBuilderFn)

                      -> impl Future<Output=Result<ConnectionChannel<Self::StateType>, Box<dyn Error + Sync + Send>>> + Send;

    /// Start the service with a single processor (after calling either [Self::spawn_unresponsive_processor()]
    /// or [Self::spawn_responsive_processor()] once) -- A.K.A. "The Single Protocol Mode".\
    /// See [Self::start_multi_protocol()] if you want a service that shares connections among
    /// different protocol processors.
    ///
    /// Starts the service using the provided `connection_channel` to distribute the connections.
    fn start_single_protocol(&mut self, connection_channel: ConnectionChannel<Self::StateType>)
                            -> impl Future<Output=Result<(), Box<dyn Error + Sync + Send>>> + Send
                            where Self: Send, Self::StateType: Default {
        async {
            // this closure will cause incoming or just-opened connections to be sent to `connection_channel` and returned connections to be dropped
            let connection_routing_closure = move |_socket_connection: &SocketConnection<Self::StateType>, is_reused: bool|
                if is_reused {
                    None
                } else {
                    Some(connection_channel.clone_sender())
                };
            // tracking the connection events is not really necessary for the "single protocol" case here, as, for this specific case, the "protocol events" contain that information already
            let connection_events_callback = |_: ConnectionEvent<'_, Self::StateType>| future::ready(());
            self.start_multi_protocol(Self::StateType::default(), connection_routing_closure, connection_events_callback).await
        }
    }

    /// Starts the service using the provided `connection_routing_closure` to distribute the connections among the configured processors
    /// -- previously fed in by [Self::spawn_responsive_processor()] & [Self::spawn_unresponsive_processor()].
    ///
    /// `protocol_stacking_closure := FnMut(socket_connection: &SocketConnection<StateType>, is_reused: bool) -> connection_receiver: Option<tokio::sync::mpsc::Sender<TcpStream>>`
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
    fn start_multi_protocol<ConnectionEventsCallbackFuture:  Future<Output=()> + Send>
                           (&mut self,
                            initial_connection_state:    Self::StateType,
                            connection_routing_closure:  impl FnMut(/*socket_connection: */&SocketConnection<Self::StateType>, /*is_reused: */bool) -> Option<tokio::sync::mpsc::Sender<SocketConnection<Self::StateType>>> + Send + 'static,
                            connection_events_callback:  impl for <'r> Fn(/*event: */ConnectionEvent<'r, Self::StateType>)                          -> ConnectionEventsCallbackFuture                                       + Send + 'static)
                           -> impl Future<Output=Result<(), Box<dyn Error + Sync + Send>>> + Send;

    /// Returns an async closure that blocks until [Self::terminate()] is called.
    /// Example:
    /// ```no_compile
    ///     self.start_the_service();
    ///     self.termination_waiter()().await;
    fn termination_waiter(&mut self) -> Box< dyn FnOnce() -> BoxFuture<'static, Result<(), Box<dyn std::error::Error + Send + Sync>>> >;

    /// Notifies the service it is time to stop / shutdown / terminate.\
    /// It is a recommended practice that the `connection_events_handler()` you provided (when starting each dialog processor)
    /// inform all clients that a remote-initiated disconnection (due to the call to this function) is happening -- the protocol must support that, though.
    fn terminate(self) -> impl Future<Output=Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send;

}


/// For internal use: defines `ProcessorUniType` & `SenderChannel` based on the given [Channels] parameter
/// (for use when spawning processors with [MessagingService::spawn_unresponsive_processor()] &
///  [MessagingService::spawn_responsive_processor()].)
#[macro_export]
macro_rules! _define_processor_uni_and_sender_channel_types {
    ($const_config: expr, Atomic, $remote_messages: ty, $local_messages: ty) => {
        const _CONST_CONFIG:              ConstConfig  = $const_config;
        const _PROCESSOR_BUFFER:          usize        = _CONST_CONFIG.receiver_channel_size as usize;
        const _PROCESSOR_UNI_INSTRUMENTS: usize        = _CONST_CONFIG.executor_instruments.into();
        const _SENDER_BUFFER:             usize        = _CONST_CONFIG.sender_channel_size   as usize;
        type ProcessorUniType = UniZeroCopyAtomic<$remote_messages, _PROCESSOR_BUFFER, 1, _PROCESSOR_UNI_INSTRUMENTS>;
        type SenderChannel = ChannelUniMoveAtomic<$local_messages, _SENDER_BUFFER, 1>;
    };
    ($const_config: expr, FullSync, $remote_messages: ty, $local_messages: ty) => {
        const _CONST_CONFIG:              ConstConfig  = $const_config;
        const _PROCESSOR_BUFFER:          usize        = _CONST_CONFIG.receiver_channel_size as usize;
        const _PROCESSOR_UNI_INSTRUMENTS: usize        = _CONST_CONFIG.executor_instruments.into();
        const _SENDER_BUFFER:             usize        = _CONST_CONFIG.sender_channel_size   as usize;
        type ProcessorUniType = UniZeroCopyFullSync<$remote_messages, _PROCESSOR_BUFFER, 1, _PROCESSOR_UNI_INSTRUMENTS>;
        type SenderChannel = ChannelUniMoveFullSync<$local_messages, _SENDER_BUFFER, 1>;
    };
    ($const_config: expr, Crossbeam, $remote_messages: ty, $local_messages: ty) => {
        const _CONST_CONFIG:              ConstConfig  = $const_config;
        const _PROCESSOR_BUFFER:          usize        = _CONST_CONFIG.receiver_channel_size as usize;
        const _PROCESSOR_UNI_INSTRUMENTS: usize        = _CONST_CONFIG.executor_instruments.into();
        const _SENDER_BUFFER:             usize        = _CONST_CONFIG.sender_channel_size   as usize;
        type ProcessorUniType = UniMoveCrossbeam<$remote_messages, _PROCESSOR_BUFFER, 1, _PROCESSOR_UNI_INSTRUMENTS>;
        type SenderChannel = ChannelUniMoveCrossbeam<$local_messages, _SENDER_BUFFER, 1>;
    };
}
pub use _define_processor_uni_and_sender_channel_types;
