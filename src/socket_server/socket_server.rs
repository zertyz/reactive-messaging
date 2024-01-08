//! Provides the [new_socket_server!()] & [new_composite_socket_server!()] macros for instantiating servers, which may then be started by
//! [start_unresponsive_server_processor!()] & [start_responsive_server_processor!()] -- with each taking different variants of the reactive processor logic --
//! or have the composite processors spawned by [spawn_unresponsive_composite_server_processor!()] & [spawn_responsive_composite_server_processor!()].
//!
//! Both reactive processing logic variants will take in a `Stream` as parameter and should return another `Stream` as output:
//!   * Responsive: the reactive processor's output `Stream` yields items that are messages to be sent back to each client;
//!   * Unresponsive the reactive processor's output `Stream` won't be sent to the clients, allowing the `Stream`s to return items from any type.
//!
//! The Unresponsive processors still allow you to send messages to the client and should be preferred (as far as performance is concerned)
//! for protocols where the count of messages IN don't nearly map 1/1 with the count of messages OUT.
//!
//! For every server variant, the `Stream` items may be a combination of fallible/non-fallible (using `Result<>` or not) & future/non-future.
//!
//! Instead of using the mentioned macros, you might want take a look at [CompositeSocketServer] to access the inner implementation directly
//! -- both ways have the same flexibility, but the macro version takes in all parameters in the conveniently packed and documented [ConstConfig]
//! struct, instead of requiring several const generic parameters.


use crate::{
    socket_server::common::upgrade_to_termination_tracking,
    types::{
        ConnectionEvent,
        MessagingMutinyStream,
        ResponsiveMessages,
    },
    socket_connection::{
        peer::Peer,
        socket_connection_handler::SocketConnectionHandler,
        connection_provider::{ServerConnectionHandler,ConnectionChannel},
    },
    ReactiveMessagingDeserializer,
    ReactiveMessagingSerializer,
    config::ConstConfig,
};
use std::{
    fmt::Debug,
    future::Future,
    marker::PhantomData,
    sync::Arc,
};
use reactive_mutiny::prelude::advanced::{
    ChannelUniMoveFullSync,
    UniZeroCopyFullSync,
    FullDuplexUniChannel,
    GenericUni,
};
use std::net::SocketAddr;
use futures::{future::BoxFuture, Stream};
use tokio::{
    io::AsyncWriteExt,
    net::TcpStream,
};
use log::{error, trace, warn};


/// Instantiates & allocates resources for a stateless [CompositeSocketServer] (suitable for single protocol communications),
/// ready to be later started by [start_unresponsive_server_processor!()] or [start_responsive_server_processor!()]
/// -- using the default "Atomic" channels (see [new_fullsync_server!()] & [new_crossbeam_server!()] for alternatives).\
/// Params:
///   - `const_config`: [ConstConfig] -- the configurations for the server, enforcing const/compile time optimizations;
///   - `interface_ip: IntoString` -- the interface to listen to incoming connections;
///   - `port: u16` -- the port to listen to incoming connections;
///   - `remote_messages`: [ReactiveMessagingDeserializer<>] -- the type of the messages produced by the clients;
///   - `local_messages`: [ReactiveMessagingSerializer<>] -- the type of the messages produced by this server -- should, additionally, implement the `Default` trait.\
/// See [new_composite_socket_server!()] if you want to use the "Composite Protocol Stacking" pattern.
#[macro_export]
macro_rules! new_socket_server {
    ($const_config:    expr,
     $interface_ip:    expr,
     $port:            expr,
     $remote_messages: ty,
     $local_messages:  ty) => {
        crate::new_atomic_server!($const_config, $interface_ip, $port, $remote_messages, $local_messages)
    }
}
pub use new_socket_server;


/// Instantiates & allocates resources for a stateful [CompositeSocketServer] (suitable for the "Composite Protocol Stacking" pattern),
/// ready to have processors added by [spawn_unresponsive_server_processor!()] or [spawn_responsive_server_processor!()]
/// and to be later started by [CompositeSocketServer::start_with_routing_closure]
/// -- using the default "Atomic" channels (see [new_composite_fullsync_server!()] & [new_composite_crossbeam_server!()] for alternatives).\
/// Params:
///   - `const_config`: [ConstConfig] -- the configurations for the server, enforcing const/compile time optimizations;
///   - `interface_ip: IntoString` -- the interface to listen to incoming connections;
///   - `port: u16` -- the port to listen to incoming connections;
///   - `remote_messages`: [ReactiveMessagingDeserializer<>] -- the type of the messages produced by the clients;
///   - `local_messages`: [ReactiveMessagingSerializer<>] -- the type of the messages produced by this server -- should, additionally, implement the `Default` trait.
///   - `state_type: Default` -- The state type used by the "connection routing closure" (to be provided) to promote the "Composite Protocol Stacking" pattern
/// See [new_socket_server!()] if you want to use the "Composite Protocol Stacking" pattern.
#[macro_export]
macro_rules! new_composite_socket_server {
    ($const_config:    expr,
     $interface_ip:    expr,
     $port:            expr,
     $remote_messages: ty,
     $local_messages:  ty,
     $state_type:      ty) => {
        crate::new_composite_atomic_server!($const_config, $interface_ip, $port, $remote_messages, $local_messages, $state_type)
    }
}
pub use new_composite_socket_server;


/// For docs, see [new_socket_server!()]
#[macro_export]
macro_rules! new_atomic_server {
    ($const_config:    expr,
     $interface_ip:    expr,
     $port:            expr,
     $remote_messages: ty,
     $local_messages:  ty) => {
        crate::new_composite_atomic_server!($const_config, $interface_ip, $port, $remote_messages, $local_messages, ())
    }
}
pub use new_atomic_server;


