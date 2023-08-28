//! Provides the [new_socket_client!()] macro for instantiating clients, which may then be started with [spawn_unresponsive_client_processor!()] or [spawn_responsive_processor!()],
//! with each taking different variants of reactive processor logic.\
//! Both reactive processing logic variants will take in a `Stream` as parameter and should return another `Stream` as output:
//!   * Responsive: the reactive processor's output `Stream` yields items that are messages to be sent back to the server;
//!   * Unresponsive the reactive processor's output `Stream` won't be sent to the server, allowing the `Stream`s to return items from any type.
//!
//! The Unresponsive processors still allow you to send messages to the server and should be preferred (as far as performance is concerned)
//! for protocols where the count of messages IN don't nearly map 1/1 with the count of messages OUT.
//!
//! For every client variant, the `Stream` items may be a combination of fallible/non-fallible (using `Result<>` or not) & future/non-future.
//!
//! Instead of using the mentioned macros, you might want take a look at [GenericSocketClient] to access the inner implementation directly
//! -- both ways have the same flexibility, but the macro version takes in all parameters in the conveniently packed and documented [ConstConfig]
//! struct, instead of requiring several const generic parameters.
//!
//! IMPLEMENTATION NOTE: There would be a common trait (that is no longer present) binding [SocketClient] to [GenericSocketClient]
//!                      -- the reason being Rust 1.71 still not supporting zero-cost async in traits.
//!                      Other API designs had been attempted, but using async in traits (through the `async-trait` crate) and the
//!                      necessary GATs to provide zero-cost-abstracts, revealed a compiler bug making writing processors very difficult to compile:
//!                        - https://github.com/rust-lang/rust/issues/96865

/// Instantiates & allocates resources for a [GenericSocketClient], ready to be later started by [spawn_unresponsive_client_processor!()] or [spawn_responsive_client_processor!()].\
/// Params:
///   - `const_config`: [ConstConfig] -- the configurations for the server, enforcing const time optimizations;
///   - `interface_ip: IntoString` -- the interface to listen to incoming connections;
///   - `port: u16` -- the port to listen to incoming connections;
///   - `RemoteMessages`: [ReactiveMessagingDeserializer<>] -- the type of the messages produced by the clients;
///   - `LocalMessage`: [ReactiveMessagingSerializer<>] -- the type of the messages produced by this server -- should, additionally, implement the `Default` trait.


use std::fmt::Debug;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use futures::future::BoxFuture;
use futures::Stream;
use crate::socket_connection::{Peer, SocketConnectionHandler};
use crate::socket_client::common::upgrade_to_shutdown_and_connected_state_tracking;
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


