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


/// Helps to infer some types used by [UnresponsiveSocketServer] implementations -- also helping to simplify Generic Programming when used as a Generic Parameter.\
pub trait UnresponsiveSocketServerAssociatedTypes {
    type RemoteMessages:      ReactiveMessagingDeserializer<Self::RemoteMessages>                                     + Send + Sync + PartialEq + Debug + 'static;
    type LocalMessages:       ReactiveMessagingSerializer<Self::LocalMessages>                                        + Send + Sync + PartialEq + Debug + 'static;
    type ProcessorUniType:    GenericUni<ItemType=Self::RemoteMessages>                                               + Send + Sync + 'static;
    type SenderChannelType:   FullDuplexUniChannel<ItemType=Self::LocalMessages, DerivedItemType=Self::LocalMessages> + Sync + Send + 'static;
    type ConnectionEventType;
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