/// For docs, see [new_socket_server!()]
#[macro_export]
macro_rules! new_fullsync_server {
    ($const_config:    expr,
     $interface_ip:    expr,
     $port:            expr,
     $remote_messages: ty,
     $local_messages:  ty) => {
        crate::new_composite_fullsync_server!($const_config, $interface_ip, $port, $remote_messages, $local_messages, ())
    }
}
pub use new_fullsync_server;


/// For docs, see [new_socket_server!()]
#[macro_export]
macro_rules! new_crossbeam_server {
    ($const_config:    expr,
     $interface_ip:    expr,
     $port:            expr,
     $remote_messages: ty,
     $local_messages:  ty) => {
        crate::new_composite_crossbeam_server!($const_config, $interface_ip, $port, $remote_messages, $local_messages, ())
    }
}
pub use new_crossbeam_server;


/// For docs, see [new_composite_socket_server!()]
#[macro_export]
macro_rules! new_composite_atomic_server {
    ($const_config:    expr,
     $interface_ip:    expr,
     $port:            expr,
     $remote_messages: ty,
     $local_messages:  ty,
     $state_type:      ty) => {{
        const CONST_CONFIG:               ConstConfig = $const_config;
        const _CONFIG:                    u64         = CONST_CONFIG.into();
        const _PROCESSOR_BUFFER:          usize       = CONST_CONFIG.receiver_buffer as usize;
        const _PROCESSOR_UNI_INSTRUMENTS: usize       = CONST_CONFIG.executor_instruments.into();
        const _SENDER_BUFFER:             usize       = CONST_CONFIG.sender_buffer   as usize;

        CompositeSocketServer::<_CONFIG,
                                $remote_messages,
                                $local_messages,
                                UniZeroCopyAtomic<$remote_messages, _PROCESSOR_BUFFER, 1, _PROCESSOR_UNI_INSTRUMENTS>,
                                ChannelUniMoveAtomic<$local_messages, _SENDER_BUFFER, 1>,
                                $state_type >
                             ::new($interface_ip, $port)
    }}
}
pub use new_composite_atomic_server;


/// For docs, see [new_composite_socket_server!()]
#[macro_export]
macro_rules! new_composite_fullsync_server {
    ($const_config:    expr,
     $interface_ip:    expr,
     $port:            expr,
     $remote_messages: ty,
     $local_messages:  ty,
     $state_type:      ty) => {{
        const CONST_CONFIG:               ConstConfig = $const_config;
        const _CONFIG:                    u64         = CONST_CONFIG.into();
        const _PROCESSOR_BUFFER:          usize       = CONST_CONFIG.receiver_buffer as usize;
        const _PROCESSOR_UNI_INSTRUMENTS: usize       = CONST_CONFIG.executor_instruments.into();
        const _SENDER_BUFFER:             usize       = CONST_CONFIG.sender_buffer   as usize;

        CompositeSocketServer::<_CONFIG,
                                $remote_messages,
                                $local_messages,
                                UniZeroCopyFullSync<$remote_messages, _PROCESSOR_BUFFER, 1, _PROCESSOR_UNI_INSTRUMENTS>,
                                ChannelUniMoveFullSync<$local_messages, _SENDER_BUFFER, 1>,
                                $state_type >
                             ::new($interface_ip, $port)
    }}
}
pub use new_composite_fullsync_server;


/// For docs, see [new_composite_socket_server!()]
#[macro_export]
macro_rules! new_composite_crossbeam_server {
    ($const_config:    expr,
     $interface_ip:    expr,
     $port:            expr,
     $remote_messages: ty,
     $local_messages:  ty,
     $state_type:      ty) => {{
        const CONST_CONFIG:               ConstConfig = $const_config;
        const _CONFIG:                    u64         = CONST_CONFIG.into();
        const _PROCESSOR_BUFFER:          usize       = CONST_CONFIG.receiver_buffer as usize;
        const _PROCESSOR_UNI_INSTRUMENTS: usize       = CONST_CONFIG.executor_instruments.into();
        const _SENDER_BUFFER:             usize       = CONST_CONFIG.sender_buffer   as usize;

        CompositeSocketServer::<_CONFIG,
                                $remote_messages,
                                $local_messages,
                                UniMoveCrossbeam<$remote_messages, _PROCESSOR_BUFFER, 1, _PROCESSOR_UNI_INSTRUMENTS>,
                                ChannelUniMoveCrossbeam<$local_messages, _SENDER_BUFFER, 1>,
                                $state_type >
                             ::new($interface_ip, $port)
    }}
}
pub use new_composite_crossbeam_server;


/// Starts a server (previously instantiated by [new_socket_server!()]) that will communicate with clients using a single protocol -- as defined by the given
/// `dialog_processor_builder_fn`, a builder of "unresponsive" `Stream`s as specified in [CompositeSocketServer::spawn_unresponsive_processor()].\
/// If you want to follow the "Composite Protocol Stacking" pattern, see the [spawn_unresponsive_composite_server_processor!()] macro instead.
#[macro_export]
macro_rules! start_unresponsive_server_processor {
    ($socket_server:                expr,
     $connection_events_handler_fn: expr,
     $dialog_processor_builder_fn:  expr) => {{
        match $socket_server.spawn_unresponsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await {
            Ok(connection_channel) => $socket_server.start_with_single_protocol(connection_channel).await,
            Err(err) => Err(err),
        }
    }}
}
pub use start_unresponsive_server_processor;