/// Instantiates & allocates resources for a [GenericSocketClient], ready to be later started by [spawn_unresponsive_client_processor!()] or [spawn_responsive_client_processor!()].\
/// Params:
///   - `const_config`: [ConstConfig] -- the configurations for the client, enforcing const time optimizations;
///   - `ip: IntoString` -- the tcp ip to connect to;
///   - `port: u16` -- the tcp port to connect to;
///   - `RemoteMessages`: [ReactiveMessagingDeserializer<>] -- the type of the messages produced by the server;
///   - `LocalMessage`: [ReactiveMessagingSerializer<>] -- the type of the messages produced by this client -- should, additionally, implement the `Default` trait.
#[macro_export]
macro_rules! new_socket_client {
    ($const_config:    expr,
     $ip:              expr,
     $port:            expr,
     $remote_messages: ty,
     $local_messages:  ty) => {{
        const _CONFIG:                    u64   = $const_config.into();
        const _PROCESSOR_BUFFER:          usize = $const_config.receiver_buffer as usize;
        const _PROCESSOR_UNI_INSTRUMENTS: usize = $const_config.executor_instruments.into();
        const _SENDER_BUFFER:             usize = $const_config.sender_buffer   as usize;
        match $const_config.channel {
            Channels::Atomic => SocketClient::<_CONFIG,
                                               $remote_messages,
                                               $local_messages,
                                               _PROCESSOR_BUFFER,
                                               _PROCESSOR_UNI_INSTRUMENTS,
                                               _SENDER_BUFFER>
                                            ::Atomic(GenericSocketClient::<_CONFIG,
                                                                           $remote_messages,
                                                                           $local_messages,
                                                                           UniZeroCopyAtomic<$remote_messages, _PROCESSOR_BUFFER, 1, _PROCESSOR_UNI_INSTRUMENTS>,
                                                                           ChannelUniMoveAtomic<$local_messages, _SENDER_BUFFER, 1> >
                                                                        ::new($ip, $port) ),
            Channels::FullSync => SocketClient::<_CONFIG,
                                                 $remote_messages,
                                                 $local_messages,
                                                 _PROCESSOR_BUFFER,
                                                 _PROCESSOR_UNI_INSTRUMENTS,
                                                 _SENDER_BUFFER>
                                              ::FullSync(GenericSocketClient::<_CONFIG,
                                                                               $remote_messages,
                                                                               $local_messages,
                                                                               UniZeroCopyFullSync<$remote_messages, _PROCESSOR_BUFFER, 1, _PROCESSOR_UNI_INSTRUMENTS>,
                                                                               ChannelUniMoveFullSync<$local_messages, _SENDER_BUFFER, 1> >
                                                                            ::new($ip, $port) ),
            Channels::Crossbeam => SocketClient::<_CONFIG,
                                                  $remote_messages,
                                                  $local_messages,
                                                  _PROCESSOR_BUFFER,
                                                  _PROCESSOR_UNI_INSTRUMENTS,
                                                  _SENDER_BUFFER>
                                               ::Crossbeam(GenericSocketClient::<_CONFIG,
                                                                                 $remote_messages,
                                                                                 $local_messages,
                                                                                 UniMoveCrossbeam<$remote_messages, _PROCESSOR_BUFFER, 1, _PROCESSOR_UNI_INSTRUMENTS>,
                                                                                 ChannelUniMoveCrossbeam<$local_messages, _SENDER_BUFFER, 1> >
                                                                              ::new($ip, $port) ),
        }
    }}
}
pub use new_socket_client;


/// See [GenericSocketClient::spawn_unresponsive_processor()]
#[macro_export]
macro_rules! spawn_unresponsive_client_processor {
    ($socket_server:                expr,
     $connection_events_handler_fn: expr,
     $dialog_processor_builder_fn:  expr) => {{
        match &mut $socket_server {
            SocketClient::Atomic    (generic_socket_client)       => generic_socket_client.spawn_unresponsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await,
            SocketClient::FullSync  (generic_socket_client)       => generic_socket_client.spawn_unresponsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await,
            SocketClient::Crossbeam (generic_socket_client)       => generic_socket_client.spawn_unresponsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await,
        }
    }}
}
pub use spawn_unresponsive_client_processor;


/// See [GenericSocketClient::spawn_responsive_processor()]
#[macro_export]
macro_rules! spawn_responsive_client_processor {
    ($socket_client:                expr,
     $connection_events_handler_fn: expr,
     $dialog_processor_builder_fn:  expr) => {{
        match &mut $socket_client {
            SocketClient::Atomic    (generic_socket_client)       => generic_socket_client.spawn_responsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await,
            SocketClient::FullSync  (generic_socket_client)       => generic_socket_client.spawn_responsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await,
            SocketClient::Crossbeam (generic_socket_client)       => generic_socket_client.spawn_responsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await,
        }
    }}
}
pub use spawn_responsive_client_processor;


