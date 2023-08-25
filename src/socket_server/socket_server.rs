//! Provides the [new_socket_server!()] macro for instantiating servers, which may then be started with [spawn_unresponsive_server_processor!()] or [spawn_responsive_server_processor!()],
//! with each taking different variants of reactive processor logic.\
//! Both reactive processing logic variants will take in a `Stream` as parameter and should return another `Stream` as output:
//!   * Responsive: the reactive processor's output `Stream` yields items that are messages to be sent back to each client;
//!   * Unresponsive the reactive processor's output `Stream` won't be sent to the clients, allowing the `Stream`s to return items from any type.
//!
//! The Unresponsive processors still allow you to send messages to the client and should be preferred (as far as performance is concerned)
//! for protocols where the count of messages IN don't nearly map 1/1 with the count of messages OUT.
//!
//! For every server variant, the `Stream` items may be a combination of fallible/non-fallible (using `Result<>` or not) & future/non-future.
//!
//! Instead of using the mentioned macros, you might want take a look at [GenericSocketServer] to access the inner implementation directly
//! -- both ways have the same flexibility, but the macro version takes in all parameters in the conveniently packed and documented [ConstConfig]
//! struct, instead of requiring several const generic parameters.
//!
//! IMPLEMENTATION NOTE: There would be a common trait (that is no longer present) binding [SocketServer] to [GenericSocketServer]
//!                      -- the reason being Rust 1.71 still not supporting zero-cost async in traits.
//!                      Other API designs had been attempted, but using async in traits (through the `async-trait` crate) and the
//!                      necessary GATs to provide zero-cost-abstracts, revealed a compiler bug making writing processors very difficult to compile:
//!                        - https://github.com/rust-lang/rust/issues/96865


use std::fmt::Debug;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;
use futures::future::BoxFuture;
use futures::Stream;
use crate::socket_connection::{Peer, SocketConnectionHandler};
use crate::socket_server::common::upgrade_to_shutdown_tracking;
use crate::types::{ConnectionEvent, MessagingMutinyStream, ResponsiveMessages};
use crate::{ReactiveMessagingDeserializer, ReactiveMessagingSerializer};
use crate::socket_connection::common::{ReactiveMessagingSender, RetryableSender};
use crate::config::{Channels, ConstConfig};
use reactive_mutiny::prelude::advanced::{
    ChannelUniMoveAtomic,
    ChannelUniMoveCrossbeam,
    ChannelUniMoveFullSync,
    UniMoveCrossbeam,
    UniZeroCopyAtomic,
    UniZeroCopyFullSync,
    FullDuplexUniChannel,
    ChannelCommon,
    ChannelProducer,
    GenericUni,
};
use log::warn;


/// Instantiates & allocates resources for a [GenericSocketServer], ready to be later started by [spawn_unresponsive_server_processor!()] or [spawn_responsive_server_processor!()].\
/// Params:
///   - `const_config`: [ConstConfig] -- the configurations for the server, enforcing const time optimizations;
///   - `interface_ip: IntoString` -- the interface to listen to incoming connections;
///   - `port: u16` -- the port to listen to incoming connections;
///   - `RemoteMessages`: [ReactiveMessagingDeserializer<>] -- the type of the messages produced by the clients;
///   - `LocalMessage`: [ReactiveMessagingSerializer<>] -- the type of the messages produced by this server -- should, additionally, implement the `Default` trait.
#[macro_export]
macro_rules! new_socket_server {
    ($const_config:    expr,
     $interface_ip:    expr,
     $port:            expr,
     $remote_messages: ty,
     $local_messages:  ty) => {{
        use crate::socket_server::{SocketServer, GenericSocketServer};
        const CONFIG:                    u64   = $const_config.into();
        const PROCESSOR_BUFFER:          usize = $const_config.receiver_buffer as usize;
        const PROCESSOR_UNI_INSTRUMENTS: usize = $const_config.executor_instruments.into();
        const SENDER_BUFFER:             usize = $const_config.sender_buffer   as usize;
        match $const_config.channel {
            Channels::Atomic => SocketServer::<CONFIG,
                                               $remote_messages,
                                               $local_messages,
                                               PROCESSOR_BUFFER,
                                               PROCESSOR_UNI_INSTRUMENTS,
                                               SENDER_BUFFER>
                                            ::Atomic(GenericSocketServer::<CONFIG,
                                                                           $remote_messages,
                                                                           $local_messages,
                                                                           UniZeroCopyAtomic<$remote_messages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                                                           ChannelUniMoveAtomic<$local_messages, SENDER_BUFFER, 1> >
                                                                        ::new($interface_ip, $port) ),
            Channels::FullSync => SocketServer::<CONFIG,
                                                 $remote_messages,
                                                 $local_messages,
                                                 PROCESSOR_BUFFER,
                                                 PROCESSOR_UNI_INSTRUMENTS,
                                                 SENDER_BUFFER>
                                              ::FullSync(GenericSocketServer::<CONFIG,
                                                                               $remote_messages,
                                                                               $local_messages,
                                                                               UniZeroCopyFullSync<$remote_messages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                                                               ChannelUniMoveFullSync<$local_messages, SENDER_BUFFER, 1> >
                                                                            ::new($interface_ip, $port) ),
            Channels::Crossbeam => SocketServer::<CONFIG,
                                                  $remote_messages,
                                                  $local_messages,
                                                  PROCESSOR_BUFFER,
                                                  PROCESSOR_UNI_INSTRUMENTS,
                                                  SENDER_BUFFER>
                                               ::Crossbeam(GenericSocketServer::<CONFIG,
                                                                                 $remote_messages,
                                                                                 $local_messages,
                                                                                 UniMoveCrossbeam<$remote_messages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                                                                 ChannelUniMoveCrossbeam<$local_messages, SENDER_BUFFER, 1> >
                                                                              ::new($interface_ip, $port) ),
        }
    }}
}
pub use new_socket_server;


