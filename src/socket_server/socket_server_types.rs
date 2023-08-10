//! Types used across the socket server module, including some that might be exported outside.

use crate::{ReactiveMessagingDeserializer, ReactiveMessagingSerializer, ResponsiveMessages};
use std::fmt::Debug;
use std::future::Future;
use std::sync::Arc;
use async_trait::async_trait;
use futures::future::BoxFuture;
use futures::Stream;
use reactive_mutiny::prelude::{FullDuplexUniChannel, GenericUni};
use crate::config::ConstConfig;
use crate::socket_connection_handler::Peer;


/// Contract for spawning a unresponsive server with the given connection & dialog processor functions, returning a controller for the running daemon
#[async_trait]
pub trait ServerUnresponsiveProcessor<const PROCESSOR_UNI_INSTRUMENTS: usize>: UnresponsiveSocketServerAssociatedTypes<PROCESSOR_UNI_INSTRUMENTS> {

    type ServerController: SocketServerController;

    /// Spawns a task to run a Server listening @ `self`'s `interface_ip` & `port` and returns, immediately,
    /// an object through which the caller may inquire some stats (if opted in) and request the server to shutdown.\
    /// The given `dialog_processor_builder_fn` will be called for each new client and will return a `reactive-mutiny` Stream
    /// that will produce non-futures & non-fallibles items that won't be sent to the clients
    /// -- if you want the processor to produce "answer messages" to the clients, see [SocketServer::spawn_responsive_processor()].
    async fn spawn_unresponsive_processor<const CONFIG: usize,
                                          OutputStreamItemsType:                                                          Send + Sync             + Debug + 'static,
                                          ServerStreamType:               Stream<Item=OutputStreamItemsType>            + Send + 'static,
                                          ConnectionEventsCallbackFuture: Future<Output=()>                             + Send,
                                          ConnectionEventsCallback:       Fn(/*server_event: */<Self as UnresponsiveSocketServerAssociatedTypes<PROCESSOR_UNI_INSTRUMENTS>>::ConnectionEventType)                                                                                                                                                                          -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                          ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<<Self as UnresponsiveSocketServerAssociatedTypes<PROCESSOR_UNI_INSTRUMENTS>>::SenderChannelType>>, /*client_messages_stream: */<Self as UnresponsiveSocketServerAssociatedTypes<PROCESSOR_UNI_INSTRUMENTS>>::StreamType) -> ServerStreamType               + Send + Sync + 'static>

                                         (mut self,
                                          connection_events_callback:  ConnectionEventsCallback,
                                          dialog_processor_builder_fn: ProcessorBuilderFn)

                                         -> Result<Self::ServerController, Box<dyn std::error::Error + Sync + Send + 'static>>;

}

/// Contract for spawning a responsive server with the given connection & dialog processor functions, returning a controller for the running daemon
#[async_trait]
pub trait ServerResponsiveProcessor<const PROCESSOR_UNI_INSTRUMENTS: usize>: ResponsiveSocketServerAssociatedTypes<PROCESSOR_UNI_INSTRUMENTS> {

    const CONST_CONFIG: ConstConfig;

    /// Spawns a task to run a Server listening @ `self`'s `interface_ip` & `port` and returns, immediately,
    /// an object through which the caller may inquire some stats (if opted in) and request the server to shutdown.\
    /// The given `dialog_processor_builder_fn` will be called for each new client and will return a `reactive-mutiny` Stream
    /// that will produce non-futures & non-fallibles `ServerMessages` that will be sent to the clients.
    async fn spawn_responsive_processor<const CONFIG: usize,
                                        ServerStreamType:                Stream<Item=<Self as ResponsiveSocketServerAssociatedTypes<PROCESSOR_UNI_INSTRUMENTS>>::LocalMessages>                                                                                                                                                                                                                         + Send + 'static,
                                        ConnectionEventsCallbackFuture:  Future<Output=()>                                                                                                                                                                                                                                                                                                              + Send,
                                        ConnectionEventsCallback:        Fn(/*server_event: */<Self as ResponsiveSocketServerAssociatedTypes<PROCESSOR_UNI_INSTRUMENTS>>::ConnectionEventType)                                                                                                                                                                        -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                        ProcessorBuilderFn:              Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<<Self as ResponsiveSocketServerAssociatedTypes<PROCESSOR_UNI_INSTRUMENTS>>::SenderChannelType>>, /*client_messages_stream: */<Self as ResponsiveSocketServerAssociatedTypes<PROCESSOR_UNI_INSTRUMENTS>>::StreamType) -> ServerStreamType               + Send + Sync + 'static>

                                       (mut self,
                                        connection_events_callback:  ConnectionEventsCallback,
                                        dialog_processor_builder_fn: ProcessorBuilderFn)

                                       -> Result<dyn SocketServerController, Box<dyn std::error::Error + Sync + Send + 'static>>;
}