/// Starts a server (previously instantiated by [new_socket_server!()]) that will communicate with clients using a single protocol -- as defined by the given
/// `dialog_processor_builder_fn`, a builder of "responsive" `Stream`s as specified in [CompositeSocketServer::spawn_responsive_processor()].\
/// If you want to follow the "Composite Protocol Stacking" pattern, see the [spawn_responsive_composite_server_processor!()] macro instead.
#[macro_export]
macro_rules! start_responsive_server_processor {
    ($socket_server:                expr,
     $connection_events_handler_fn: expr,
     $dialog_processor_builder_fn:  expr) => {{
        match $socket_server.spawn_responsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await {
            Ok(connection_channel) => $socket_server.start_with_single_protocol(connection_channel).await,
            Err(err) => Err(err),
        }
    }}
}
pub use start_responsive_server_processor;
use crate::types::MessagingService;


/// Real definition & implementation for our Socket Server, full of generic parameters.\
/// Probably you want to instantiate this structure through the sugared macros [new_socket_server!()] or [new_composite_socket_server!()] instead.
/// Generic Parameters:
///   - `CONFIG`:           the `u64` version of the [ConstConfig] instance used to build this struct -- from which `ProcessorUniType` and `SenderChannel` derive;
///   - `RemoteMessages`:   the messages that are generated by the clients (usually an `enum`);
///   - `LocalMessages`:    the messages that are generated by the server (usually an `enum`);
///   - `ProcessorUniType`: an instance of a `reactive-mutiny`'s [Uni] type (using one of the zero-copy channels) --
///                         This [Uni] will execute the given server reactive logic for each incoming message (see how it is used in [new_socket_server!()] or [new_composite_socket_server!()]);
///   - `SenderChannel`:    an instance of a `reactive-mutiny`'s Uni movable `Channel`, which will provide a `Stream` of messages to be sent to the client;
///   - `StateType`:        The state type used by the "connection routing closure" (to be provided), enabling the "Composite Protocol Stacking" pattern.
pub struct CompositeSocketServer<const CONFIG:        u64,
                               RemoteMessages:      ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
                               LocalMessages:       ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
                               ProcessorUniType:    GenericUni<ItemType=RemoteMessages>                                         + Send + Sync                     + 'static,
                               SenderChannel:       FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync,
                               StateType:                                                                                         Send + Sync + Default           + 'static> {

    /// The interface to listen to incoming connections
    interface_ip: String,
    /// The port to listen to incoming connections
    port: u16,
    /// The abstraction containing the network loop that accepts connections for us + facilities to start processing already
    /// opened connections (enabling the "Composite Protocol Stacking" design pattern)
    connection_provider: Option<ServerConnectionHandler>,
    /// Signalers to cause [Self::termination_waiter()]'s closure to return (once they all dispatch their signals)
    processor_termination_complete_receivers:  Option<Vec<tokio::sync::oneshot::Receiver<()>>>,
    /// Connections returned by processors after they are done with them -- these connections
    /// may be routed to another processor if the "Composite Protocol Stacking" pattern is in play.
    returned_connections_source: Option<tokio::sync::mpsc::Receiver<(TcpStream, Option<StateType>)>>,
    returned_connections_sink: tokio::sync::mpsc::Sender<(TcpStream, Option<StateType>)>,
    _phantom: PhantomData<(RemoteMessages,LocalMessages,ProcessorUniType,SenderChannel)>
}
impl<const CONFIG:        u64,
     RemoteMessages:      ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:       ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:    GenericUni<ItemType=RemoteMessages>                                         + Send + Sync                     + 'static,
     SenderChannel:       FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync                     + 'static,
     StateType:                                                                                         Send + Sync + Default           + 'static>
CompositeSocketServer<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannel, StateType> {

    /// Creates a new server instance listening on TCP/IP:
    ///   `interface_ip`:         the interface's IP to listen to -- 0.0.0.0 will cause listening to all network interfaces
    ///   `port`:                 what port to listen to
    pub fn new<IntoString: Into<String>>
              (interface_ip: IntoString,
               port:         u16)
              -> Self {
        let (returned_connections_sink, returned_connections_source) = tokio::sync::mpsc::channel::<(TcpStream, Option<StateType>)>(2);
        Self {
            interface_ip: interface_ip.into(),
            port,
            connection_provider: None,
            processor_termination_complete_receivers: Some(vec![]),
            returned_connections_source: Some(returned_connections_source),
            returned_connections_sink,
            _phantom: PhantomData,
        }
    }
}

impl<const CONFIG:        u64,
     RemoteMessages:      ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:       ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:    GenericUni<ItemType=RemoteMessages>                                         + Send + Sync                     + 'static,
     SenderChannel:       FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync                     + 'static,
     StateType:                                                                                         Send + Sync + Default           + 'static>
