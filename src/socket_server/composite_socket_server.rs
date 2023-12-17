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
//! Instead of using the mentioned macros, you might want take a look at [GenericCompositeSocketServer] to access the inner implementation directly
//! -- both ways have the same flexibility, but the macro version takes in all parameters in the conveniently packed and documented [ConstConfig]
//! struct, instead of requiring several const generic parameters.
//!
//! IMPLEMENTATION NOTE: There would be a common trait (that is no longer present) binding [SocketServer] to [GenericCompositeSocketServer]
//!                      -- the reason being Rust 1.71 still not supporting zero-cost async fn in traits.
//!                      Other API designs had been attempted, but using async in traits (through the `async-trait` crate) and the
//!                      necessary GATs to provide zero-cost-abstracts, revealed a compiler bug making writing processors very difficult to compile:
//!                        - https://github.com/rust-lang/rust/issues/96865


use crate::{
    socket_connection::{
        peer::Peer,
        socket_connection_handler::SocketConnectionHandler,
        connection_provider::{ServerConnectionHandler,ConnectionChannel},
    },
    socket_server::common::upgrade_to_shutdown_tracking,
    types::{
        ConnectionEvent,
        MessagingMutinyStream,
        ResponsiveMessages,
    },
    ReactiveMessagingDeserializer,
    ReactiveMessagingSerializer,
    config::Channels,
};
use std::{
    fmt::Debug,
    future::Future,
    marker::PhantomData,
    sync::Arc,
};
use std::net::SocketAddr;
use futures::{future::BoxFuture, Stream};
use reactive_mutiny::prelude::advanced::{
    ChannelUniMoveAtomic,
    ChannelUniMoveCrossbeam,
    ChannelUniMoveFullSync,
    UniMoveCrossbeam,
    UniZeroCopyAtomic,
    UniZeroCopyFullSync,
    FullDuplexUniChannel,
    GenericUni,
};
use log::{error, trace, warn};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::mpsc::Sender;


/// Instantiates & allocates resources for a [GenericCompositeSocketServer], ready to be later started by [spawn_unresponsive_server_processor!()] or [spawn_responsive_server_processor!()].\
/// Params:
///   - `const_config`: [ConstConfig] -- the configurations for the server, enforcing const time optimizations;
///   - `interface_ip: IntoString` -- the interface to listen to incoming connections;
///   - `port: u16` -- the port to listen to incoming connections;
///   - `RemoteMessages`: [ReactiveMessagingDeserializer<>] -- the type of the messages produced by the clients;
///   - `LocalMessage`: [ReactiveMessagingSerializer<>] -- the type of the messages produced by this server -- should, additionally, implement the `Default` trait.
#[macro_export]
macro_rules! new_composite_socket_server {
    ($const_config:    expr,
     $interface_ip:    expr,
     $port:            expr,
     $remote_messages: ty,
     $local_messages:  ty,
     $state_type:      ty) => {{
        const _CONFIG:                    u64   = $const_config.into();
        const _PROCESSOR_BUFFER:          usize = $const_config.receiver_buffer as usize;
        const _PROCESSOR_UNI_INSTRUMENTS: usize = $const_config.executor_instruments.into();
        const _SENDER_BUFFER:             usize = $const_config.sender_buffer   as usize;
        match $const_config.channel {
            Channels::Atomic => CompositeSocketServer::<_CONFIG,
                                               $remote_messages,
                                               $local_messages,
                                               $state_type,
                                               _PROCESSOR_BUFFER,
                                               _PROCESSOR_UNI_INSTRUMENTS,
                                               _SENDER_BUFFER,>
                                            ::Atomic(GenericCompositeSocketServer::<_CONFIG,
                                                                           $remote_messages,
                                                                           $local_messages,
                                                                           UniZeroCopyAtomic<$remote_messages, _PROCESSOR_BUFFER, 1, _PROCESSOR_UNI_INSTRUMENTS>,
                                                                           ChannelUniMoveAtomic<$local_messages, _SENDER_BUFFER, 1>,
                                                                           $state_type >
                                                                        ::new($interface_ip, $port) ),
            Channels::FullSync => CompositeSocketServer::<_CONFIG,
                                                 $remote_messages,
                                                 $local_messages,
                                                 $state_type,
                                                 _PROCESSOR_BUFFER,
                                                 _PROCESSOR_UNI_INSTRUMENTS,
                                                 _SENDER_BUFFER>
                                              ::FullSync(GenericCompositeSocketServer::<_CONFIG,
                                                                               $remote_messages,
                                                                               $local_messages,
                                                                               UniZeroCopyFullSync<$remote_messages, _PROCESSOR_BUFFER, 1, _PROCESSOR_UNI_INSTRUMENTS>,
                                                                               ChannelUniMoveFullSync<$local_messages, _SENDER_BUFFER, 1>,
                                                                               $state_type >
                                                                            ::new($interface_ip, $port) ),
            Channels::Crossbeam => CompositeSocketServer::<_CONFIG,
                                                  $remote_messages,
                                                  $local_messages,
                                                  $state_type,
                                                  _PROCESSOR_BUFFER,
                                                  _PROCESSOR_UNI_INSTRUMENTS,
                                                  _SENDER_BUFFER>
                                               ::Crossbeam(GenericCompositeSocketServer::<_CONFIG,
                                                                                 $remote_messages,
                                                                                 $local_messages,
                                                                                 UniMoveCrossbeam<$remote_messages, _PROCESSOR_BUFFER, 1, _PROCESSOR_UNI_INSTRUMENTS>,
                                                                                 ChannelUniMoveCrossbeam<$local_messages, _SENDER_BUFFER, 1>,
                                                                                 $state_type >
                                                                              ::new($interface_ip, $port) ),
        }
    }}
}
pub use new_composite_socket_server;