/// See [GenericSocketServer::spawn_unresponsive_processor()]
#[macro_export]
macro_rules! spawn_unresponsive_server_processor {
    ($socket_server:                expr,
     $connection_events_handler_fn: expr,
     $dialog_processor_builder_fn:  expr) => {{
        use crate::socket_server::SocketServer;
        match &mut $socket_server {
            SocketServer::Atomic    (generic_socket_server)       => generic_socket_server.spawn_unresponsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await,
            SocketServer::FullSync  (generic_socket_server)       => generic_socket_server.spawn_unresponsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await,
            SocketServer::Crossbeam (generic_socket_server)       => generic_socket_server.spawn_unresponsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await,
            SocketServer::BadConfig { config: _config, _phantom } => unreachable!(),
        }
    }}
}
pub use spawn_unresponsive_server_processor;


/// See [GenericSocketServer::spawn_responsive_processor()]
#[macro_export]
macro_rules! spawn_responsive_server_processor {
    ($socket_server:                expr,
     $connection_events_handler_fn: expr,
     $dialog_processor_builder_fn:  expr) => {{
        use crate::socket_server::SocketServer;
        match &mut $socket_server {
            SocketServer::Atomic    (generic_socket_server)       => generic_socket_server.spawn_responsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await,
            SocketServer::FullSync  (generic_socket_server)       => generic_socket_server.spawn_responsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await,
            SocketServer::Crossbeam (generic_socket_server)       => generic_socket_server.spawn_responsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await,
            SocketServer::BadConfig { config: _config, _phantom } => unreachable!(),
        }
    }}
}
pub use spawn_responsive_server_processor;