MessagingService<CONFIG> for CompositeSocketServer<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannel, StateType> {
    type RemoteMessages = RemoteMessages;
    type LocalMessages = LocalMessages;
    type ProcessorUniType = ProcessorUniType;
    type SenderChannel = SenderChannel;
    type StateType = StateType;

    async fn spawn_unresponsive_processor<OutputStreamItemsType:                                                                                                                                                                                                                                                Send + Sync + Debug       + 'static,
                                          ServerStreamType:               Stream<Item=OutputStreamItemsType>                                                                                                                                                                                                  + Send                      + 'static,
                                          ConnectionEventsCallbackFuture: Future<Output=()>                                                                                                                                                                                                                   + Send                      + 'static,
                                          ConnectionEventsCallback:       Fn(/*event: */ConnectionEvent<CONFIG, LocalMessages, SenderChannel, StateType>)                                                                                                                 -> ConnectionEventsCallbackFuture + Send + Sync               + 'static,
                                          ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<CONFIG, LocalMessages, SenderChannel, StateType>>, /*client_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> ServerStreamType               + Send + Sync               + 'static>

                                         (&mut self,
                                          connection_events_callback:  ConnectionEventsCallback,
                                          dialog_processor_builder_fn: ProcessorBuilderFn)

                                         -> Result<ConnectionChannel, Box<dyn std::error::Error + Sync + Send>> {

        // configure this processor's "termination is complete" signaler
        let (local_termination_sender, local_termination_receiver) = tokio::sync::oneshot::channel::<()>();
        self.processor_termination_complete_receivers.as_mut().expect("BUG!").push(local_termination_receiver);
        let connection_events_callback = upgrade_to_termination_tracking(local_termination_sender, connection_events_callback);

        // the source of connections for this processor to start working on
        let mut connection_provider = ConnectionChannel::new();
        let new_connections_source = connection_provider.receiver()
            .ok_or_else(|| format!("couldn't move the Connection Receiver out of the Connection Provider"))?;

        // start the server
        let socket_communications_handler = SocketConnectionHandler::<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannel, StateType>::new();
        socket_communications_handler.server_loop_for_unresponsive_text_protocol(&self.interface_ip,
                                                                                 self.port,
                                                                                 new_connections_source,
                                                                                 self.returned_connections_sink.clone(),
                                                                                 connection_events_callback,
                                                                                 dialog_processor_builder_fn).await
            .map_err(|err| format!("Error starting an unresponsive GenericCompositeSocketServer @ {}:{}: {:?}", self.interface_ip, self.port, err))?;
        Ok(connection_provider)
    }

    async fn spawn_responsive_processor<ServerStreamType:                Stream<Item=LocalMessages>                                                                                                                                                                                                          + Send        + 'static,
                                        ConnectionEventsCallbackFuture:  Future<Output=()>                                                                                                                                                                                                                   + Send        + 'static,
                                        ConnectionEventsCallback:        Fn(/*event: */ConnectionEvent<CONFIG, LocalMessages, SenderChannel, StateType>)                                                                                                                 -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                        ProcessorBuilderFn:              Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<CONFIG, LocalMessages, SenderChannel, StateType>>, /*client_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> ServerStreamType               + Send + Sync + 'static>

                                       (&mut self,
                                        connection_events_callback:  ConnectionEventsCallback,
                                        dialog_processor_builder_fn: ProcessorBuilderFn)

                                       -> Result<ConnectionChannel, Box<dyn std::error::Error + Sync + Send>>

                                       where LocalMessages: ResponsiveMessages<LocalMessages> {

        // configure this processor's "termination is complete" signaler
        let (local_termination_sender, local_termination_receiver) = tokio::sync::oneshot::channel::<()>();
        self.processor_termination_complete_receivers.as_mut().expect("BUG!").push(local_termination_receiver);
        let connection_events_callback = upgrade_to_termination_tracking(local_termination_sender, connection_events_callback);

        // the source of connections for this processor to start working on
        let mut connection_provider = ConnectionChannel::new();
        let new_connections_source = connection_provider.receiver()
            .ok_or_else(|| format!("couldn't move the Connection Receiver out of the Connection Provider"))?;

        // start the server
        let socket_communications_handler = SocketConnectionHandler::<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannel, StateType>::new();
        socket_communications_handler.server_loop_for_responsive_text_protocol(&self.interface_ip,
                                                                               self.port,
                                                                               new_connections_source,
                                                                               self.returned_connections_sink.clone(),
                                                                               connection_events_callback,
                                                                               dialog_processor_builder_fn).await
            .map_err(|err| format!("Error starting a responsive GenericCompositeSocketServer @ {}:{}: {:?}", self.interface_ip, self.port, err))?;
        Ok(connection_provider)
    }

    async fn start_with_routing_closure(&mut self,
                                        mut connection_routing_closure: impl FnMut(/*connection: */& tokio::net::TcpStream, /*last_state: */Option<StateType>) -> Option<tokio::sync::mpsc::Sender<TcpStream>> + Send + 'static)
                                       -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
        let mut connection_provider = ServerConnectionHandler::new(&self.interface_ip, self.port).await
            .map_err(|err| format!("couldn't start the Connection Provider server event loop: {err}"))?;
        let mut new_connections_source = connection_provider.connection_receiver()
            .ok_or_else(|| format!("couldn't move the Connection Receiver out of the Connection Provider"))?;
        _ = self.connection_provider.insert(connection_provider);

        let mut returned_connections_source = self.returned_connections_source.take()
            .ok_or_else(|| format!("couldn't `take()` from the `returned_connections_source`. Has the server been `.start()`ed more than once?"))?;

        let interface_ip = self.interface_ip.clone();
        let port = self.port;

        // Spawns the "connection routing task" to:
        //   - Listen to newly incoming connections as well as upgraded/downgraded ones shared between processors
        //   - Gives them to the `protocol_stacking_closure`
        //   - Routes them to the right processor or close the connection
        tokio::spawn(async move {

            loop {
                let (mut connection, sender) = tokio::select! {

                    // process newly incoming connections
                    new_connection = new_connections_source.recv() => {
                        let Some(new_connection) = new_connection else { break };
                        let sender = connection_routing_closure(&new_connection, None);
                        (new_connection, sender)
                    },

                    // process connections returned by the processors (after they ended processing them)
                    returned_connection_and_state = returned_connections_source.recv() => {
                        let Some((returned_connection, last_state)) = returned_connection_and_state else { break };
                        let sender = connection_routing_closure(&returned_connection, last_state);
                        (returned_connection, sender)
                    },
                };

                // route the connection to another processor or drop it
                match sender {
                    Some(sender) => {
                        let (client_ip, client_port) = connection.peer_addr()
                            .map(|peer_addr| match peer_addr {
                                SocketAddr::V4(v4) => (v4.ip().to_string(), v4.port()),
                                SocketAddr::V6(v6) => (v6.ip().to_string(), v6.port()),
                            })
                            .unwrap_or_else(|err| (format!("<unknown -- err:{err}>"), 0));
                        trace!("`reactive-messaging::CompositeSocketServer`: ROUTING the client {client_ip}:{client_port} of the server @ {interface_ip}:{port} to another processor");
                        if let Err(_) = sender.send(connection).await {
                            error!("`reactive-messaging::CompositeSocketServer`: BUG(?) in server @ {interface_ip}:{port} while re-routing the client {client_ip}:{client_port}'s socket: THE NEW (ROUTED) PROCESSOR CAN NO LONGER RECEIVE CONNECTIONS -- THE CONNECTION WILL BE DROPPED");
                            break
                        }
                    },
                    None => {
                        if let Err(err) = connection.shutdown().await {
                            let (client_ip, client_port) = connection.peer_addr()
                                .map(|peer_addr| match peer_addr {
                                    SocketAddr::V4(v4) => (v4.ip().to_string(), v4.port()),
                                    SocketAddr::V6(v6) => (v6.ip().to_string(), v6.port()),
                                })
                                .unwrap_or_else(|err| (format!("<unknown -- err:{err}>"), 0));
                            error!("`reactive-messaging::CompositeSocketServer`: ERROR in server @ {interface_ip}:{port} while shutting down the socket with client {client_ip}:{client_port}: {err}");
                        }
                    }
                }
            }
            // loop ended
            trace!("`reactive-messaging::CompositeSocketServer`: The 'Connection Routing Task' for server @ {interface_ip}:{port} ended -- hopefully, due to a graceful server termination.");
        });
        Ok(())
    }

    fn termination_waiter(&mut self) -> Box< dyn FnOnce() -> BoxFuture<'static, Result<(), Box<dyn std::error::Error + Send + Sync>>> > {
        let mut local_termination_receiver = self.processor_termination_complete_receivers.take();
        let interface_ip = self.interface_ip.clone();
        let port = self.port;
        Box::new(move || Box::pin(async move {
            let Some(local_termination_receiver) = local_termination_receiver.take() else {
                return Err(Box::from(format!("GenericCompositeSocketServer::termination_waiter(): termination requested for server @ {interface_ip}:{port}, but the server was not started (or a previous termination was commanded) at the moment the `termination_waiter()`'s returned closure was called")))
            };
            for (i, processor_termination_complete_receiver) in local_termination_receiver.into_iter().enumerate() {
                if let Err(err) = processor_termination_complete_receiver.await {
                    error!("GenericCompositeSocketServer::termination_waiter(): It is no longer possible to tell when the processor {i} will be termination for server @ {interface_ip}:{port}: `one_shot` signal error: {err}")
                }
            }
            Ok(())
        }))
    }

    async fn terminate(mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match self.connection_provider.take() {
            Some(connection_provider) => {
                warn!("GenericCompositeSocketServer: Termination asked & initiated for server @ {}:{}", self.interface_ip, self.port);
                connection_provider.shutdown().await;
                Ok(())
            }
            None => {
                Err(Box::from("GenericCompositeSocketServer: Termination requested, but the service was not started -- no `self.start_with_*()` was called. Ignoring..."))
            }
        }
    }
}