/// Represents a client built out of `CONFIG` (a `u64` version of [ConstConfig], from which the other const generic parameters derive).\
/// Don't instantiate this struct directly -- use [new_socket_client!()] instead.
pub enum SocketClient<const CONFIG:                    u64,
                      RemoteMessages:                  ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug           + 'static,
                      LocalMessages:                   ReactiveMessagingSerializer<LocalMessages>    + Send + Sync + PartialEq + Debug + Default + 'static,
                      const PROCESSOR_BUFFER:          usize,
                      const PROCESSOR_UNI_INSTRUMENTS: usize,
                      const SENDER_BUFFER:             usize> {

    Atomic(GenericSocketClient::<CONFIG,
                                 RemoteMessages,
                                 LocalMessages,
                                 UniZeroCopyAtomic<RemoteMessages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                 ChannelUniMoveAtomic<LocalMessages, SENDER_BUFFER, 1> >),

    FullSync(GenericSocketClient::<CONFIG,
                                   RemoteMessages,
                                   LocalMessages,
                                   UniZeroCopyFullSync<RemoteMessages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                   ChannelUniMoveFullSync<LocalMessages, SENDER_BUFFER, 1> >),

    Crossbeam(GenericSocketClient::<CONFIG,
                                    RemoteMessages,
                                    LocalMessages,
                                    UniMoveCrossbeam<RemoteMessages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                    ChannelUniMoveCrossbeam<LocalMessages, SENDER_BUFFER, 1> >),
}
 impl<const CONFIG:                    u64,
      RemoteMessages:                  ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug           + 'static,
      LocalMessages:                   ReactiveMessagingSerializer<LocalMessages>    + Send + Sync + PartialEq + Debug + Default + 'static,
      const PROCESSOR_BUFFER:          usize,
      const PROCESSOR_UNI_INSTRUMENTS: usize,
      const SENDER_BUFFER:             usize>
 SocketClient<CONFIG, RemoteMessages, LocalMessages, PROCESSOR_BUFFER, PROCESSOR_UNI_INSTRUMENTS, SENDER_BUFFER> {

     /// See [GenericSocketClient::shutdown_waiter()]
     pub fn shutdown_waiter(&mut self) -> Box<dyn FnOnce() -> BoxFuture<'static, Result<(), Box<dyn std::error::Error + Send + Sync>>> > {
         match self {
             SocketClient::Atomic    (generic_socket_client) => Box::new(generic_socket_client.shutdown_waiter()),
             SocketClient::FullSync  (generic_socket_client) => Box::new(generic_socket_client.shutdown_waiter()),
             SocketClient::Crossbeam (generic_socket_client) => Box::new(generic_socket_client.shutdown_waiter()),
         }
     }

     /// See [GenericSocketClient::shutdown()]
     pub fn shutdown(mut self, timeout_ms: u32) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
         match self {
             SocketClient::Atomic    (generic_socket_client) => generic_socket_client.shutdown(timeout_ms),
             SocketClient::FullSync  (generic_socket_client) => generic_socket_client.shutdown(timeout_ms),
             SocketClient::Crossbeam (generic_socket_client) => generic_socket_client.shutdown(timeout_ms),
         }
     }
}


/// Real definition & implementation for our Socket Client, full of generic parameters.\
/// Probably you want to instantiate this structure through the sugared macro [new_socket_client!()] instead.
/// Generic Parameters:
///   - `CONFIG`:           the `u64` version of the [ConstConfig] instance used to build this struct -- from which `ProcessorUniType` and `SenderChannel` derive;
///   - `RemoteMessages`:   the messages that are generated by the server (usually an `enum`);
///   - `LocalMessages`:    the messages that are generated by this client (usually an `enum`);
///   - `ProcessorUniType`: an instance of a `reactive-mutiny`'s [Uni] type (using one of the zero-copy channels) --
///                         This [Uni] will execute the given client reactive logic for each incoming message (see how it is used in [new_socket_client!()]);
///   - `SenderChannel`:    an instance of a `reactive-mutiny`'s Uni movable `Channel`, which will provide a `Stream` of messages to be sent to the server.
pub struct GenericSocketClient<const CONFIG:        u64,
                               RemoteMessages:      ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
                               LocalMessages:       ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
                               ProcessorUniType:    GenericUni<ItemType=RemoteMessages>                                         + Send + Sync                     + 'static,
                               SenderChannel:       FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync> {

    /// false if a disconnection happened, as tracked by the socket logic
    connected: Arc<AtomicBool>,
    /// The interface to listen to incoming connections
    ip: String,
    /// The port to listen to incoming connections
    port: u16,
    /// Signaler to stop this client
    client_shutdown_signaler: Option<tokio::sync::oneshot::Sender<u32>>,
    /// Signaler to cause [wait_for_shutdown()] to return
    local_shutdown_receiver: Option<tokio::sync::oneshot::Receiver<()>>,
    _phantom: PhantomData<(RemoteMessages,LocalMessages,ProcessorUniType,SenderChannel)>
}
impl<const CONFIG:        u64,
     RemoteMessages:      ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:       ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:    GenericUni<ItemType=RemoteMessages>                                         + Send + Sync                     + 'static,
     SenderChannel:       FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync>