/// See [GenericCompositeSocketServer::spawn_unresponsive_processor()]
#[macro_export]
macro_rules! spawn_unresponsive_composite_server_processor {
    ($socket_server:                expr,
     $connection_events_handler_fn: expr,
     $dialog_processor_builder_fn:  expr) => {{
        match &mut $socket_server {
            CompositeSocketServer::Atomic    (generic_composite_socket_server)  =>  generic_composite_socket_server.spawn_unresponsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await,
            CompositeSocketServer::FullSync  (generic_composite_socket_server)  =>  generic_composite_socket_server.spawn_unresponsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await,
            CompositeSocketServer::Crossbeam (generic_composite_socket_server)  =>  generic_composite_socket_server.spawn_unresponsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await,
        }
    }}
}
pub use spawn_unresponsive_composite_server_processor;


/// See [GenericCompositeSocketServer::spawn_responsive_processor()]
#[macro_export]
macro_rules! spawn_responsive_composite_server_processor {
    ($socket_server:                expr,
     $connection_events_handler_fn: expr,
     $dialog_processor_builder_fn:  expr) => {{
        match &mut $socket_server {
            CompositeSocketServer::Atomic    (generic_composite_socket_server)  =>  generic_composite_socket_server.spawn_responsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await,
            CompositeSocketServer::FullSync  (generic_composite_socket_server)  =>  generic_composite_socket_server.spawn_responsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await,
            CompositeSocketServer::Crossbeam (generic_composite_socket_server)  =>  generic_composite_socket_server.spawn_responsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await,
        }
    }}
}
pub use spawn_responsive_composite_server_processor;


/// See [GenericCompositeSocketServer::start()]
#[macro_export]
macro_rules! start_composite_server {
    ($socket_server:        expr,
     $connection_router_fn: expr) => {{
        match &mut $socket_server {
            CompositeSocketServer::Atomic    (generic_composite_socket_server)  =>  generic_composite_socket_server.start($connection_router_fn).await,
            CompositeSocketServer::FullSync  (generic_composite_socket_server)  =>  generic_composite_socket_server.start($connection_router_fn).await,
            CompositeSocketServer::Crossbeam (generic_composite_socket_server)  =>  generic_composite_socket_server.start($connection_router_fn).await,
        }
    }}
}
pub use start_composite_server;


