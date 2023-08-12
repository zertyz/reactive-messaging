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
use crate::prelude::MessagingMutinyStream;
use crate::socket_connection_handler::Peer;
use crate::types::ConnectionEvent;


/// Contract for spawning a unresponsive server with the given connection & dialog processor functions, returning a controller for the running daemon
#[async_trait]
pub trait ServerUnresponsiveProcessor: UnresponsiveSocketServerGenericTypes {

    const CONST_CONFIG:              ConstConfig;

    /// Spawns a task to run a Server listening @ `self`'s `interface_ip` & `port` and returns, immediately,
    /// an object through which the caller may inquire some stats (if opted in) and request the server to shutdown.\
    /// The given `dialog_processor_builder_fn` will be called for each new client and will return a `reactive-mutiny` Stream
    /// that will produce non-futures & non-fallibles items that won't be sent to the clients
    /// -- if you want the processor to produce "answer messages" to the clients, see [SocketServer::spawn_responsive_processor()].
    async fn spawn_unresponsive_processor<OutputStreamItemsType:                                                                                                                                                                                                                Send + Sync + Debug + 'static,
                                          ServerStreamType:               Stream<Item=OutputStreamItemsType>                                                                                                                                                                  + Send                + 'static,
                                          ConnectionEventsCallbackFuture: Future<Output=()>                                                                                                                                                                                   + Send,
                                          ProcessorUniType:               GenericUni<ItemType=Self::RemoteMessages>                                                                                                                                                           + Send + Sync         + 'static,
                                          SenderChannelType:              FullDuplexUniChannel<ItemType=Self::LocalMessages, DerivedItemType=Self::LocalMessages>                                                                                                             + Sync + Send         + 'static,
                                          ConnectionEventsCallback:       Fn(/*server_event: */ConnectionEvent<SenderChannelType>)                                                                                                          -> ConnectionEventsCallbackFuture + Send + Sync         + 'static,
                                          ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<SenderChannelType>>, /*client_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> ServerStreamType               + Send + Sync         + 'static>

                                         (self,
                                          connection_events_callback:  ConnectionEventsCallback,
                                          dialog_processor_builder_fn: ProcessorBuilderFn)

                                         -> Result<Arc<dyn SocketServerController>, Box<dyn std::error::Error + Sync + Send + 'static>>;

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
    fn shutdown(self: Box<Self>, timeout_ms: u32) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}


/// Fills in the same purpose as [UnresponsiveSocketServerGenericTypes], but provides additional types
/// that are filled in only by the [CustomUnresponsiveSocketServer]
pub trait UnresponsiveSocketServerAssociatedTypes: UnresponsiveSocketServerGenericTypes + UnresponsiveSocketServerGenericTypes {
    type ProcessorUniType:    GenericUni<ItemType=Self::RemoteMessages>                                               + Send + Sync + 'static;
    type SenderChannelType:   FullDuplexUniChannel<ItemType=Self::LocalMessages, DerivedItemType=Self::LocalMessages> + Sync + Send + 'static;
    type ConnectionEventType;
}

/// Helps to infer some types used by [UnresponsiveSocketServer] implementations -- also helping to simplify Generic Programming when used as a Generic Parameter.\
/// See also [ResponsiveSocketServerAssociatedTypes]
/// ```nocompile
///     type THE_TYPE_I_WANT = <SocketServer<...> as GenericSocketServer>::THE_TYPE_YOU_WANT
pub trait UnresponsiveSocketServerGenericTypes {
    type RemoteMessages:      ReactiveMessagingDeserializer<Self::RemoteMessages>                                     + Send + Sync + PartialEq + Debug + 'static;
    type LocalMessages:       ReactiveMessagingSerializer<Self::LocalMessages>                                        + Send + Sync + PartialEq + Debug + 'static;
    type StreamItemType;
    type StreamType;
}

/// Helps to infer some types used by [ResponsiveSocketServer] implementations -- also helping to simplify Generic Programming when used as a Generic Parameter.\
pub trait ResponsiveSocketServerAssociatedTypes {
    type RemoteMessages:      ReactiveMessagingDeserializer<Self::RemoteMessages>                                     + Send + Sync + PartialEq + Debug + 'static;
    type LocalMessages:       ReactiveMessagingSerializer<Self::LocalMessages>                                        +
                              ResponsiveMessages<Self::LocalMessages>                                                 + Send + Sync + PartialEq + Debug + 'static;
    type ProcessorUniType:    GenericUni<ItemType=Self::RemoteMessages>                                               + Send + Sync + 'static;
    type SenderChannelType:   FullDuplexUniChannel<ItemType=Self::LocalMessages, DerivedItemType=Self::LocalMessages> + Sync + Send + 'static;
    type ConnectionEventType;
    type StreamItemType;
    type StreamType;
}