GenericSocketClient<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannel> {

    /// Instantiates a client to connect to a TCP/IP Server:
    ///   `ip`:                   the server IP to connect to
    ///   `port`:                 the server port to connect to
    pub fn new<IntoString: Into<String>>
              (ip:   IntoString,
               port: u16)
              -> Self {
        Self {
            connected: Arc::new(AtomicBool::new(false)),
            ip: ip.into(),
            port,
            client_shutdown_signaler: None,
            local_shutdown_receiver:  None,
            _phantom:                 PhantomData,
        }
    }

    /// Spawns a task to start the local client, returning immediately.\
    /// The given `dialog_processor_builder_fn` will be called when the connection is established and should return a `Stream`
    /// that will produce non-futures & non-fallible items that **won't be sent to the server**:
    ///   - `connection_events_callback`: -- a generic function (or closure) to handle connected, disconnected and shutdown events. Sign it as:
    ///     ```nocompile
    ///     async fn connection_events_handler<const CONFIG:  u64,
    ///                                        LocalMessages: ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
    ///                                        SenderChannel: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync>
    ///                                       (_event: ConnectionEvent<CONFIG, LocalMessages, SenderChannel>) {...}
    ///     ```
    ///   - `dialog_processor_builder_fn` -- the generic function (or closure) that receives the `Stream` of server messages and returns another `Stream`, which won't
    ///                                      be sent out to the server -- called once for each connection. Sign it as:
    ///     ```nocompile
    ///     fn unresponsive_processor<const CONFIG:   u64,
    ///                               LocalMessages:  ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
    ///                               SenderChannel:  FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync,
    ///                               StreamItemType: Deref<Target=[your type for messages produced by the SERVER]>>
    ///                              (server_addr:            String,
    ///                               connected_port:         u16,
    ///                               peer:                   Arc<Peer<CONFIG, LocalMessages, SenderChannel>>,
    ///                               server_messages_stream: impl Stream<Item=StreamItemType>)
    ///                              -> impl Stream<Item=()> {...}
    ///     ```
    /// -- if you want the processor to produce answer messages of type `LocalMessages` to be sent to the server, see [Self::spawn_responsive_processor()]:
    #[inline(always)]
    pub async fn spawn_unresponsive_processor<OutputStreamItemsType:                                                                                                                                                                                                                                   Send + Sync + Debug       + 'static,
                                              ServerStreamType:               Stream<Item=OutputStreamItemsType>                                                                                                                                                                                     + Send                      + 'static,
                                              ConnectionEventsCallbackFuture: Future<Output=()>                                                                                                                                                                                                      + Send                      + 'static,
                                              ConnectionEventsCallback:       Fn(/*event: */ConnectionEvent<CONFIG, LocalMessages, SenderChannel>)                                                                                                                 -> ConnectionEventsCallbackFuture + Send + Sync               + 'static,
                                              ProcessorBuilderFn:             Fn(/*server_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<CONFIG, LocalMessages, SenderChannel>>, /*server_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> ServerStreamType               + Send + Sync               + 'static>

                                             (&mut self,
                                              connection_events_callback:  ConnectionEventsCallback,
                                              dialog_processor_builder_fn: ProcessorBuilderFn)

                                             -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

        let (client_shutdown_sender, client_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (local_shutdown_sender, local_shutdown_receiver) = tokio::sync::oneshot::channel::<()>();
        self.client_shutdown_signaler = Some(client_shutdown_sender);
        self.local_shutdown_receiver = Some(local_shutdown_receiver);
        let ip = self.ip.clone();
        let port = self.port;

        let connection_events_callback = upgrade_to_shutdown_and_connected_state_tracking(&self.connected, local_shutdown_sender, connection_events_callback);

        let socket_connection_handler = SocketConnectionHandler::<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannel>::new();
        socket_connection_handler.client_for_unresponsive_text_protocol(ip.clone(),
                                                                        port,
                                                                        client_shutdown_receiver,
                                                                        connection_events_callback,
                                                                        dialog_processor_builder_fn).await
            .map_err(|err| format!("Error starting unresponsive GenericSocketClient @ {ip}:{port}: {:?}", err))?;
        Ok(())
    }

    /// Spawns a task to start the local client, returning immediately,
    /// The given `dialog_processor_builder_fn` will be called when the connection is established and should return a `Stream`
    /// that will produce non-futures & non-fallible items that *will be sent to the server*:
    ///   - `connection_events_callback`: -- a generic function (or closure) to handle connected, disconnected and shutdown events. Sign it as:
    ///     ```nocompile
    ///     async fn connection_events_handler<const CONFIG:  u64,
    ///                                        LocalMessages: ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
    ///                                        SenderChannel: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync>
    ///                                       (_event: ConnectionEvent<CONFIG, LocalMessages, SenderChannel>) {...}
    ///     ```
    ///   - `dialog_processor_builder_fn` -- the generic function (or closure) that receives the `Stream` of server messages and returns the `Stream` of client messages to
    ///                                      be sent to the server -- called once for each connection. Sign it as:
    ///     ```nocompile
    ///     fn responsive_processor<const CONFIG:   u64,
    ///                             LocalMessages:  ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
    ///                             SenderChannel:  FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync,
    ///                             StreamItemType: Deref<Target=[your type for messages produced by the SERVER]>>
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
        self.client_shutdown_signaler = Some(server_shutdown_sender);
        self.local_shutdown_receiver = Some(local_shutdown_receiver);
        let ip = self.ip.clone();
        let port = self.port;

        let connection_events_callback = upgrade_to_shutdown_and_connected_state_tracking(&self.connected, local_shutdown_sender, connection_events_callback);

        let socket_connection_handler = SocketConnectionHandler::<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannel>::new();
        socket_connection_handler.client_for_responsive_text_protocol
            (ip.clone(),
             port,
             server_shutdown_receiver,
             connection_events_callback,
             dialog_processor_builder_fn).await
            .map_err(|err| format!("Error starting responsive GenericSocketClient @ {ip}:{port}: {:?}", err))?;
        Ok(())
    }

    /// Returns an async closure that blocks until [Self::shutdown()] is called.
    /// Example:
    /// ```no_compile
    ///     self.spawn_the_client(logic...);
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
                        Err(err) => Err(Box::from(format!("GenericSocketClient::wait_for_shutdown(): It is no longer possible to tell when the client will be shutdown: `one_shot` signal error: {err}")))
                    }
                } else {
                    Err(Box::from("GenericSocketClient: \"wait for shutdown\" requested, but the client was not started (or a previous shutdown was commanded) at the moment `shutdown_waiter()` was called"))
                }
            }
        })
    }

    /// Notifies the client it is time to shutdown.\
    /// A shutdown is considered graceful if it could be accomplished in less than `timeout_ms` milliseconds.\
    /// It is a good practice that the `connection_events_handler()` you provided when starting the client
    /// uses this time to inform the server that a client-initiated disconnection (due to a local shutdown) is happening.
    pub fn shutdown(mut self, timeout_ms: u32) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match self.client_shutdown_signaler.take() {
            Some(server_sender) => {
                warn!("GenericSocketClient: Shutdown asked & initiated for client connected @ {}:{} -- timeout: {timeout_ms}ms", self.ip, self.port);
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
    use crate::{ron_deserializer, ron_serializer};
    use std::{future, ops::Deref};
    use futures::StreamExt;
    use serde::{Deserialize, Serialize};


    const REMOTE_SERVER: &str = "66.45.249.218";


    /// Test that our instantiation macro is able to produce servers backed by all possible channel types
    #[cfg_attr(not(doc), test)]
    fn instantiation() {
        let atomic_client = new_socket_client!(
            ConstConfig {
                channel: Channels::Atomic,
                ..ConstConfig::default()
            },
            REMOTE_SERVER, 443, String, String);
        assert!(matches!(atomic_client, SocketClient::Atomic(_)), "an Atomic Client couldn't be instantiated");

        let fullsync_client = new_socket_client!(
            ConstConfig {
                channel: Channels::FullSync,
                ..ConstConfig::default()
            },
            REMOTE_SERVER, 443, String, String);
        assert!(matches!(fullsync_client, SocketClient::FullSync(_)), "a FullSync Client couldn't be instantiated");

        let crossbeam_client = new_socket_client!(
            ConstConfig {
                channel: Channels::Crossbeam,
                ..ConstConfig::default()
            },
            REMOTE_SERVER, 443, String, String);
        assert!(matches!(crossbeam_client, SocketClient::Crossbeam(_)), "a Crossbeam Client couldn't be instantiated");
    }

    /// Test that our client types are ready for usage
    #[cfg_attr(not(doc),tokio::test)]
    async fn doc_usage() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

        // demonstrates how to build an unresponsive client
        ///////////////////////////////////////////////////
        // using fully typed generic functions that will work with all possible configs
        let mut client = new_socket_client!(
            ConstConfig::default(),
            REMOTE_SERVER,
            443,
            DummyClientAndServerMessages,
            DummyClientAndServerMessages);
        spawn_unresponsive_client_processor!(client,
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
                                 (_client_addr:           String,
                                  _connected_port:        u16,
                                  _peer:                  Arc<Peer<CONFIG, LocalMessages, SenderChannel>>,
                                  client_messages_stream: impl Stream<Item=StreamItemType>)
                                 -> impl Stream<Item=()> {
            client_messages_stream.map(|payload| ())
        }
        let shutdown_waiter = client.shutdown_waiter();
        client.shutdown(200)?;
        shutdown_waiter().await?;

        // demonstrates how to build a responsive server
        ////////////////////////////////////////////////
        // using fully typed generic functions that will work with all possible configs
        let mut client = new_socket_client!(
            ConstConfig::default(),
            REMOTE_SERVER,
            443,
            DummyClientAndServerMessages,
            DummyClientAndServerMessages);
        spawn_responsive_client_processor!(client,
            connection_events_handler,
            responsive_processor
        )?;
        fn responsive_processor<const CONFIG:   u64,
                                SenderChannel:  FullDuplexUniChannel<ItemType=DummyClientAndServerMessages, DerivedItemType=DummyClientAndServerMessages> + Send + Sync,
                                StreamItemType: Deref<Target=DummyClientAndServerMessages>>
                               (_client_addr:           String,
                                _connected_port:        u16,
                                _peer:                  Arc<Peer<CONFIG, DummyClientAndServerMessages, SenderChannel>>,
                                client_messages_stream: impl Stream<Item=StreamItemType>)
                               -> impl Stream<Item=DummyClientAndServerMessages> {
            client_messages_stream.map(|_payload| DummyClientAndServerMessages::FloodPing)
        }
        let shutdown_waiter = client.shutdown_waiter();
        client.shutdown(200)?;
        shutdown_waiter().await?;

        // demonstrates how to use it with closures -- also allowing for any channel in the configs
        ///////////////////////////////////////////////////////////////////////////////////////////
        let mut client = new_socket_client!(
            ConstConfig::default(),
            REMOTE_SERVER,
            443,
            DummyClientAndServerMessages,
            DummyClientAndServerMessages);
        spawn_unresponsive_client_processor!(client,
            |_| future::ready(()),
            |_, _, _, client_messages_stream| client_messages_stream.map(|_payload| DummyClientAndServerMessages::FloodPing)
        )?;
        let shutdown_waiter = client.shutdown_waiter();
        client.shutdown(200)?;
        shutdown_waiter().await?;

        // demonstrates how to use the concrete type
        ////////////////////////////////////////////
        // notice there may be a discrepancy in the `ConstConfig` you provide and the actual concrete types
        // you also provide for `UniProcessor` and `SenderChannel` -- therefore, this usage is not recommended
        // (but it is here anyway since it may bring, theoretically, a infinitesimal performance benefit)
        const CONFIG: ConstConfig = ConstConfig {
            receiver_buffer:      2048,
            sender_buffer:        1024,
            channel:              Channels::FullSync,
            executor_instruments: reactive_mutiny::prelude::Instruments::LogsWithExpensiveMetrics,
            ..ConstConfig::default()
        };
        type ProcessorUniType = UniZeroCopyFullSync<DummyClientAndServerMessages, {CONFIG.receiver_buffer as usize}, 1, {CONFIG.executor_instruments.into()}>;
        type SenderChannelType = ChannelUniMoveFullSync<DummyClientAndServerMessages, {CONFIG.sender_buffer as usize}, 1>;
        let mut client = GenericSocketClient :: <{CONFIG.into()},
                                                                     DummyClientAndServerMessages,
                                                                     DummyClientAndServerMessages,
                                                                     ProcessorUniType,
                                                                     SenderChannelType>
                                                                 :: new("66.45.249.218",443);
        client.spawn_unresponsive_processor(
            |_| future::ready(()),
            |_, _, _, client_messages_stream| client_messages_stream.map(|_payload| DummyClientAndServerMessages::FloodPing)
        ).await?;
        let shutdown_waiter = client.shutdown_waiter();
        client.shutdown(200)?;
        shutdown_waiter().await?;

        Ok(())
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
                .expect("unresponsive_socket_client.rs unit tests: No errors should have happened here!")
        }
        #[inline(always)]
        fn processor_error_message(err: String) -> DummyClientAndServerMessages {
            panic!("unresponsive_socket_client.rs unit tests: protocol error when none should have happened: {err}");
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