/// Represents a server built out of `CONFIG` (a `u64` version of [ConstConfig], from which the other const generic parameters derive).\
/// Don't instantiate this struct directly -- use [new_socket_server!()] instead.
pub enum SocketServer<const CONFIG:                    u64,
                      RemoteMessages:                  ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug           + 'static,
                      LocalMessages:                   ReactiveMessagingSerializer<LocalMessages>    + Send + Sync + PartialEq + Debug + Default + 'static,
                      const PROCESSOR_BUFFER:          usize,
                      const PROCESSOR_UNI_INSTRUMENTS: usize,
                      const SENDER_BUFFER:             usize> {

    Atomic(GenericSocketServer::<CONFIG,
                                 RemoteMessages,
                                 LocalMessages,
                                 UniZeroCopyAtomic<RemoteMessages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                 ChannelUniMoveAtomic<LocalMessages, SENDER_BUFFER, 1> >),

    FullSync(GenericSocketServer::<CONFIG,
                                   RemoteMessages,
                                   LocalMessages,
                                   UniZeroCopyFullSync<RemoteMessages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                   ChannelUniMoveFullSync<LocalMessages, SENDER_BUFFER, 1> >),

    Crossbeam(GenericSocketServer::<CONFIG,
                                    RemoteMessages,
                                    LocalMessages,
                                    UniMoveCrossbeam<RemoteMessages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                    ChannelUniMoveCrossbeam<LocalMessages, SENDER_BUFFER, 1> >),
    BadConfig {
        config:   u64,
        _phantom: PhantomData<(RemoteMessages, LocalMessages)>
    },
}
 impl<const CONFIG:                    u64,
      RemoteMessages:                  ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug           + 'static,
      LocalMessages:                   ReactiveMessagingSerializer<LocalMessages>    + Send + Sync + PartialEq + Debug + Default + 'static,
      const PROCESSOR_BUFFER:          usize,
      const PROCESSOR_UNI_INSTRUMENTS: usize,
      const SENDER_BUFFER:             usize>
 SocketServer<CONFIG, RemoteMessages, LocalMessages, PROCESSOR_BUFFER, PROCESSOR_UNI_INSTRUMENTS, SENDER_BUFFER> {

     /// See [GenericSocketServer::shutdown_waiter()]
     pub fn shutdown_waiter(&mut self) -> Box<dyn FnOnce() -> BoxFuture<'static, Result<(), Box<dyn std::error::Error + Send + Sync>>> > {
         match self {
             SocketServer::Atomic    (generic_socket_server) => Box::new(generic_socket_server.shutdown_waiter()),
             SocketServer::FullSync  (generic_socket_server) => Box::new(generic_socket_server.shutdown_waiter()),
             SocketServer::Crossbeam (generic_socket_server) => Box::new(generic_socket_server.shutdown_waiter()),
             SocketServer::BadConfig { config: _config, _phantom } => unreachable!(),
         }
     }

     /// See [GenericSocketServer::shutdown()]
     pub fn shutdown(mut self, timeout_ms: u32) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
         match self {
             SocketServer::Atomic    (generic_socket_server) => generic_socket_server.shutdown(timeout_ms),
             SocketServer::FullSync  (generic_socket_server) => generic_socket_server.shutdown(timeout_ms),
             SocketServer::Crossbeam (generic_socket_server) => generic_socket_server.shutdown(timeout_ms),
             SocketServer::BadConfig { config: _config, _phantom }               => unreachable!(),
         }
     }

}


/// Real definition & implementation for our Socket Server, full of generic parameters.\
/// Probably you want to instantiate this structure through the sugared macro [new_socket_server!()] instead.
/// Generic Parameters:
///   - `CONFIG`:           the `u64` version of the [ConstConfig] instance used to build this struct -- from which `ProcessorUniType` and `SenderChannel` derive;
///   - `RemoteMessages`:   the messages that are generated by the clients (usually an `enum`);
///   - `LocalMessages`:    the messages that are generated by the server (usually an `enum`);
///   - `ProcessorUniType`: an instance of a `reactive-mutiny`'s [Uni] type (using one of the zero-copy channels) --
///                         This [Uni] will execute the given server reactive logic for each incoming message (see how it is used in [new_socket_server!()]);
///   - `SenderChannel`:    an instance of a `reactive-mutiny`'s Uni movable `Channel`, which will provide a `Stream` of messages to be sent to the client.
pub struct GenericSocketServer<const CONFIG:        u64,
                               RemoteMessages:      ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
                               LocalMessages:       ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
                               ProcessorUniType:    GenericUni<ItemType=RemoteMessages>                                         + Send + Sync                     + 'static,
                               SenderChannel:       FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync> {

    /// The interface to listen to incoming connections
    interface_ip: String,
    /// The port to listen to incoming connections
    port: u16,
    /// Signaler to stop this server
    server_shutdown_signaler: Option<tokio::sync::oneshot::Sender<u32>>,
    /// Signaler to cause [wait_for_shutdown()] to return
    local_shutdown_receiver: Option<tokio::sync::oneshot::Receiver<()>>,
    _phantom: PhantomData<(RemoteMessages,LocalMessages,ProcessorUniType,SenderChannel)>
}
impl<const CONFIG:        u64,
     RemoteMessages:      ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:       ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:    GenericUni<ItemType=RemoteMessages>                                         + Send + Sync                     + 'static,
     SenderChannel:       FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync>