/// Represents a server built out of `CONFIG` (a `u64` version of [ConstConfig], from which the other const generic parameters derive).\
/// Don't instantiate this struct directly -- use [new_socket_server!()] instead.
pub enum CompositeSocketServer<const CONFIG:                    u64,
                      RemoteMessages:                  ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug           + 'static,
                      LocalMessages:                   ReactiveMessagingSerializer<LocalMessages>    + Send + Sync + PartialEq + Debug + Default + 'static,
                      StateType:                                                                       Send + Sync                               + 'static,
                      const PROCESSOR_BUFFER:          usize,
                      const PROCESSOR_UNI_INSTRUMENTS: usize,
                      const SENDER_BUFFER:             usize> {

    Atomic(GenericCompositeSocketServer::<CONFIG,
                                 RemoteMessages,
                                 LocalMessages,
                                 UniZeroCopyAtomic<RemoteMessages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                 ChannelUniMoveAtomic<LocalMessages, SENDER_BUFFER, 1>,
                                 StateType >),

    FullSync(GenericCompositeSocketServer::<CONFIG,
                                   RemoteMessages,
                                   LocalMessages,
                                   UniZeroCopyFullSync<RemoteMessages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                   ChannelUniMoveFullSync<LocalMessages, SENDER_BUFFER, 1>,
                                   StateType >),

    Crossbeam(GenericCompositeSocketServer::<CONFIG,
                                    RemoteMessages,
                                    LocalMessages,
                                    UniMoveCrossbeam<RemoteMessages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                    ChannelUniMoveCrossbeam<LocalMessages, SENDER_BUFFER, 1>,
                                    StateType >),
}
 impl<const CONFIG:                    u64,
      RemoteMessages:                  ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug           + 'static,
      LocalMessages:                   ReactiveMessagingSerializer<LocalMessages>    + Send + Sync + PartialEq + Debug + Default + 'static,
      StateType:                                                                       Send + Sync                               + 'static,
      const PROCESSOR_BUFFER:          usize,
      const PROCESSOR_UNI_INSTRUMENTS: usize,
      const SENDER_BUFFER:             usize>
 CompositeSocketServer<CONFIG, RemoteMessages, LocalMessages, StateType, PROCESSOR_BUFFER, PROCESSOR_UNI_INSTRUMENTS, SENDER_BUFFER> {

     /// See [GenericCompositeSocketServer::start()]
     pub async fn start(&mut self,
                        protocol_stacking_closure: impl FnMut(/*connection: */& tokio::net::TcpStream, /*last_state: */Option<StateType>) -> Option<tokio::sync::mpsc::Sender<TcpStream>> + Send + 'static)
                       -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
         match self {
             CompositeSocketServer::Atomic    (generic_socket_server) => generic_socket_server.start(protocol_stacking_closure).await,
             CompositeSocketServer::FullSync  (generic_socket_server) => generic_socket_server.start(protocol_stacking_closure).await,
             CompositeSocketServer::Crossbeam (generic_socket_server) => generic_socket_server.start(protocol_stacking_closure).await,
         }
     }


     /// See [GenericCompositeSocketServer::shutdown_waiter()]
     pub fn shutdown_waiter(&mut self) -> Box<dyn FnOnce() -> BoxFuture<'static, Result<(), Box<dyn std::error::Error + Send + Sync>>> > {
         match self {
             CompositeSocketServer::Atomic    (generic_socket_server) => Box::new(generic_socket_server.shutdown_waiter()),
             CompositeSocketServer::FullSync  (generic_socket_server) => Box::new(generic_socket_server.shutdown_waiter()),
             CompositeSocketServer::Crossbeam (generic_socket_server) => Box::new(generic_socket_server.shutdown_waiter()),
         }
     }

     /// See [GenericCompositeSocketServer::shutdown()]
     pub async fn shutdown(self) {
         match self {
             CompositeSocketServer::Atomic    (generic_socket_server) => generic_socket_server.shutdown().await,
             CompositeSocketServer::FullSync  (generic_socket_server) => generic_socket_server.shutdown().await,
             CompositeSocketServer::Crossbeam (generic_socket_server) => generic_socket_server.shutdown().await,
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
pub struct GenericCompositeSocketServer<const CONFIG:        u64,
                               RemoteMessages:      ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
                               LocalMessages:       ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
                               ProcessorUniType:    GenericUni<ItemType=RemoteMessages>                                         + Send + Sync                     + 'static,
                               SenderChannel:       FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync,
                               StateType:                                                                                         Send + Sync                     + 'static> {

    /// The interface to listen to incoming connections
    interface_ip: String,
    /// The port to listen to incoming connections
    port: u16,
    /// The abstraction containing the network loop that accepts connections for us + facilities to start processing already
    /// opened connections (enabling the "Composite Protocol Stacking" design pattern)
    connection_provider: Option<ServerConnectionHandler>,
    /// Signaler to cause [wait_for_shutdown()] to return
    processor_shutdown_complete_receivers:  Option<Vec<tokio::sync::oneshot::Receiver<()>>>,
    /// Connection returned by processors after they are done with them -- these connections
    /// may be route to another processor if "composite protocol stacking" is in play.
    returned_connections_source: Option<tokio::sync::mpsc::Receiver<(TcpStream, Option<StateType>)>>,
    returned_connections_sink: tokio::sync::mpsc::Sender<(TcpStream, Option<StateType>)>,
    _phantom: PhantomData<(RemoteMessages,LocalMessages,ProcessorUniType,SenderChannel, StateType)>
}
impl<const CONFIG:        u64,
     RemoteMessages:      ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:       ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:    GenericUni<ItemType=RemoteMessages>                                         + Send + Sync                     + 'static,
     SenderChannel:       FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync                     + 'static,
     StateType:                                                                                         Send + Sync                     + 'static>
GenericCompositeSocketServer<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannel, StateType> {

    /// Creates a new server instance listening on TCP/IP:
    ///   `interface_ip`:         the interface's IP to listen to -- 0.0.0.0 will cause listening to all network interfaces
    ///   `port`:                 what port to listen to
    pub fn new<IntoString: Into<String>>
              (interface_ip: IntoString,
               port:         u16)
              -> Self {
        let (returned_connections_sink, returned_connections_source) = tokio::sync::mpsc::channel::<(TcpStream, Option<StateType>)>(2);
        Self {
            interface_ip:                           interface_ip.into(),
            port,
            connection_provider:                    None,
            processor_shutdown_complete_receivers:  Some(vec![]),
            returned_connections_source:            Some(returned_connections_source),
            returned_connections_sink,
            _phantom:                               PhantomData,
        }
    }

    /// Start to listen to connections -- effectively starting the server -- using the provided `protocol_stacking_closure` to
    /// distribute the connections among the configured processors -- previously fed in by [Self::add_responsive_processor()] &
    /// [Self::add_unresponsive_processor()].
    ///
    /// `protocol_stacking_closure := FnMut(connection: &tokio::net::TcpStream, last_state: Option<StateType>) -> connection_receiver: Option<tokio::sync::mpsc::Receiver<TcpStream>>`
    ///
    /// -- this closure "decides what to do" with available connections, routing them to the appropriate processors:
    ///   - Newly received connections will have `last_state` set to None -- otherwise, this will be set by the processor
    ///     before the [Peer] is closed -- see [Peer::set_state()].
    ///   - The returned value must be one of the "handles" returned by [Self::add_responsive_processor()] or
    ///     [Self::add_unresponsive_processor()].
    ///   - If `None` is returned, the connection will be closed.
    ///
    /// This method returns an error in the following cases:
    ///   1) if the binding process fails;
    ///   2) if no processors were configured.
    pub async fn start(&mut self,
                       mut protocol_stacking_closure: impl FnMut(/*connection: */& tokio::net::TcpStream, /*last_state: */Option<StateType>) -> Option<tokio::sync::mpsc::Sender<TcpStream>> + Send + 'static)
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
                    // v = tokio => {},

                    // process newly incoming connections
                    new_connection = new_connections_source.recv() => {
                        let Some(new_connection) = new_connection else { break };
                        let sender = protocol_stacking_closure(&new_connection, None);
                        (new_connection, sender)
                    },

                    // process connections returned by the processors (after they ended processing them)
                    returned_connection_and_state = returned_connections_source.recv() => {
                        let Some((returned_connection, last_state)) = returned_connection_and_state else { break };
                        let sender = protocol_stacking_closure(&returned_connection, last_state);
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
                        if let Err(err) = sender.send(connection).await {
                            warn!("`reactive-messaging::CompositeSocketServer`: BUG(?) in server @ {interface_ip}:{port} while re-routing the client {client_ip}:{client_port}'s socket: THE NEW (ROUTED) PROCESSOR CAN NO LONGER RECEIVE CONNECTIONS -- THE CONNECTION WILL BE DROPPED");
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
            trace!("`reactive-messaging::CompositeSocketServer`: The 'Connection Routing Task' for server @ {interface_ip}:{port} ended -- hopefully, due to a graceful server shutdown.");
        });

        Ok(())
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
    pub async fn spawn_unresponsive_processor<OutputStreamItemsType:                                                                                                                                                                                                                                                Send + Sync + Debug       + 'static,
                                              ServerStreamType:               Stream<Item=OutputStreamItemsType>                                                                                                                                                                                                  + Send                      + 'static,
                                              ConnectionEventsCallbackFuture: Future<Output=()>                                                                                                                                                                                                                   + Send                      + 'static,
                                              ConnectionEventsCallback:       Fn(/*event: */ConnectionEvent<CONFIG, LocalMessages, SenderChannel, StateType>)                                                                                                                 -> ConnectionEventsCallbackFuture + Send + Sync               + 'static,
                                              ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<CONFIG, LocalMessages, SenderChannel, StateType>>, /*client_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> ServerStreamType               + Send + Sync               + 'static>

                                             (&mut self,
                                              connection_events_callback:  ConnectionEventsCallback,
                                              dialog_processor_builder_fn: ProcessorBuilderFn)

                                             -> Result<ConnectionChannel, Box<dyn std::error::Error + Sync + Send>> {

        // configure this processor's "shutdown is complete" signaler
        let (local_shutdown_sender, local_shutdown_receiver) = tokio::sync::oneshot::channel::<()>();
        self.processor_shutdown_complete_receivers.as_mut().expect("BUG!").push(local_shutdown_receiver);
        let connection_events_callback = upgrade_to_shutdown_tracking(local_shutdown_sender, connection_events_callback);

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
    pub async fn spawn_responsive_processor<ServerStreamType:                Stream<Item=LocalMessages>                                                                                                                                                                                                          + Send        + 'static,
                                            ConnectionEventsCallbackFuture:  Future<Output=()>                                                                                                                                                                                                                   + Send        + 'static,
                                            ConnectionEventsCallback:        Fn(/*event: */ConnectionEvent<CONFIG, LocalMessages, SenderChannel, StateType>)                                                                                                                 -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                            ProcessorBuilderFn:              Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<CONFIG, LocalMessages, SenderChannel, StateType>>, /*client_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> ServerStreamType               + Send + Sync + 'static>

                                           (&mut self,
                                            connection_events_callback:  ConnectionEventsCallback,
                                            dialog_processor_builder_fn: ProcessorBuilderFn)

                                           -> Result<ConnectionChannel, Box<dyn std::error::Error + Sync + Send>>

                                           where LocalMessages: ResponsiveMessages<LocalMessages> {

        // configure this processor's "shutdown is complete" signaler
        let (local_shutdown_sender, local_shutdown_receiver) = tokio::sync::oneshot::channel::<()>();
        self.processor_shutdown_complete_receivers.as_mut().expect("BUG!").push(local_shutdown_receiver);
        let connection_events_callback = upgrade_to_shutdown_tracking(local_shutdown_sender, connection_events_callback);

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

    /// Returns an async closure that blocks until [Self::shutdown()] is called.
    /// Example:
    /// ```no_compile
    ///     self.spawn_the_server(logic...);
    ///     self.shutdown_waiter()().await;
    pub fn shutdown_waiter(&mut self) -> impl FnOnce() -> BoxFuture<'static, Result<(), Box<dyn std::error::Error + Send + Sync>>> {
        let mut local_shutdown_receiver = self.processor_shutdown_complete_receivers.take();
        let interface_ip = self.interface_ip.clone();
        let port = self.port;
        move || Box::pin(async move {
            let Some(mut local_shutdown_receiver) = local_shutdown_receiver.take() else {
                return Err(Box::from(format!("GenericCompositeSocketServer::wait_for_shutdown(): shutdown requested for server @ {interface_ip}:{port}, but the server was not started (or a previous shutdown was commanded) at the moment the `shutdown_waiter()`'s returned closure was called")))
            };
            for (i, processor_shutdown_complete_receiver) in local_shutdown_receiver.into_iter().enumerate() {
                if let Err(err) = processor_shutdown_complete_receiver.await {
                    error!("GenericCompositeSocketServer::wait_for_shutdown(): It is no longer possible to tell when the processor {i} will be shutdown for server @ {interface_ip}:{port}: `one_shot` signal error: {err}")
                }
            }
            Ok(())
        })
    }

    /// Notifies the server it is time to shutdown.\
    /// A shutdown is considered graceful if it could be accomplished in less than `timeout_ms` milliseconds.\
    /// It is a good practice that the `connection_events_handler()` you provided when starting the server
    /// uses this time to inform all clients that a remote-initiated disconnection (due to a shutdown) is happening.
    pub async fn shutdown(mut self) {
        if !self.returned_connections_sink.is_closed() {
            warn!("GenericCompositeSocketServer: Shutdown asked & initiated for server @ {}:{}", self.interface_ip, self.port);
        } else {
            warn!("GenericCompositeSocketServer: BUG!! Shutdown asked for server @ {}:{}, but it seems the server is not running", self.interface_ip, self.port);
        }
    }

    /// Spawns a listener for the connections no longer issued by the dialog processors -- on this implementation,
    /// they will be simply closed (in opposition to the [CompositeSocketServer], where they may be reutilized)
    fn spawn_disconnection_task(&self, mut returned_connection_source: tokio::sync::mpsc::Receiver<(TcpStream, Option<StateType>)>) {
        let interface_ip = self.interface_ip.clone();
        let port = self.port;
        tokio::spawn(async move {
            while let Some((mut connection, _)) = returned_connection_source.recv().await {
                if let Err(err) = connection.shutdown().await {
                    let (client_ip, client_port) = connection.peer_addr()
                        .map(|peer_addr| match peer_addr {
                            SocketAddr::V4(v4) => (v4.ip().to_string(), v4.port()),
                            SocketAddr::V6(v6) => (v6.ip().to_string(), v6.port()),
                        })
                        .unwrap_or_else(|err| (format!("<unknown -- err:{err}>"), 0));
                    error!("`reactive-messaging::SocketServer`: ERROR in server @ {interface_ip}:{port} while shutting down the socket with client {client_ip}:{client_port}: {err}");
                }
            }
        });
    }
}


/// Unit tests the [socket_server](self) module
#[cfg(any(test,doc))]
mod tests {
    use super::*;
    use crate::prelude::{
        ConstConfig,
        SocketClient,
        GenericSocketClient,
        new_socket_client,
        ron_deserializer,
        ron_serializer,
        spawn_responsive_client_processor,
    };
    use std::{
        future,
        ops::Deref,
        sync::atomic::{AtomicU32, Ordering::Relaxed},
        time::Duration,
    };
    use serde::{Deserialize, Serialize};
    use futures::StreamExt;
    use tokio::sync::Mutex;


    /// The start of the port range for the tests in this module -- so not to clash with other modules when tests are run in parallel
    const PORT_START: u16 = 8070;


    /// Test that our instantiation macro is able to produce servers backed by all possible channel types
    #[cfg_attr(not(doc),test)]
    fn instantiation() {
        let atomic_server = new_composite_socket_server!(
            ConstConfig {
                channel: Channels::Atomic,
                ..ConstConfig::default()
            },
            "127.0.0.1", PORT_START+1, String, String, ());
        assert!(matches!(atomic_server, CompositeSocketServer::Atomic(_)), "an Atomic Server couldn't be instantiated");

        let fullsync_server  = new_composite_socket_server!(
            ConstConfig {
                channel: Channels::FullSync,
                ..ConstConfig::default()
            },
            "127.0.0.1", PORT_START+2, String, String, ());
        assert!(matches!(fullsync_server, CompositeSocketServer::FullSync(_)), "a FullSync Server couldn't be instantiated");

        let crossbeam_server = new_composite_socket_server!(
            ConstConfig {
                channel: Channels::Crossbeam,
                ..ConstConfig::default()
            },
            "127.0.0.1", PORT_START+3, String, String, ());
        assert!(matches!(crossbeam_server, CompositeSocketServer::Crossbeam(_)), "a Crossbeam Server couldn't be instantiated");
    }

    /// Test that our server types are ready for usage
    #[cfg_attr(not(doc),tokio::test)]
    async fn doc_usage() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

        // demonstrates how to build an unresponsive server
        ///////////////////////////////////////////////////
        // using fully typed generic functions that will work with all possible configs
        let mut server = new_composite_socket_server!(
            ConstConfig::default(),
            "127.0.0.1",
            PORT_START+4,
            DummyClientAndServerMessages,
            DummyClientAndServerMessages,
            () );
        let connection_channel = spawn_unresponsive_composite_server_processor!(server,
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
        server.start(move |_, last_state| last_state.map_or_else(|| Some(connection_channel.clone_sender()), |_| None)).await?;
        let shutdown_waiter = server.shutdown_waiter();
        server.shutdown().await;
        shutdown_waiter().await?;

        // demonstrates how to build a responsive server
        ////////////////////////////////////////////////
        // using fully typed generic functions that will work with all possible configs
        let mut server = new_composite_socket_server!(
            ConstConfig::default(),
            "127.0.0.1",
            PORT_START+4,
            DummyClientAndServerMessages,
            DummyClientAndServerMessages,
            () );
        let connection_channel = spawn_responsive_composite_server_processor!(server,
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
        server.start(move |_, last_state| last_state.map_or_else(|| Some(connection_channel.clone_sender()), |_| None)).await?;
        let shutdown_waiter = server.shutdown_waiter();
        server.shutdown().await;
        shutdown_waiter().await?;

        // demonstrates how to use it with closures -- also allowing for any channel in the configs
        ///////////////////////////////////////////////////////////////////////////////////////////
        let mut server = new_composite_socket_server!(
            ConstConfig::default(),
            "127.0.0.1",
            PORT_START+4,
            DummyClientAndServerMessages,
            DummyClientAndServerMessages,
            () );
        let connection_channel = spawn_unresponsive_composite_server_processor!(server,
            |_| future::ready(()),
            |_, _, _, client_messages_stream| client_messages_stream.map(|_payload| DummyClientAndServerMessages::FloodPing)
        )?;
        server.start(move |_, last_state| last_state.map_or_else(|| Some(connection_channel.clone_sender()), |_| None)).await?;
        let shutdown_waiter = server.shutdown_waiter();
        server.shutdown().await;
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
        let mut server = GenericCompositeSocketServer :: <{CONFIG.into()},
                                                                     DummyClientAndServerMessages,
                                                                     DummyClientAndServerMessages,
                                                                     ProcessorUniType,
                                                                     SenderChannelType,
                                                                     ()>
                                                                 :: new("127.0.0.1", PORT_START+5);
        let connection_channel = server.spawn_unresponsive_processor(
            |_| future::ready(()),
            |_, _, _, client_messages_stream| client_messages_stream.map(|_payload| DummyClientAndServerMessages::FloodPing)
        ).await?;
        server.start(move |_, last_state| last_state.map_or_else(|| Some(connection_channel.clone_sender()), |_| None)).await?;
        let shutdown_waiter = server.shutdown_waiter();
        server.shutdown().await;
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
        const PORT: u16 = PORT_START+6;

        // the shutdown timeout, in milliseconds
        let expected_max_shutdown_duration_ms = 543;
        // the tollerance, in milliseconds -- a too small shutdown duration means the server didn't wait for the client's disconnection; too much (possibly eternal) means it didn't enforce the timeout
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
            channel: Channels::FullSync,
            ..ConstConfig::default()
        };
        type ProcessorUniType = UniZeroCopyFullSync<DummyClientAndServerMessages, {CONFIG.receiver_buffer as usize}, 1, {CONFIG.executor_instruments.into()}>;
        type SenderChannelType = ChannelUniMoveFullSync<DummyClientAndServerMessages, {CONFIG.sender_buffer as usize}, 1>;
        let mut server = GenericCompositeSocketServer :: <{CONFIG.into()},
                                                                     DummyClientAndServerMessages,
                                                                     DummyClientAndServerMessages,
                                                                     ProcessorUniType,
                                                                     SenderChannelType,
                                                                     () >
                                                                 :: new("127.0.0.1", PORT);
        let connection_channel = server.spawn_responsive_processor(
            move |connection_event: ConnectionEvent<{CONFIG.into()}, DummyClientAndServerMessages, SenderChannelType>| {
                let client_peer = Arc::clone(&client_peer_ref1);
                async move {
                    match connection_event {
                        ConnectionEvent::PeerConnected { peer } => {
                            // register the client -- which will initiate the server shutdown further down in this test
                            client_peer.lock().await.replace(peer);
                        },
                        ConnectionEvent::PeerDisconnected { peer: _, stream_stats: _ } => (),
                        ConnectionEvent::ApplicationShutdown => {
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
                    std::mem::forget(client_message);   // TODO 2023-07-15: investigate this reactive-mutiny related bug: it seems OgreUnique doesn't like the fact that this type doesn't need dropping? (no internal strings)... or is it a reactive-messaging bug?
                    server_received_messages_count.fetch_add(1, Relaxed);
                    DummyClientAndServerMessages::FloodPing
                })
            }
        ).await.expect("Spawning a server processor");
        server.start(move |_, last_state| last_state.map_or_else(|| Some(connection_channel.clone_sender()), |_| None)).await.expect("Starting the server");

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
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        // shutdown the server & wait until the shutdown process is complete
        let wait_for_server_shutdown = server.shutdown_waiter();
        server.shutdown().await;
        let start = std::time::SystemTime::now();
        _ = tokio::time::timeout(Duration::from_secs(5), wait_for_server_shutdown()).await
            .expect("ERROR Waiting for the server to live it's life and to complete the shutdown process");
        let elapsed_ms = start.elapsed().unwrap().as_millis();
        assert!(client_received_messages_count_ref2.load(Relaxed) > 1, "The client didn't receive any messages (not even the 'server is shutting down' notification)");
        assert!(server_received_messages_count_ref2.load(Relaxed) > 1, "The server didn't receive any messages (not even 'gracefully disconnecting' after being notified that the server is shutting down)");
        assert!(elapsed_ms <= max_time_ms as u128,
                "The server shutdown (of a never complying client) didn't complete in a reasonable time, meaning the shutdown code is wrong. Maximum acceptable time: {}ms; Measured Time: {}ms",
                max_time_ms, elapsed_ms);
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