/// Unit tests the [socket_server](self) module
#[cfg(any(test,doc))]
mod tests {
    use super::*;
    use crate::prelude::*;
    use std::{
        future,
        ops::Deref,
        sync::atomic::{AtomicU32, Ordering::Relaxed},
        time::Duration,
    };
    use std::sync::atomic::AtomicBool;
    use serde::{Deserialize, Serialize};
    use futures::StreamExt;
    use tokio::sync::Mutex;


    /// The interface for listening to connections
    const LISTENING_INTERFACE: &str = "127.0.0.1";
    /// The start of the port range for the tests in this module -- so not to clash with other modules when tests are run in parallel
    const PORT_START: u16 = 8040;


    /// Test that our instantiation macro for single protocol servers is able to produce servers backed by all possible channel types
    #[cfg_attr(not(doc),test)]
    fn single_server_instantiation() {
        let atomic_server = new_atomic_server!(
            ConstConfig {
                ..ConstConfig::default()
            },
            LISTENING_INTERFACE, PORT_START, String, String);

        let fullsync_server  = new_fullsync_server!(
            ConstConfig {
                ..ConstConfig::default()
            },
            LISTENING_INTERFACE, PORT_START+1, String, String);

        let crossbeam_server = new_crossbeam_server!(
            ConstConfig {
                ..ConstConfig::default()
            },
            LISTENING_INTERFACE, PORT_START+2, String, String);
    }

    /// Test that our instantiation macro for composite protocol servers is able to produce servers backed by all possible channel types
    #[cfg_attr(not(doc),test)]
    fn composite_server_instantiation() {
        let atomic_server = new_composite_atomic_server!(
            ConstConfig {
                ..ConstConfig::default()
            },
            LISTENING_INTERFACE, PORT_START+3, String, String, ());

        let fullsync_server  = new_composite_fullsync_server!(
            ConstConfig {
                ..ConstConfig::default()
            },
            LISTENING_INTERFACE, PORT_START+4, String, String, ());

        let crossbeam_server = new_composite_crossbeam_server!(
            ConstConfig {
                ..ConstConfig::default()
            },
            LISTENING_INTERFACE, PORT_START+5, String, String, ());
    }

    /// Test that our server types are ready for usage
    /// (showcases the "single protocol" case)
    #[cfg_attr(not(doc),tokio::test)]
    async fn doc_usage() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

        const PORT: u16 = PORT_START+6;     // All the following servers are started on this same port, ensuring the `terminate()` proceedings are working fine

        // demonstrates how to build an unresponsive server
        ///////////////////////////////////////////////////
        // using fully typed generic functions that will work with all possible configs
        let mut server = new_socket_server!(
            ConstConfig::default(),
            LISTENING_INTERFACE,
            PORT,
            DummyClientAndServerMessages,
            DummyClientAndServerMessages);
        start_unresponsive_server_processor!(server,
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
            client_messages_stream.map(|_payload| ())
        }
        let termination_waiter = server.termination_waiter();
        server.terminate().await?;
        termination_waiter().await?;