/// Controls a [SocketServer] after it has been started -- shutting down, asking for stats, ....
pub trait SocketServerController {
    /// Returns an async closure that blocks until [Self::shutdown()] is called.
    /// Example:
    /// ```no_compile
    ///     self.shutdown_waiter()().await;
    fn shutdown_waiter(&mut self) -> Box<dyn FnOnce() -> BoxFuture<'static, Result<(), Box<dyn std::error::Error + Send + Sync>>> >;

    /// Notifies the server it is time to shutdown.\
    /// A shutdown is considered graceful if it could be accomplished in less than `timeout_ms` milliseconds.\
    /// It is a good practice that the `connection_events_handler()` you provided when starting the server
    /// uses this time to inform all clients that a server-initiated disconnection (due to a shutdown) is happening.
    fn shutdown(self, timeout_ms: u32) -> Result<(), Box<dyn std::error::Error>>;
}


/// Helps to infer some types used by [UnresponsiveSocketServer] implementations -- also helping to simplify Generic Programming when used as a Generic Parameter.\
/// See also [ResponsiveSocketServerAssociatedTypes]
/// ```nocompile
///     type THE_TYPE_I_WANT = <SocketServer<...> as GenericSocketServer>::THE_TYPE_YOU_WANT
pub trait UnresponsiveSocketServerAssociatedTypes<const PROCESSOR_UNI_INSTRUMENTS: usize> {
    const PROCESSOR_UNI_INSTRUMENTS: usize;
    type RemoteMessages:      ReactiveMessagingDeserializer<Self::RemoteMessages>                                     + Send + Sync + PartialEq + Debug + 'static;
    type LocalMessages:       ReactiveMessagingSerializer<Self::LocalMessages>                                        + Send + Sync + PartialEq + Debug + 'static;
    type ProcessorUniType:    GenericUni<PROCESSOR_UNI_INSTRUMENTS, ItemType=Self::RemoteMessages>                    + Send + Sync + 'static;
    type SenderChannelType:   FullDuplexUniChannel<ItemType=Self::LocalMessages, DerivedItemType=Self::LocalMessages> + Sync + Send + 'static;
    type ConnectionEventType;
    type StreamItemType;
    type StreamType;
}

/// Helps to infer some types used by [ResponsiveSocketServer] implementations -- also helping to simplify Generic Programming when used as a Generic Parameter.\
/// See also [UnresponsiveSocketServerAssociatedTypes]
/// ```nocompile
///     type THE_TYPE_I_WANT = <SocketServer<...> as GenericSocketServer>::THE_TYPE_YOU_WANT
pub trait ResponsiveSocketServerAssociatedTypes<const PROCESSOR_UNI_INSTRUMENTS: usize> {
    const PROCESSOR_UNI_INSTRUMENTS: usize;
    type RemoteMessages:      ReactiveMessagingDeserializer<Self::RemoteMessages>                                     + Send + Sync + PartialEq + Debug + 'static;
    type LocalMessages:       ReactiveMessagingSerializer<Self::LocalMessages>                                        +
                              ResponsiveMessages<Self::LocalMessages>                                                 + Send + Sync + PartialEq + Debug + 'static;
    type ProcessorUniType:    GenericUni<PROCESSOR_UNI_INSTRUMENTS, ItemType=Self::RemoteMessages>                    + Send + Sync + 'static;
    type SenderChannelType:   FullDuplexUniChannel<ItemType=Self::LocalMessages, DerivedItemType=Self::LocalMessages> + Sync + Send + 'static;
    type ConnectionEventType;
    type StreamItemType;
    type StreamType;
}