GenericSocketServer<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannel> {

    /// Creates a new server instance listening on TCP/IP:
    ///   `interface_ip`:         the interface's IP to listen to -- 0.0.0.0 will cause listening to all network interfaces
    ///   `port`:                 what port to listen to
    pub fn new<IntoString: Into<String>>
              (interface_ip: IntoString,
               port:         u16)
              -> Self {
        Self {
            interface_ip:             interface_ip.into(),
            port,
            server_shutdown_signaler: None,
            local_shutdown_receiver:  None,
            _phantom:                 PhantomData,
        }
    }

    /// Spawns a task to start the local server, returning immediately.\
    /// The given `dialog_processor_builder_fn` will be called for each new connection and should return a `Stream`
    /// that will produce non-futures & non-fallible items that **won't be sent to the client**:
    ///   - `connection_events_callback`: -- a generic function (or closure) to handle connected, disconnected and shutdown events (possibly to manage sessions). Sign it as:
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
    #[inline(always)]
    pub async fn spawn_unresponsive_processor<OutputStreamItemsType:                                                                                                                                                                                                                                   Send + Sync + Debug       + 'static,
                                              ServerStreamType:               Stream<Item=OutputStreamItemsType>                                                                                                                                                                                     + Send                      + 'static,
                                              ConnectionEventsCallbackFuture: Future<Output=()>                                                                                                                                                                                                      + Send                      + 'static,
                                              ConnectionEventsCallback:       Fn(/*event: */ConnectionEvent<CONFIG, LocalMessages, SenderChannel>)                                                                                                                 -> ConnectionEventsCallbackFuture + Send + Sync               + 'static,
                                              ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<CONFIG, LocalMessages, SenderChannel>>, /*client_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> ServerStreamType               + Send + Sync               + 'static>

                                             (&mut self,
                                              connection_events_callback:  ConnectionEventsCallback,
                                              dialog_processor_builder_fn: ProcessorBuilderFn)

                                             -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

        let (server_shutdown_sender, server_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (local_shutdown_sender, local_shutdown_receiver) = tokio::sync::oneshot::channel::<()>();
        self.server_shutdown_signaler = Some(server_shutdown_sender);
        self.local_shutdown_receiver = Some(local_shutdown_receiver);
        let listening_interface = self.interface_ip.clone();
        let port = self.port;

        let connection_events_callback = upgrade_to_shutdown_tracking(local_shutdown_sender, connection_events_callback);

        let socket_connection_handler = SocketConnectionHandler::<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannel>::new();
        socket_connection_handler.server_loop_for_unresponsive_text_protocol(listening_interface.clone(),
                                                                             port,
                                                                             server_shutdown_receiver,
                                                                             connection_events_callback,
                                                                             dialog_processor_builder_fn).await
            .map_err(|err| format!("Error starting an unresponsive GenericSocketServer @ {listening_interface}:{port}: {:?}", err))?;
        Ok(())
    }

    /// Spawns a task to start the local server, returning immediately,
    /// The given `dialog_processor_builder_fn` will be called for each new connection and will return a `Stream`
    /// that will produce non-futures & non-fallible items that *will be sent to the client*:
    ///   - `connection_events_callback`: -- a generic function (or closure) to handle connected, disconnected and shutdown events (possibly to manage sessions). Sign it as:
    ///     ```nocompile
    ///     async fn connection_events_handler<const CONFIG:  u64,
    ///                                        LocalMessages: ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
    ///                                        SenderChannel: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync>
    ///                                       (_event: ConnectionEvent<CONFIG, LocalMessages, SenderChannel>) {...}
    ///     ```
    ///   - `dialog_processor_builder_fn` -- the generic function (or closure) that receives the `Stream` of client messages and returns the `Stream` of server messages to
    ///                                      be sent to the clients -- called once for each connection. Sign it as:
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
    pub async fn spawn_responsive_processor<ServerStreamType:                Stream<Item=LocalMessages>                                                                                                                                                                                             + Send        + 'static,
                                            ConnectionEventsCallbackFuture:  Future<Output=()>                                                                                                                                                                                                      + Send        + 'static,
                                            ConnectionEventsCallback:        Fn(/*event: */ConnectionEvent<CONFIG, LocalMessages, SenderChannel>)                                                                                                                 -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                            ProcessorBuilderFn:              Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<CONFIG, LocalMessages, SenderChannel>>, /*client_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> ServerStreamType               + Send + Sync + 'static>

                                           (&mut self,
                                            connection_events_callback:  ConnectionEventsCallback,
                                            dialog_processor_builder_fn: ProcessorBuilderFn)

                                           -> Result<(), Box<dyn std::error::Error + Sync + Send>>

                                           where LocalMessages: ResponsiveMessages<LocalMessages> {

        let (server_shutdown_sender, server_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (local_shutdown_sender, local_shutdown_receiver) = tokio::sync::oneshot::channel::<()>();
        self.server_shutdown_signaler = Some(server_shutdown_sender);
        self.local_shutdown_receiver = Some(local_shutdown_receiver);
        let listening_interface = self.interface_ip.clone();
        let port = self.port;

        let connection_events_callback = upgrade_to_shutdown_tracking(local_shutdown_sender, connection_events_callback);

        let socket_connection_handler = SocketConnectionHandler::<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannel>::new();
        socket_connection_handler.server_loop_for_responsive_text_protocol
            (listening_interface.clone(),
             port,
             server_shutdown_receiver,
             connection_events_callback,
             dialog_processor_builder_fn).await
            .map_err(|err| format!("Error starting a responsive GenericSocketServer @ {listening_interface}:{port}: {:?}", err))?;
        Ok(())
    }

    /// Returns an async closure that blocks until [Self::shutdown()] is called.
    /// Example:
    /// ```no_compile
    ///     self.spawn_the_server(logic...);
    ///     self.shutdown_waiter()().await;
    pub fn shutdown_waiter(&mut self) -> impl FnOnce() -> BoxFuture<'static, Result<(), Box<dyn std::error::Error + Send + Sync>>> {
        let mut local_shutdown_receiver = self.local_shutdown_receiver.take();
        move || Box::pin({
            async move {
                if let Some(local_shutdown_receiver) = local_shutdown_receiver.take() {
                    match local_shutdown_receiver.await {
                        Ok(()) => {
                            Ok(())
                        },
                        Err(err) => Err(Box::from(format!("GenericSocketServer::wait_for_shutdown(): It is no longer possible to tell when the server will be shutdown: `one_shot` signal error: {err}")))
                    }
                } else {
                    Err(Box::from("GenericSocketServer: \"wait for shutdown\" requested, but the server was not started (or a previous shutdown was commanded) at the moment `shutdown_waiter()` was called"))
                }
            }
        })
    }

    /// Notifies the server it is time to shutdown.\
    /// A shutdown is considered graceful if it could be accomplished in less than `timeout_ms` milliseconds.\
    /// It is a good practice that the `connection_events_handler()` you provided when starting the server
    /// uses this time to inform all clients that a remote-initiated disconnection (due to a shutdown) is happening.
    pub fn shutdown(mut self, timeout_ms: u32) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match self.server_shutdown_signaler.take() {
            Some(server_sender) => {
                warn!("GenericSocketServer: Shutdown asked & initiated for server @ {}:{} -- timeout: {timeout_ms}ms", self.interface_ip, self.port);
                if let Err(_sent_value) = server_sender.send(timeout_ms) {
                    Err(Box::from("GenericSocketServer BUG: couldn't send shutdown signal to the network loop. Program is, likely, hanged. Please, investigate and fix!"))
                } else {
                    Ok(())
                }
            }
            None => {
                Err(Box::from("GenericSocketServer: Shutdown requested, but the service was not started. Ignoring..."))
            }
        }
    }

}


/// Unit tests the [socket_server](self) module
#[cfg(any(test,doc))]
mod tests {
    use super::*;
    use std::future;
    use std::ops::Deref;
    use std::sync::atomic::AtomicU32;
    use std::sync::atomic::Ordering::Relaxed;
    use std::time::Duration;
    use futures::StreamExt;
    use serde::{Deserialize, Serialize};
    use tokio::sync::Mutex;
    use crate::{new_socket_client, ron_deserializer, ron_serializer, spawn_responsive_client_processor};


    /// Test that our instantiation macro is able to produce servers backed by all possible channel types
    #[cfg_attr(not(doc),test)]
    fn instantiation() {
        let atomic_server = new_socket_server!(
            ConstConfig {
                channel: Channels::Atomic,
                ..ConstConfig::default()
            },
            "127.0.0.1", 8040, String, String);
        assert!(matches!(atomic_server, SocketServer::Atomic(_)), "an Atomic Server couldn't be instantiated");

        let fullsync_server  = new_socket_server!(
            ConstConfig {
                channel: Channels::FullSync,
                ..ConstConfig::default()
            },
            "127.0.0.1", 8041, String, String);
        assert!(matches!(fullsync_server, SocketServer::FullSync(_)), "a FullSync Server couldn't be instantiated");

        let crossbeam_server = new_socket_server!(
            ConstConfig {
                channel: Channels::Crossbeam,
                ..ConstConfig::default()
            },
            "127.0.0.1", 8042, String, String);
        assert!(matches!(crossbeam_server, SocketServer::Crossbeam(_)), "a Crossbeam Server couldn't be instantiated");
    }

    /// Test that our types can be compiled & instantiated & are ready for usage
    #[cfg_attr(not(doc),tokio::test)]
    async fn doc_usage() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

        // demonstrates how to build an unresponsive server
        ///////////////////////////////////////////////////
        // using fully typed generic functions that will work with all possible configs
        let mut server = new_socket_server!(
            ConstConfig::default(),
            "127.0.0.1",
            8043,
            DummyClientAndServerMessages,
            DummyClientAndServerMessages);
        spawn_unresponsive_server_processor!(server,
            connection_events_handler,
            unresponsive_processor
        )?;
        async fn connection_events_handler<const CONFIG:  u64,
                                           LocalMessages: ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
                                           SenderChannel: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync>
                                          (_event: ConnectionEvent<CONFIG, LocalMessages, SenderChannel>) {
        }
        fn unresponsive_processor<const CONFIG:   u64,
                                  LocalMessages:  ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
                                  SenderChannel:  FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync,
                                  StreamItemType: Deref<Target=DummyClientAndServerMessages>>
                                 (client_addr:            String,
                                  connected_port:         u16,
                                  peer:                   Arc<Peer<CONFIG, LocalMessages, SenderChannel>>,
                                  client_messages_stream: impl Stream<Item=StreamItemType>)
                                 -> impl Stream<Item=()> {
            client_messages_stream.map(|payload| ())
        }
        let shutdown_waiter = server.shutdown_waiter();
        server.shutdown(200)?;
        shutdown_waiter().await?;

        // demonstrates how to build an responsive server
        /////////////////////////////////////////////////
        // using fully typed generic functions that will work with all possible configs
        let mut server = new_socket_server!(
            ConstConfig::default(),
            "127.0.0.1",
            8043,
            DummyClientAndServerMessages,
            DummyClientAndServerMessages);
        spawn_responsive_server_processor!(server,
            connection_events_handler,
            responsive_processor
        )?;
        fn responsive_processor<const CONFIG:   u64,
                                SenderChannel:  FullDuplexUniChannel<ItemType=DummyClientAndServerMessages, DerivedItemType=DummyClientAndServerMessages> + Send + Sync,
                                StreamItemType: Deref<Target=DummyClientAndServerMessages>>
                               (client_addr:            String,
                                connected_port:         u16,
                                peer:                   Arc<Peer<CONFIG, DummyClientAndServerMessages, SenderChannel>>,
                                client_messages_stream: impl Stream<Item=StreamItemType>)
                               -> impl Stream<Item=DummyClientAndServerMessages> {
            client_messages_stream.map(|_payload| DummyClientAndServerMessages::FloodPing)
        }
        let shutdown_waiter = server.shutdown_waiter();
        server.shutdown(200)?;
        shutdown_waiter().await?;

        // demonstrates how to use it with closures -- also allowing for any channel in the configs
        ///////////////////////////////////////////////////////////////////////////////////////////
        let mut server = new_socket_server!(
            ConstConfig::default(),
            "127.0.0.1",
            8043,
            DummyClientAndServerMessages,
            DummyClientAndServerMessages);
        spawn_unresponsive_server_processor!(server,
            |_| future::ready(()),
            |_, _, _, client_messages_stream| client_messages_stream.map(|payload| DummyClientAndServerMessages::FloodPing)
        )?;
        let shutdown_waiter = server.shutdown_waiter();
        server.shutdown(200)?;
        shutdown_waiter().await?;

        // demonstrates how to use the internal & generic implementation
        ////////////////////////////////////////////////////////////////
        // notice there may be a discrepancy in the `ConstConfig` you provide and the actual concrete types
        // you also provide for `UniProcessor` and `SenderChannel` -- therefore, this usage is not recommended
        const CONFIG: ConstConfig = ConstConfig {
            receiver_buffer:      2048,
            sender_buffer:        1024,
            channel:              Channels::FullSync,
            executor_instruments: reactive_mutiny::prelude::Instruments::LogsWithExpensiveMetrics,
            ..ConstConfig::default()
        };
        type ProcessorUniType = UniZeroCopyFullSync<DummyClientAndServerMessages, {CONFIG.receiver_buffer as usize}, 1, {CONFIG.executor_instruments.into()}>;
        type SenderChannelType = ChannelUniMoveFullSync<DummyClientAndServerMessages, {CONFIG.sender_buffer as usize}, 1>;
        let mut server = GenericSocketServer :: <{CONFIG.into()},
                                                                     DummyClientAndServerMessages,
                                                                     DummyClientAndServerMessages,
                                                                     ProcessorUniType,
                                                                     SenderChannelType>
                                                                 :: new("127.0.0.1", 8043);
        server.spawn_unresponsive_processor(
            |_| future::ready(()),
            |_, _, _, client_messages_stream| client_messages_stream.map(|payload| DummyClientAndServerMessages::FloodPing)
        ).await?;
        let shutdown_waiter = server.shutdown_waiter();
        server.shutdown(200)?;
        shutdown_waiter().await?;

        Ok(())
    }

    /// assures the shutdown process is able to:
    ///   1) communicate with all clients
    ///   2) wait for up to the given timeout for them to gracefully disconnect
    ///   3) forcibly disconnect, if needed
    ///   4) notify any waiter on the server (after all the above steps are done) within the given timeout
    #[cfg_attr(not(doc),tokio::test(flavor = "multi_thread"))]
    async fn shutdown_process() {
        const PORT: u16 = 8044;

        // the shutdown timeout, in milliseconds
        let expected_max_shutdown_duration_ms = 543;
        // the tollerance, in milliseconds -- a too small shutdown duration means the server didn't wait for the client's disconnection; too much (possibly eternal) means it didn't enforce the timeout
        let tollerance_ms = 20;

        // sensors
        let client_received_messages_count_ref1 = Arc::new(AtomicU32::new(0));
        let client_received_messages_count_ref2 = Arc::clone(&client_received_messages_count_ref1);
        let server_received_messages_count_ref1 = Arc::new(AtomicU32::new(0));
        let server_received_messages_count_ref2 = Arc::clone(&server_received_messages_count_ref1);

        // start the server -- the test logic is here
        let client_peer_ref1 = Arc::new(Mutex::new(None));
        let client_peer_ref2 = Arc::clone(&client_peer_ref1);

        const CONFIG: ConstConfig = ConstConfig {
            channel: Channels::FullSync,
            ..ConstConfig::default()
        };
        type ProcessorUniType = UniZeroCopyFullSync<DummyClientAndServerMessages, {CONFIG.receiver_buffer as usize}, 1, {CONFIG.executor_instruments.into()}>;
        type SenderChannelType = ChannelUniMoveFullSync<DummyClientAndServerMessages, {CONFIG.sender_buffer as usize}, 1>;
        let mut server = GenericSocketServer :: <{CONFIG.into()},
            DummyClientAndServerMessages,
            DummyClientAndServerMessages,
                                                                     ProcessorUniType,
                                                                     SenderChannelType >
                                                                 :: new("127.0.0.1", PORT);
        server.spawn_responsive_processor(
            move |connection_event: ConnectionEvent<{CONFIG.into()}, DummyClientAndServerMessages, SenderChannelType>| {
                let client_peer = Arc::clone(&client_peer_ref1);
                async move {
                    match connection_event {
                        ConnectionEvent::PeerConnected { peer } => {
                            // register the client -- which will initiate the server shutdown further down in this test
                            client_peer.lock().await.replace(peer);
                        },
                        ConnectionEvent::PeerDisconnected { peer: _, stream_stats: _ } => (),
                        ConnectionEvent::ApplicationShutdown { timeout_ms } => {
                            // send a message to the client (the first message, actually... that will initiate a flood of back-and-forth messages)
                            // then try to close the connection (which would only be gracefully done once all messages were sent... which may never happen).
                            let client_peer = client_peer.lock().await;
                            let client_peer = client_peer.as_ref().expect("No client is connected");
                            // send the flood starting message
                            let _ = client_peer.send_async(DummyClientAndServerMessages::FloodPing).await;
                            client_peer.flush_and_close(Duration::from_millis(timeout_ms as u64)).await;
                            // guarantees this operation will take slightly more than the timeout+tolerance to complete
                            tokio::time::sleep(Duration::from_millis((timeout_ms+/*tollerance_ms+*/20+10) as u64)).await;
                        }
                    }
                }
            },
            move |_, _, _, client_messages: MessagingMutinyStream<ProcessorUniType>| {
                let server_received_messages_count = Arc::clone(&server_received_messages_count_ref1);
                client_messages.map(move |client_message| {
                    std::mem::forget(client_message);   // TODO 2023-07-15: investigate this reactive-mutiny related bug: it seems OgreUnique doesn't like the fact that this type doesn't need dropping? (no internal strings)... or is it a reactive-messaging bug?
                    server_received_messages_count.fetch_add(1, Relaxed);
                    DummyClientAndServerMessages::FloodPing
                })
            }
        ).await.expect("Starting the server");

        // start a client that will engage in a flood ping with the server when provoked (never closing the connection)
        let mut client = new_socket_client!(
            ConstConfig::default(),
            "127.0.0.1",
            PORT,
            DummyClientAndServerMessages,
            DummyClientAndServerMessages);
        spawn_responsive_client_processor!(
            client,
            |_| async {},
            move |_, _, _, server_messages| {
                let client_received_messages_count = Arc::clone(&client_received_messages_count_ref1);
                server_messages.map(move |server_message| {
                    std::mem::forget(server_message);   // TODO 2023-07-15: investigate this reactive-mutiny related bug: it seems OgreUnique doesn't like the fact that this type doesn't need dropping? (no internal strings)... or is it a reactive-messaging bug?
                    client_received_messages_count.fetch_add(1, Relaxed);
                    DummyClientAndServerMessages::FloodPing
                })
            }
        ).expect("Starting the client");

        // wait for the client to connect
        while client_peer_ref2.lock().await.is_none() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        // shutdown the server & wait until the shutdown process is complete
        let wait_for_server_shutdown = server.shutdown_waiter();
        server.shutdown(expected_max_shutdown_duration_ms)
            .expect("Signaling the server of the shutdown intention");
        let start = std::time::SystemTime::now();
        wait_for_server_shutdown().await
            .expect("Waiting for the server to live it's life and to complete the shutdown process");
        let elapsed_ms = start.elapsed().unwrap().as_millis();
        assert!(client_received_messages_count_ref2.load(Relaxed) > 1, "The client didn't receive any messages (not even the 'server is shutting down' notification)");
        assert!(server_received_messages_count_ref2.load(Relaxed) > 1, "The server didn't receive any messages (not even 'gracefully disconnecting' after being notified that the server is shutting down)");
        assert!(elapsed_ms.abs_diff(expected_max_shutdown_duration_ms as u128) < tollerance_ms as u128,
                "The server shutdown (of a never compling client) didn't complete in a reasonable time, meaning the shutdown code is wrong. Timeout: {}ms; Tollerance: {}ms; Measured Time: {}ms",
                expected_max_shutdown_duration_ms, tollerance_ms, elapsed_ms);
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
    enum DummyClientAndServerMessages {
        #[default]
        FloodPing,
    }

    impl Deref for DummyClientAndServerMessages {
        type Target = DummyClientAndServerMessages;
        fn deref(&self) -> &Self::Target {
            self
        }
    }

    impl ReactiveMessagingSerializer<DummyClientAndServerMessages> for DummyClientAndServerMessages {
        #[inline(always)]
        fn serialize(remote_message: &DummyClientAndServerMessages, buffer: &mut Vec<u8>) {
            ron_serializer(remote_message, buffer)
                .expect("unresponsive_socket_server.rs unit tests: No errors should have happened here!")
        }
        #[inline(always)]
        fn processor_error_message(err: String) -> DummyClientAndServerMessages {
            panic!("unresponsive_socket_server.rs unit tests: protocol error when none should have happened: {err}");
        }
    }
    impl ResponsiveMessages<DummyClientAndServerMessages> for DummyClientAndServerMessages {
        #[inline(always)]
        fn is_disconnect_message(_processor_answer: &DummyClientAndServerMessages) -> bool {
            false
        }
        #[inline(always)]
        fn is_no_answer_message(_processor_answer: &DummyClientAndServerMessages) -> bool {
            false
        }
    }
    impl ReactiveMessagingDeserializer<DummyClientAndServerMessages> for DummyClientAndServerMessages {
        #[inline(always)]
        fn deserialize(local_message: &[u8]) -> Result<DummyClientAndServerMessages, Box<dyn std::error::Error + Sync + Send>> {
            ron_deserializer(local_message)
        }
    }
}