        // demonstrates how to build a responsive server
        ////////////////////////////////////////////////
        // using fully typed generic functions that will work with all possible configs
        let mut server = new_socket_server!(
            ConstConfig::default(),
            LISTENING_INTERFACE,
            PORT,
            DummyClientAndServerMessages,
            DummyClientAndServerMessages);
        start_responsive_server_processor!(server,
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

        let termination_waiter = server.termination_waiter();
        server.terminate().await?;
        termination_waiter().await?;

        // demonstrates how to use it with closures -- also allowing for any channel in the configs
        ///////////////////////////////////////////////////////////////////////////////////////////
        let mut server = new_socket_server!(
            ConstConfig::default(),
            LISTENING_INTERFACE,
            PORT,
            DummyClientAndServerMessages,
            DummyClientAndServerMessages);
        start_unresponsive_server_processor!(server,
            |_| future::ready(()),
            |_, _, _, client_messages_stream| client_messages_stream.map(|_payload| DummyClientAndServerMessages::FloodPing)
        )?;
        let termination_waiter = server.termination_waiter();
        server.terminate().await?;
        termination_waiter().await?;

        // demonstrates how to use the internal & generic implementation
        ////////////////////////////////////////////////////////////////
        // notice there may be a discrepancy in the `ConstConfig` you provide and the actual concrete types
        // you also provide for `UniProcessor` and `SenderChannel` -- therefore, this usage is not recommended
        const CONFIG: ConstConfig = ConstConfig {
            receiver_buffer:      2048,
            sender_buffer:        1024,
            executor_instruments: reactive_mutiny::prelude::Instruments::LogsWithExpensiveMetrics,
            ..ConstConfig::default()
        };
        type ProcessorUniType = UniZeroCopyFullSync<DummyClientAndServerMessages, {CONFIG.receiver_buffer as usize}, 1, {CONFIG.executor_instruments.into()}>;
        type SenderChannelType = ChannelUniMoveFullSync<DummyClientAndServerMessages, {CONFIG.sender_buffer as usize}, 1>;
        let mut server = CompositeSocketServer :: <{CONFIG.into()},
                                                                         DummyClientAndServerMessages,
                                                                         DummyClientAndServerMessages,
                                                                         ProcessorUniType,
                                                                         SenderChannelType,
                                                                         ()>
                                                                     :: new(LISTENING_INTERFACE, PORT);
        let connection_channel = server.spawn_unresponsive_processor(
            |_| future::ready(()),
            |_, _, _, client_messages_stream| client_messages_stream.map(|_payload| DummyClientAndServerMessages::FloodPing)
        ).await?;
        server.start_with_single_protocol(connection_channel).await?;
        let termination_waiter = server.termination_waiter();
        server.terminate().await?;
        termination_waiter().await?;

        Ok(())
    }

    /// assures the termination process is able to:
    ///   1) communicate with all clients
    ///   2) wait for up to the given timeout for them to gracefully disconnect
    ///   3) forcibly disconnect, if needed
    ///   4) notify any waiter on the server (after all the above steps are done) within the given timeout
    #[cfg_attr(not(doc),tokio::test(flavor = "multi_thread"))]
    async fn termination_process() {
        const PORT: u16 = PORT_START+7;

        // the tolerance, in milliseconds -- a too small termination duration means the server didn't wait for the client's disconnection; too much (possibly eternal) means it didn't enforce the timeout
        let max_time_ms = 20;

        // sensors
        let client_received_messages_count_ref1 = Arc::new(AtomicU32::new(0));
        let client_received_messages_count_ref2 = Arc::clone(&client_received_messages_count_ref1);
        let server_received_messages_count_ref1 = Arc::new(AtomicU32::new(0));
        let server_received_messages_count_ref2 = Arc::clone(&server_received_messages_count_ref1);

        // start the server -- the test logic is here
        let client_peer_ref1 = Arc::new(Mutex::new(None));
        let client_peer_ref2 = Arc::clone(&client_peer_ref1);

        const CONFIG: ConstConfig = ConstConfig {
            ..ConstConfig::default()
        };
        type ProcessorUniType = UniZeroCopyFullSync<DummyClientAndServerMessages, {CONFIG.receiver_buffer as usize}, 1, {CONFIG.executor_instruments.into()}>;
        type SenderChannelType = ChannelUniMoveFullSync<DummyClientAndServerMessages, {CONFIG.sender_buffer as usize}, 1>;
        let mut server = CompositeSocketServer :: <{CONFIG.into()},
                                                                         DummyClientAndServerMessages,
                                                                         DummyClientAndServerMessages,
                                                                         ProcessorUniType,
                                                                         SenderChannelType,
                                                                         () >
                                                                     :: new(LISTENING_INTERFACE, PORT);
        let connection_channel = server.spawn_responsive_processor(
            move |connection_event: ConnectionEvent<{CONFIG.into()}, DummyClientAndServerMessages, SenderChannelType>| {
                let client_peer = Arc::clone(&client_peer_ref1);
                async move {
                    match connection_event {
                        ConnectionEvent::PeerConnected { peer } => {
                            // register the client -- which will initiate the server termination further down in this test
                            client_peer.lock().await.replace(peer);
                        },
                        ConnectionEvent::PeerDisconnected { peer: _, stream_stats: _ } => (),
                        ConnectionEvent::LocalServiceTermination => {
                            // send a message to the client (the first message, actually... that will initiate a flood of back-and-forth messages)
                            // then try to close the connection (which would only be gracefully done once all messages were sent... which may never happen).
                            let client_peer = client_peer.lock().await;
                            let client_peer = client_peer.as_ref().expect("No client is connected");
                            // send the flood starting message
                            let _ = client_peer.send_async(DummyClientAndServerMessages::FloodPing).await;
                            client_peer.flush_and_close(Duration::ZERO).await;
                        }
                    }
                }
            },
            move |_, _, _, client_messages: MessagingMutinyStream<ProcessorUniType>| {
                let server_received_messages_count = Arc::clone(&server_received_messages_count_ref1);
                client_messages.map(move |client_message| {
                    std::mem::forget(client_message);   // TODO 2023-07-15: investigate this reactive-mutiny/rust related bug: it seems OgreUnique doesn't like the fact that this type doesn't need dropping? (no internal strings)... or is it a reactive-messaging bug?
                    server_received_messages_count.fetch_add(1, Relaxed);
                    DummyClientAndServerMessages::FloodPing
                })
            }
        ).await.expect("Spawning a server processor");
        server.start_with_single_protocol(connection_channel).await.expect("Starting the server");

        // start a client that will engage in a flood ping with the server when provoked (never closing the connection)
        let mut client = new_socket_client!(
            ConstConfig::default(),
            LISTENING_INTERFACE,
            PORT,
            DummyClientAndServerMessages,
            DummyClientAndServerMessages);
        start_responsive_client_processor!(
            client,
            |_| async {},
            move |_, _, _, server_messages| {
                let client_received_messages_count = Arc::clone(&client_received_messages_count_ref1);
                server_messages.map(move |server_message| {
                    std::mem::forget(server_message);   // TODO 2023-07-15: investigate this reactive-mutiny/rust related bug: it seems OgreUnique doesn't like the fact that this type doesn't need dropping? (no internal strings)... or is it a reactive-messaging bug?
                    client_received_messages_count.fetch_add(1, Relaxed);
                    DummyClientAndServerMessages::FloodPing
                })
            }
        ).expect("Starting the client");

        // wait for the client to connect
        while client_peer_ref2.lock().await.is_none() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        // terminate the server & wait until the shutdown process is complete
        let wait_for_server_termination = server.termination_waiter();
        server.terminate().await
            .expect("ERROR Signaling the server of the termination intention");
        let start = std::time::SystemTime::now();
        _ = tokio::time::timeout(Duration::from_secs(5), wait_for_server_termination()).await
            .expect("TIMED OUT (>5s) Waiting for the server to live it's life and to complete the termination process");
        let elapsed_ms = start.elapsed().unwrap().as_millis();
        assert!(client_received_messages_count_ref2.load(Relaxed) > 1, "The client didn't receive any messages (not even the 'server is shutting down' notification)");
        assert!(server_received_messages_count_ref2.load(Relaxed) > 1, "The server didn't receive any messages (not even 'gracefully disconnecting' after being notified that the server is shutting down)");
        assert!(elapsed_ms <= max_time_ms as u128,
                "The server termination (of a never complying client) didn't complete in a reasonable time, meaning the termination code is wrong. Maximum acceptable time: {}ms; Measured Time: {}ms",
                max_time_ms, elapsed_ms);
    }

    /// assures the "Composite Protocol Stacking" pattern is supported & correctly implemented:
    ///   1) New server connections are always handled by the first processor
    ///   2) Connections can be routed freely among processors
    ///   3) "Last States" are taken into account, enabling the "connection routing closure"
    ///   4) Connections can be closed after the last processor are through with them
    /// -- for these, all processors (but the first) will answer with a "welcome message" (this is the suggested behavior for servers).
    #[cfg_attr(not(doc),tokio::test(flavor = "multi_thread"))]
    async fn composite_protocol_stacking_pattern() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

        const PORT: u16 = PORT_START+8;

        let mut server = new_composite_socket_server!(
            ConstConfig::default(),
            LISTENING_INTERFACE,
            PORT,
            String,
            String,
            Protocols );

        enum Protocols {
            IncomingClient,
            WelcomeAuthenticatedFriend,
            AccountSettings,
            GoodbyeOptions,
            Disconnect,
        }
        impl Default for Protocols {
            fn default() -> Self {
                Self::IncomingClient
            }
        }

        // first level processors shouldn't do anything until the client says something meaningful -- newcomers must know, a priori, who they are talking to (a security measure)
        let incoming_client_processor_greeted = Arc::new(AtomicBool::new(false));
        let incoming_client_processor_greeted_ref = Arc::clone(&incoming_client_processor_greeted);
        let incoming_client_processor = server.spawn_unresponsive_processor(
            |_| future::ready(()),
            move |_, _, peer, client_messages_stream| {
                let incoming_client_processor_greeted_ref = Arc::clone(&incoming_client_processor_greeted_ref);
                client_messages_stream.then(move |_payload| {
                    let peer = Arc::clone(&peer);
                    incoming_client_processor_greeted_ref.store(true, Relaxed);
                    async move {
                        peer.send_async(format!("`IncomingClient`: New peer {peer:?} ended up initial authentication proceedings. SAY SOMETHING and you will be routed to 'WelcomeAuthenticatedFriend'")).await
                            .expect("Sending failed");
                        peer.set_state(Protocols::WelcomeAuthenticatedFriend).await;
                        peer.flush_and_close(Duration::from_secs(1)).await;
                    }
                })
            }
        ).await?;

        // deeper processors should inform the client that they are now subjected to a new processor / protocol, so they may adjust accordingly
        let welcome_authenticated_friend_processor_greeted = Arc::new(AtomicBool::new(false));
        let welcome_authenticated_friend_processor_greeted_ref = Arc::clone(&welcome_authenticated_friend_processor_greeted);
        let welcome_authenticated_friend_processor = server.spawn_unresponsive_processor(
            |connection_event| async {
                match connection_event {
                    ConnectionEvent::PeerConnected { peer } =>
                        peer.send_async(format!("`WelcomeAuthenticatedFriend`: Now dealing with client {peer:?}. SAY SOMETHING and you will be routed to 'AccountSettings'")).await
                            .expect("Sending failed"),
                    _ => (),
                }
            },
            move |_, _, peer, client_messages_stream| {
                let welcome_authenticated_friend_processor_greeted_ref = Arc::clone(&welcome_authenticated_friend_processor_greeted_ref);
                client_messages_stream.then(move |_payload| {
                    let peer = Arc::clone(&peer);
                    welcome_authenticated_friend_processor_greeted_ref.store(true, Relaxed);
                    async move {
                        peer.set_state(Protocols::AccountSettings).await;
                        peer.flush_and_close(Duration::from_secs(1)).await;
                    }
                })
            }
        ).await?;

        let account_settings_processor_greeted = Arc::new(AtomicBool::new(false));
        let account_settings_processor_greeted_ref = Arc::clone(&account_settings_processor_greeted);
        let account_settings_processor = server.spawn_unresponsive_processor(
            |connection_event| async {
                match connection_event {
                    ConnectionEvent::PeerConnected { peer } =>
                        peer.send_async(format!("`AccountSettings`: Now dealing with client {peer:?}. SAY SOMETHING and you will be routed to 'GoodbyeOptions'")).await
                            .expect("Sending failed"),
                    _ => (),
                }
            },
            move |_, _, peer, client_messages_stream| {
                let account_settings_processor_greeted_ref = Arc::clone(&account_settings_processor_greeted_ref);
                client_messages_stream.then(move |_payload| {
                    let peer = Arc::clone(&peer);
                    account_settings_processor_greeted_ref.store(true, Relaxed);
                    async move {
                        peer.set_state(Protocols::GoodbyeOptions).await;
                        peer.flush_and_close(Duration::from_secs(1)).await;
                    }
                })
            }
        ).await?;

        let goodbye_options_processor_greeted = Arc::new(AtomicBool::new(false));
        let goodbye_options_processor_greeted_ref = Arc::clone(&goodbye_options_processor_greeted);
        let goodbye_options_processor = server.spawn_unresponsive_processor(
            |connection_event| async {
                match connection_event {
                    ConnectionEvent::PeerConnected { peer } =>
                        peer.send_async(format!("`GoodbyeOptions`: Now dealing with client {peer:?}. SAY SOMETHING and you will be DISCONNECTED, as our talking is over. Thank you.")).await
                            .expect("Sending failed"),
                    _ => (),
                }
            },
            move |_, _, peer, client_messages_stream| {
                let goodbye_options_processor_greeted_ref = Arc::clone(&goodbye_options_processor_greeted_ref);
                client_messages_stream.then(move |_payload| {
                    let peer = Arc::clone(&peer);
                    goodbye_options_processor_greeted_ref.store(true, Relaxed);
                    async move {
                        peer.set_state(Protocols::Disconnect).await;
                        peer.flush_and_close(Duration::from_secs(1)).await;
                    }
                })
            }
        ).await?;

        // this closure will route the connections based on the states the processors above had set
        // (it will be called whenever a protocol processor ends -- "returning" the connection)
        let connection_routing_closure = move |_connection: &TcpStream, last_state: Option<Protocols>|
            if let Some(last_state) = last_state {
                match last_state {
                    Protocols::IncomingClient             => Some(incoming_client_processor.clone_sender()),
                    Protocols::WelcomeAuthenticatedFriend => Some(welcome_authenticated_friend_processor.clone_sender()),
                    Protocols::AccountSettings            => Some(account_settings_processor.clone_sender()),
                    Protocols::GoodbyeOptions             => Some(goodbye_options_processor.clone_sender()),
                    Protocols::Disconnect                 => None,
                }
            } else {
                Some(incoming_client_processor.clone_sender())
            };
        server.start_with_routing_closure(connection_routing_closure).await?;
        let server_termination_waiter = server.termination_waiter();

        // start the client that will only connect and listen to messages until it is disconnected
        let mut client = new_socket_client!(
            ConstConfig::default(),
            LISTENING_INTERFACE,
            PORT,
            String,
            String);
        start_responsive_client_processor!(
            client,
            |connection_event| async {
                match connection_event {
                    ConnectionEvent::PeerConnected { peer }                        => peer.send_async(String::from("Hello! Am I in?")).await.expect("Sending failed"),
                    ConnectionEvent::PeerDisconnected { peer: _, stream_stats: _ } => (),
                    ConnectionEvent::LocalServiceTermination                       => (),
                }
            },
            move |_, _, _, server_messages| server_messages.map(|msg| {
                println!("RECEIVED: {msg} -- answering with 'OK'");
                String::from("OK")
            })
        )?;

        let client_waiter = client.termination_waiter();
        // wait for the client to do its stuff
        _ = tokio::time::timeout(Duration::from_secs(5), client_waiter()).await
            .expect("TIMED OUT (>5s) Waiting for the client & server to do their stuff & disconnect the client");

        // terminate the server & wait until the shutdown process is complete
        server.terminate().await?;
        server_termination_waiter().await?;

        assert!(incoming_client_processor_greeted.load(Relaxed),              "`IncomingClient` processor wasn't requested");
        assert!(welcome_authenticated_friend_processor_greeted.load(Relaxed), "`WelcomeAuthenticatedFriend` processor wasn't requested");
        assert!(account_settings_processor_greeted.load(Relaxed),             "`AccountSettings` processor wasn't requested");
        assert!(goodbye_options_processor_greeted.load(Relaxed),              "`GoodbyeOptions` processor wasn't requested");

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
                .expect("composite_socket_server.rs unit tests: No errors should have happened here!")
        }
        #[inline(always)]
        fn processor_error_message(err: String) -> DummyClientAndServerMessages {
            panic!("composite_socket_server.rs unit tests: protocol error when none should have happened: {err}");
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