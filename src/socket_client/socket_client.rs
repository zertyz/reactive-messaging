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
//! Instead of using the mentioned macros, you might want take a look at [CompositeGenericSocketClient] to access the inner implementation directly
//! -- both ways have the same flexibility, but the macro version takes in all parameters in the conveniently packed and documented [ConstConfig]
//! struct, instead of requiring several const generic parameters.
//!
//! IMPLEMENTATION NOTE: There would be a common trait (that is no longer present) binding [CompositeSocketClient] to [CompositeGenericSocketClient]
//!                      -- the reason being Rust 1.71 still not supporting zero-cost async in traits.
//!                      Other API designs had been attempted, but using async in traits (through the `async-trait` crate) and the
//!                      necessary GATs to provide zero-cost-abstracts, revealed a compiler bug making writing processors very difficult to compile:
//!                        - https://github.com/rust-lang/rust/issues/96865

/// Instantiates & allocates resources for a [CompositeGenericSocketClient], ready to be later started by [spawn_unresponsive_client_processor!()] or [spawn_responsive_client_processor!()].\
/// Params:
///   - `const_config`: [ConstConfig] -- the configurations for the server, enforcing const time optimizations;
///   - `interface_ip: IntoString` -- the interface to listen to incoming connections;
///   - `port: u16` -- the port to listen to incoming connections;
///   - `RemoteMessages`: [ReactiveMessagingDeserializer<>] -- the type of the messages produced by the clients;
///   - `LocalMessage`: [ReactiveMessagingSerializer<>] -- the type of the messages produced by this server -- should, additionally, implement the `Default` trait.


use crate::{
    socket_client::common::upgrade_to_connection_event_tracking,
    types::{
        ConnectionEvent,
        MessagingMutinyStream,
        ResponsiveMessages,
    },
    socket_connection::{
        peer::Peer,
        socket_connection_handler::SocketConnectionHandler,
    },
    ReactiveMessagingDeserializer,
    ReactiveMessagingSerializer,
    config::Channels,
};
use std::{
    fmt::Debug,
    future::Future,
    marker::PhantomData,
    sync::{
        Arc,
        atomic::AtomicBool,
    },
};
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
use futures::{future::BoxFuture, Stream};
use tokio::{
    io::AsyncWriteExt,
    net::TcpStream,
};
use log::{trace, warn, error};


/// Instantiates & allocates resources for a stateless [GenericCompositeSocketClient] (suitable for single protocol communications),
/// ready to be later started by [start_unresponsive_client_processor!()] or [start_responsive_client_processor!()].\
/// Params:
///   - `const_config`: [ConstConfig] -- the configurations for the client, enforcing const/compile time optimizations;
///   - `interface_ip: IntoString` -- the interface to listen to incoming connections;
///   - `port: u16` -- the port to listen to incoming connections;
///   - `remote_messages`: [ReactiveMessagingDeserializer<>] -- the type of the messages produced by the server;
///   - `local_messages`: [ReactiveMessagingSerializer<>] -- the type of the messages produced by this client -- should, additionally, implement the `Default` trait.
/// See [new_composite_socket_client!()] if you want to use the "Composite Protocol Stacking" pattern.
#[macro_export]
macro_rules! new_socket_client {
    ($const_config:    expr,
     $ip:              expr,
     $port:            expr,
     $remote_messages: ty,
     $local_messages:  ty) => {
        new_composite_socket_client!($const_config, $ip, $port, $remote_messages, $local_messages, ())
    }
}
pub use new_socket_client;


/// Instantiates & allocates resources for a stateful [CompositeGenericSocketClient] (suitable for the "Composite Protocol Stacking" pattern),
/// ready to have processors added by [spawn_unresponsive_client_processor!()] or [spawn_responsive_client_processor!()]
/// and to be later started by [GenericCompositeSocketClient::start_with_routing_closure].\
/// Params:
///   - `const_config`: [ConstConfig] -- the configurations for the client, enforcing const/compile time optimizations;
///   - `ip: IntoString` -- the tcp ip to connect to;
///   - `port: u16` -- the tcp port to connect to;
///   - `remote_messages`: [ReactiveMessagingDeserializer<>] -- the type of the messages produced by the server;
///   - `local_messages`: [ReactiveMessagingSerializer<>] -- the type of the messages produced by this client -- should, additionally, implement the `Default` trait.
///   - `state_type: Default` -- The state type used by the "connection routing closure" (to be provided) to promote the "Composite Protocol Stacking" pattern
/// See [new_socket_client!()] if you want to use the "Composite Protocol Stacking" pattern.
#[macro_export]
macro_rules! new_composite_socket_client {
    ($const_config:    expr,
     $ip:              expr,
     $port:            expr,
     $remote_messages: ty,
     $local_messages:  ty,
     $state_type:      ty) => {{
        const _CONFIG:                    u64   = $const_config.into();
        const _PROCESSOR_BUFFER:          usize = $const_config.receiver_buffer as usize;
        const _PROCESSOR_UNI_INSTRUMENTS: usize = $const_config.executor_instruments.into();
        const _SENDER_BUFFER:             usize = $const_config.sender_buffer   as usize;
        match $const_config.channel {
            Channels::Atomic => CompositeSocketClient::<_CONFIG,
                                                        $remote_messages,
                                                        $local_messages,
                                                        $state_type,
                                                        _PROCESSOR_BUFFER,
                                                        _PROCESSOR_UNI_INSTRUMENTS,
                                                        _SENDER_BUFFER>
                                                     ::Atomic(CompositeGenericSocketClient::<_CONFIG,
                                                                                             $remote_messages,
                                                                                             $local_messages,
                                                                                             UniZeroCopyAtomic<$remote_messages, _PROCESSOR_BUFFER, 1, _PROCESSOR_UNI_INSTRUMENTS>,
                                                                                             ChannelUniMoveAtomic<$local_messages, _SENDER_BUFFER, 1>,
                                                                                             $state_type >
                                                                                          ::new($ip, $port) ),
            Channels::FullSync => CompositeSocketClient::<_CONFIG,
                                                          $remote_messages,
                                                          $local_messages,
                                                          $state_type,
                                                          _PROCESSOR_BUFFER,
                                                          _PROCESSOR_UNI_INSTRUMENTS,
                                                          _SENDER_BUFFER>
                                                       ::FullSync(CompositeGenericSocketClient::<_CONFIG,
                                                                                                 $remote_messages,
                                                                                                 $local_messages,
                                                                                                 UniZeroCopyFullSync<$remote_messages, _PROCESSOR_BUFFER, 1, _PROCESSOR_UNI_INSTRUMENTS>,
                                                                                                 ChannelUniMoveFullSync<$local_messages, _SENDER_BUFFER, 1>,
                                                                                                 $state_type >
                                                                                              ::new($ip, $port) ),
            Channels::Crossbeam => CompositeSocketClient::<_CONFIG,
                                                           $remote_messages,
                                                           $local_messages,
                                                           $state_type,
                                                           _PROCESSOR_BUFFER,
                                                           _PROCESSOR_UNI_INSTRUMENTS,
                                                           _SENDER_BUFFER>
                                                        ::Crossbeam(CompositeGenericSocketClient::<_CONFIG,
                                                                                                   $remote_messages,
                                                                                                   $local_messages,
                                                                                                   UniMoveCrossbeam<$remote_messages, _PROCESSOR_BUFFER, 1, _PROCESSOR_UNI_INSTRUMENTS>,
                                                                                                   ChannelUniMoveCrossbeam<$local_messages, _SENDER_BUFFER, 1>,
                                                                                                   $state_type >
                                                                                                ::new($ip, $port) ),
        }
    }}
}
pub use new_composite_socket_client;


/// Starts a client (previously instantiated by [new_socket_client!()]) that will communicate with the server using a single protocol -- as defined by the given
/// `dialog_processor_builder_fn`, a builder of "unresponsive" `Stream`s as specified in [GenericCompositeSocketClient::spawn_unresponsive_processor()].\
/// If you want to follow the "Composite Protocol Stacking" pattern, see the [spawn_unresponsive_composite_client_processor!()] macro instead.
#[macro_export]
macro_rules! start_unresponsive_client_processor {
    ($socket_server:                expr,
     $connection_events_handler_fn: expr,
     $dialog_processor_builder_fn:  expr) => {{
        let connection_channel = spawn_unresponsive_composite_client_processor!($socket_server, $connection_events_handler_fn, $dialog_processor_builder_fn)?;
        $socket_server.start_with_single_protocol(connection_channel).await
    }}
}
pub use start_unresponsive_client_processor;


/// To be used by a server following the "Composite Protocol Stacking" pattern -- which should have been previously instantiated by [new_composite_socket_client!()] --
/// adds & spawns another protocol processor given by the provided `dialog_processor_builder_fn`, a builder of "unresponsive" `Stream`s,
/// specified in [GenericCompositeSocketClient::spawn_unresponsive_processor()].\
/// If you want to use a single protocol in your client, use [spawn_unresponsive_client_processor!()] macro instead.
#[macro_export]
macro_rules! spawn_unresponsive_client_processor {
    ($socket_server:                expr,
     $connection_events_handler_fn: expr,
     $dialog_processor_builder_fn:  expr) => {{
        match &mut $socket_server {
            CompositeSocketClient::Atomic    (generic_composite_socket_client)  => generic_composite_socket_client.spawn_unresponsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await,
            CompositeSocketClient::FullSync  (generic_composite_socket_client)  => generic_composite_socket_client.spawn_unresponsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await,
            CompositeSocketClient::Crossbeam (generic_composite_socket_client)  => generic_composite_socket_client.spawn_unresponsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await,
        }
    }}
}
pub use spawn_unresponsive_client_processor;


/// Starts a client (previously instantiated by [new_socket_client!()]) that will communicate with the server using a single protocol -- as defined by the given
/// `dialog_processor_builder_fn`, a builder of "responsive" `Stream`s as specified in [GenericCompositeSocketClient::spawn_responsive_processor()].\
/// If you want to follow the "Composite Protocol Stacking" pattern, see the [spawn_responsive_composite_client_processor!()] macro instead.
#[macro_export]
macro_rules! start_responsive_client_processor {
    ($socket_server:                expr,
     $connection_events_handler_fn: expr,
     $dialog_processor_builder_fn:  expr) => {{
        let connection_channel = spawn_responsive_composite_server_processor!($socket_server, $connection_events_handler_fn, $dialog_processor_builder_fn)?;
        $socket_server.start_with_single_protocol(connection_channel).await
    }}
}
pub use start_responsive_client_processor;


/// To be used by a client following the "Composite Protocol Stacking" pattern -- which should have been previously instantiated by [new_composite_socket_client!()] --
/// adds & spawns another protocol processor given by the provided `dialog_processor_builder_fn`, a builder of "responsive" `Stream`s,
/// specified in [GenericCompositeSocketClient::spawn_responsive_processor()].\
/// If you want to use a single protocol in your client, use the [spawn_responsive_client_processor!()] macro instead.
#[macro_export]
macro_rules! spawn_responsive_client_processor {
    ($socket_client:                expr,
     $connection_events_handler_fn: expr,
     $dialog_processor_builder_fn:  expr) => {{
        match &mut $socket_client {
            CompositeSocketClient::Atomic    (generic_composite_socket_client)  => generic_composite_socket_client.spawn_responsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await,
            CompositeSocketClient::FullSync  (generic_composite_socket_client)  => generic_composite_socket_client.spawn_responsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await,
            CompositeSocketClient::Crossbeam (generic_composite_socket_client)  => generic_composite_socket_client.spawn_responsive_processor($connection_events_handler_fn, $dialog_processor_builder_fn).await,
        }
    }}
}
pub use spawn_responsive_client_processor;
use crate::socket_connection::connection_provider::{ClientConnectionManager, ConnectionChannel};


/// Represents a client built out of `CONFIG` (a `u64` version of [ConstConfig], from which the other const generic parameters derive).\
/// You probably don't want to instantiate this struct directly -- use the sugared [new_socket_client!()] or [new_composite_socket_client!()] macros instead.
pub enum CompositeSocketClient<const CONFIG:                    u64,
                               RemoteMessages:                  ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug           + 'static,
                               LocalMessages:                   ReactiveMessagingSerializer<LocalMessages>    + Send + Sync + PartialEq + Debug + Default + 'static,
                               StateType:                                                                       Send + Sync + Default                     + 'static,
                               const PROCESSOR_BUFFER:          usize,
                               const PROCESSOR_UNI_INSTRUMENTS: usize,
                               const SENDER_BUFFER:             usize> {

    Atomic(CompositeGenericSocketClient::<CONFIG,
                                          RemoteMessages,
                                          LocalMessages,
                                          UniZeroCopyAtomic<RemoteMessages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                          ChannelUniMoveAtomic<LocalMessages, SENDER_BUFFER, 1>,
                                          StateType >),

    FullSync(CompositeGenericSocketClient::<CONFIG,
                                            RemoteMessages,
                                            LocalMessages,
                                            UniZeroCopyFullSync<RemoteMessages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                            ChannelUniMoveFullSync<LocalMessages, SENDER_BUFFER, 1>,
                                            StateType >),

    Crossbeam(CompositeGenericSocketClient::<CONFIG,
                                             RemoteMessages,
                                             LocalMessages,
                                             UniMoveCrossbeam<RemoteMessages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                             ChannelUniMoveCrossbeam<LocalMessages, SENDER_BUFFER, 1>,
                                             StateType >),
}
 impl<const CONFIG:                    u64,
      RemoteMessages:                  ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug           + 'static,
      LocalMessages:                   ReactiveMessagingSerializer<LocalMessages>    + Send + Sync + PartialEq + Debug + Default + 'static,
      StateType:                                                                       Send + Sync + Default                     + 'static,
      const PROCESSOR_BUFFER:          usize,
      const PROCESSOR_UNI_INSTRUMENTS: usize,
      const SENDER_BUFFER:             usize>
 CompositeSocketClient<CONFIG, RemoteMessages, LocalMessages, StateType, PROCESSOR_BUFFER, PROCESSOR_UNI_INSTRUMENTS, SENDER_BUFFER> {

     /// See [GenericCompositeSocketClient::start_with_single_protocol()]
     pub async fn start_with_single_protocol(&mut self, connection_channel: ConnectionChannel)
                                            -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
         match self {
             CompositeSocketClient::Atomic    (composite_socket_client) => composite_socket_client.start_with_single_protocol(connection_channel).await,
             CompositeSocketClient::FullSync  (composite_socket_client) => composite_socket_client.start_with_single_protocol(connection_channel).await,
             CompositeSocketClient::Crossbeam (composite_socket_client) => composite_socket_client.start_with_single_protocol(connection_channel).await,
         }
     }

     /// See [GenericCompositeSocketClient::start_with_routing_closure()]
     pub async fn start_with_routing_closure(&mut self,
                                             protocol_stacking_closure: impl FnMut(/*connection: */& tokio::net::TcpStream, /*last_state: */Option<StateType>) -> Option<tokio::sync::mpsc::Sender<TcpStream>> + Send + 'static)
                                             -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
         match self {
             CompositeSocketClient::Atomic    (composite_socket_client) => composite_socket_client.start_with_routing_closure(protocol_stacking_closure).await,
             CompositeSocketClient::FullSync  (composite_socket_client) => composite_socket_client.start_with_routing_closure(protocol_stacking_closure).await,
             CompositeSocketClient::Crossbeam (composite_socket_client) => composite_socket_client.start_with_routing_closure(protocol_stacking_closure).await,
         }
     }

     /// See [CompositeGenericSocketClient::termination_waiter()]
     pub fn termination_waiter(&mut self) -> Box<dyn FnOnce() -> BoxFuture<'static, Result<(), Box<dyn std::error::Error + Send + Sync>>> > {
         match self {
             CompositeSocketClient::Atomic    (composite_socket_client) => Box::new(composite_socket_client.termination_waiter()),
             CompositeSocketClient::FullSync  (composite_socket_client) => Box::new(composite_socket_client.termination_waiter()),
             CompositeSocketClient::Crossbeam (composite_socket_client) => Box::new(composite_socket_client.termination_waiter()),
         }
     }

     /// See [CompositeGenericSocketClient::terminate()]
     pub fn terminate(self, timeout_ms: u32) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
         match self {
             CompositeSocketClient::Atomic    (composite_socket_client) => composite_socket_client.terminate(timeout_ms),
             CompositeSocketClient::FullSync  (composite_socket_client) => composite_socket_client.terminate(timeout_ms),
             CompositeSocketClient::Crossbeam (composite_socket_client) => composite_socket_client.terminate(timeout_ms),
         }
     }
}


/// Real definition & implementation for our Socket Client, full of generic parameters.\
/// Probably you want to instantiate this structure through the sugared macros [new_socket_client!()] or [new_composite_socket_client!()] instead.
/// Generic Parameters:
///   - `CONFIG`:           the `u64` version of the [ConstConfig] instance used to build this struct -- from which `ProcessorUniType` and `SenderChannel` derive;
///   - `RemoteMessages`:   the messages that are generated by the server (usually an `enum`);
///   - `LocalMessages`:    the messages that are generated by this client (usually an `enum`);
///   - `ProcessorUniType`: an instance of a `reactive-mutiny`'s [Uni] type (using one of the zero-copy channels) --
///                         This [Uni] will execute the given client reactive logic for each incoming message (see how it is used in [new_socket_client!()]);
///   - `SenderChannel`:    an instance of a `reactive-mutiny`'s Uni movable `Channel`, which will provide a `Stream` of messages to be sent to the server;
///   - `StateType`:        The state type used by the "connection routing closure" (to be provided), enabling the "Composite Protocol Stacking" pattern.
pub struct CompositeGenericSocketClient<const CONFIG:        u64,
                                        RemoteMessages:      ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
                                        LocalMessages:       ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
                                        ProcessorUniType:    GenericUni<ItemType=RemoteMessages>                                         + Send + Sync                     + 'static,
                                        SenderChannel:       FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync,
                                        StateType:                                                                                         Send + Sync + Default           + 'static> {

    /// false if a disconnection happened, as tracked by the socket logic
    connected: Arc<AtomicBool>,
    /// The interface to listen to incoming connections
    ip: String,
    /// The port to listen to incoming connections
    port: u16,
    /// Signaler to stop this client
    client_termination_signaler: Option<tokio::sync::oneshot::Sender<u32>>,
    /// Signaler to cause [Self::termination_waiter()]'s closure to return
    local_termination_is_complete_receiver: Option<tokio::sync::oneshot::Receiver<()>>,
    /// The client connection is returned by processors after they are done with it -- this connection
    /// may be routed to another processor if the "Composite Protocol Stacking" pattern is in play.
    returned_connection_source: Option<tokio::sync::mpsc::Receiver<(TcpStream, Option<StateType>)>>,
    returned_connection_sink: tokio::sync::mpsc::Sender<(TcpStream, Option<StateType>)>,
    _phantom: PhantomData<(RemoteMessages,LocalMessages,ProcessorUniType,SenderChannel)>
}
impl<const CONFIG:        u64,
     RemoteMessages:      ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:       ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:    GenericUni<ItemType=RemoteMessages>                                         + Send + Sync                     + 'static,
     SenderChannel:       FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync                     + 'static,
     StateType:                                                                                         Send + Sync + Default           + 'static>
CompositeGenericSocketClient<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannel, StateType> {

    /// Instantiates a client to connect to a TCP/IP Server:
    ///   `ip`:                   the server IP to connect to
    ///   `port`:                 the server port to connect to
    pub fn new<IntoString: Into<String>>
              (ip:   IntoString,
               port: u16)
              -> Self {
        let (returned_connection_sink, returned_connection_source) = tokio::sync::mpsc::channel::<(TcpStream, Option<StateType>)>(2);
        Self {
            connected:                               Arc::new(AtomicBool::new(false)),
            ip:                                      ip.into(),
            port,
            client_termination_signaler:             None,
            local_termination_is_complete_receiver:  None,
            returned_connection_source:              Some(returned_connection_source),
            returned_connection_sink,
            _phantom:                                PhantomData,
        }
    }

    /// Start the client with a single processor (after calling either [Self::spawn_unresponsive_processor()]
    /// or [Self::spawn_responsive_processor()] once) -- A.K.A. "The Single Protocol Mode".\
    /// See [Self::start_with_routing_closure()] if you want a client that shares the connection among
    /// different protocol processors.
    ///
    /// Starts the client -- effectively connecting to the server -- using the provided `connection_channel` to
    /// distribute the connection to the available processors.
    pub async fn start_with_single_protocol(&mut self, connection_channel: ConnectionChannel)
                                           -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
        // this closure will cause just-opened connections to be sent to `connection_channel` and returned connections to be dropped
        let connection_routing_closure = move |_connection: &TcpStream, last_state: Option<StateType>|
            last_state.map_or_else(|| Some(connection_channel.clone_sender()),
                                   |_| None);
        self.start_with_routing_closure(connection_routing_closure).await

    }

    /// Starts the client -- effectively connecting to the server -- using the provided `connection_routing_closure` to
    /// distribute the connection among the configured processors -- previously fed in by [Self::spawn_responsive_processor()] &
    /// [Self::spawn_unresponsive_processor()].
    ///
    /// `protocol_stacking_closure := FnMut(connection: &tokio::net::TcpStream, last_state: Option<StateType>) -> connection_receiver: Option<tokio::sync::mpsc::Receiver<TcpStream>>`
    ///
    /// -- this closure "decides what to do" with the connection, routing it to the appropriate processor:
    ///   - The just-opened connection will have `last_state` set to `None` -- otherwise, it will either be set by the processor
    ///     before the [Peer] is closed -- see [Peer::set_state()] -- or will have the `Default` value.
    ///   - The returned value must be one of the "handles" returned by [Self::spawn_responsive_processor()] or
    ///     [Self::spawn_unresponsive_processor()].
    ///   - If `None` is returned, the connection will be closed and this client will cease its operations.
    ///
    /// This method returns an error if no processors were configured.
    pub async fn start_with_routing_closure(&mut self,
                                            mut connection_routing_closure: impl FnMut(/*connection: */& tokio::net::TcpStream, /*last_state: */Option<StateType>) -> Option<tokio::sync::mpsc::Sender<TcpStream>> + Send + 'static)
                                            -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

        let mut connection_manager = ClientConnectionManager::<CONFIG>::new(&self.ip, self.port);
        let connection = connection_manager.connect_retryable().await
            .map_err(|err| format!("Error making client connection to {}:{} -- {err}", self.ip, self.port))?;
        let mut just_opened_connection = Some(connection);


        let mut returned_connection_source = self.returned_connection_source.take()
            .ok_or_else(|| String::from("couldn't move the 'Returned Connection Source' out of the Connection Channel"))?;

        let ip = self.ip.clone();
        let port = self.port;

        // Spawns the "connection routing task" to:
        //   - Listen to newly incoming connections as well as upgraded/downgraded ones shared between processors
        //   - Gives them to the `protocol_stacking_closure`
        //   - Routes them to the right processor or close the connection
        tokio::spawn(async move {

            loop {
                let (mut connection, sender) = match just_opened_connection.take() {
                    // process the just-opened connection (once)
                    Some(just_opened_connection) => {
                        let sender = connection_routing_closure(&just_opened_connection, None);
                        (just_opened_connection, sender)
                    },
                    // process connections returned by the processors (after they ended processing them)
                    None => {
                        let Some((returned_connection, last_state)) = returned_connection_source.recv().await else { break };
                        let sender = connection_routing_closure(&returned_connection, last_state);
                        (returned_connection, sender)
                    },
                };

                // route the connection to another processor or drop it
                match sender {
                    Some(sender) => {
                        trace!("`reactive-messaging::CompositeSocketClient`: ROUTING the connection with the server @ {ip}:{port} to another processor");
                        if let Err(err) = sender.send(connection).await {
                            error!("`reactive-messaging::CompositeSocketClient`: BUG(?) in the client connected to the server @ {ip}:{port} while re-routing the connection: THE NEW (ROUTED) PROCESSOR CAN NO LONGER RECEIVE CONNECTIONS -- THE CONNECTION WILL BE DROPPED");
                            break
                        }
                    },
                    None => {
                        if let Err(err) = connection.shutdown().await {
                            error!("`reactive-messaging::CompositeSocketClient`: ERROR in the client connected to the server @ {ip}:{port} while shutting down the connection: {err}");
                        }
                    }
                }
            }
            // loop ended
            trace!("`reactive-messaging::CompositeSocketClient`: The 'Connection Routing Task' for the client connected to the server @ {ip}:{port} ended -- hopefully, due to a graceful client termination.");
        });
        Ok(())
    }

    /// Spawns a task to start the local client, returning immediately.\
    /// The given `dialog_processor_builder_fn` will be called when the connection is established and should return a `Stream`
    /// that will produce non-futures & non-fallible items that **won't be sent to the server**:
    ///   - `connection_events_callback`: -- a generic function (or closure) to handle connected, disconnected and termination events. Sign it as:
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
    pub async fn spawn_unresponsive_processor<OutputStreamItemsType:                                                                                                                                                                                                                                                Send + Sync + Debug       + 'static,
                                              ServerStreamType:               Stream<Item=OutputStreamItemsType>                                                                                                                                                                                                  + Send                      + 'static,
                                              ConnectionEventsCallbackFuture: Future<Output=()>                                                                                                                                                                                                                   + Send                      + 'static,
                                              ConnectionEventsCallback:       Fn(/*event: */ConnectionEvent<CONFIG, LocalMessages, SenderChannel, StateType>)                                                                                                                 -> ConnectionEventsCallbackFuture + Send + Sync               + 'static,
                                              ProcessorBuilderFn:             Fn(/*server_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<CONFIG, LocalMessages, SenderChannel, StateType>>, /*server_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> ServerStreamType               + Send + Sync               + 'static>

                                             (&mut self,
                                              connection_events_callback:  ConnectionEventsCallback,
                                              dialog_processor_builder_fn: ProcessorBuilderFn)

                                             -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

        let (client_termination_sender, client_termination_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (local_termination_is_complete_sender, local_termination_is_complete_receiver) = tokio::sync::oneshot::channel::<()>();
        self.client_termination_signaler = Some(client_termination_sender);
        self.local_termination_is_complete_receiver = Some(local_termination_is_complete_receiver);
        let ip = self.ip.clone();
        let port = self.port;

        let connection_events_callback = upgrade_to_connection_event_tracking(&self.connected, local_termination_is_complete_sender, connection_events_callback);

        let mut connection_manager = ClientConnectionManager::<CONFIG>::new(&ip, port);
        let connection = connection_manager.connect_retryable().await
            .map_err(|err| format!("Error making client connection to {}:{} -- {err}", self.ip, self.port))?;
        tokio::spawn(async move {
            let socket_communications_handler = SocketConnectionHandler::<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannel, StateType>::new();
            let result = socket_communications_handler.client_for_unresponsive_text_protocol(connection,
                                                                                             client_termination_receiver,
                                                                                             connection_events_callback,
                                                                                             dialog_processor_builder_fn).await
                .map_err(|err| format!("Error while executing the dialog processor: {err}"));
            match result {
                Ok((mut socket, _)) => {
                    if let Err(err) = socket.shutdown().await {
                        error!("`reactive-messaging::SocketClient`: ERROR shutting down the socket (after the unresponsive & textual processor ended) @ {ip}:{port}: {err}");
                    }
                }
                Err(err) => {
                    error!("`reactive-messaging::SocketClient`: ERROR in client (unresponsive & textual) @ {ip}:{port}: {err}");
                }
            }
        });
        Ok(())
    }

    /// Spawns a task to start the local client, returning immediately,
    /// The given `dialog_processor_builder_fn` will be called when the connection is established and should return a `Stream`
    /// that will produce non-futures & non-fallible items that *will be sent to the server*:
    ///   - `connection_events_callback`: -- a generic function (or closure) to handle connected, disconnected and termination events. Sign it as:
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
    pub async fn spawn_responsive_processor<ServerStreamType:                Stream<Item=LocalMessages>                                                                                                                                                                                                                 + Send        + 'static,
                                            ConnectionEventsCallbackFuture:  Future<Output=()>                                                                                                                                                                                                                          + Send        + 'static,
                                            ConnectionEventsCallback:        Fn(/*event: */ConnectionEvent<CONFIG, LocalMessages, SenderChannel, StateType>)                                                                                                                 -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                            ProcessorBuilderFn:              Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<CONFIG, LocalMessages, SenderChannel, StateType>>, /*client_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> ServerStreamType               + Send + Sync + 'static>

                                           (&mut self,
                                            connection_events_callback:  ConnectionEventsCallback,
                                            dialog_processor_builder_fn: ProcessorBuilderFn)

                                           -> Result<(), Box<dyn std::error::Error + Sync + Send>>

                                           where LocalMessages: ResponsiveMessages<LocalMessages> {

        let (server_termination_sender, server_termination_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (local_termination_is_complete_sender, local_termination_is_complete_receiver) = tokio::sync::oneshot::channel::<()>();
        self.client_termination_signaler = Some(server_termination_sender);
        self.local_termination_is_complete_receiver = Some(local_termination_is_complete_receiver);
        let ip = self.ip.clone();
        let port = self.port;

        let connection_events_callback = upgrade_to_connection_event_tracking(&self.connected, local_termination_is_complete_sender, connection_events_callback);

        let mut connection_manager = ClientConnectionManager::<CONFIG>::new(&ip, port);
        let connection = connection_manager.connect_retryable().await
            .map_err(|err| format!("Error making client connection to {}:{} -- {err}", self.ip, self.port))?;
        tokio::spawn(async move {
            let socket_communications_handler = SocketConnectionHandler::<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannel, StateType>::new();
            let result = socket_communications_handler.client_for_responsive_text_protocol(connection,
                                                                                           server_termination_receiver,
                                                                                           connection_events_callback,
                                                                                           dialog_processor_builder_fn).await
                .map_err(|err| format!("Error while executing the dialog processor: {err}"));
            match result {
                Ok((mut socket, _)) => {
                    if let Err(err) = socket.shutdown().await {
                        trace!("`reactive-messaging::SocketClient`: COULDN'T shutdown the socket (after the responsive & textual processor ended) @ {ip}:{port}: {err}");
                        // ... it may have already been done by the other end
                    }
                }
                Err(err) => {
                    error!("`reactive-messaging::SocketClient`: ERROR in client (responsive & textual) @ {ip}:{port}: {err}");
                }
            }
        });
        Ok(())
    }

    /// Returns an async closure that blocks until [Self::terminate()] is called.
    /// Example:
    /// ```no_compile
    ///     self.spawn_the_client(logic...);
    ///     self.termination_waiter()().await;
    pub fn termination_waiter(&mut self) -> impl FnOnce() -> BoxFuture<'static, Result<(), Box<dyn std::error::Error + Send + Sync>>> {
        let mut local_termination_receiver = self.local_termination_is_complete_receiver.take();
        move || Box::pin({
            async move {
                if let Some(local_termination_receiver) = local_termination_receiver.take() {
                    match local_termination_receiver.await {
                        Ok(()) => {
                            Ok(())
                        },
                        Err(err) => Err(Box::from(format!("GenericSocketClient::termination_waiter(): It is no longer possible to tell when the client will be terminated: `one_shot` signal error: {err}")))
                    }
                } else {
                    Err(Box::from("GenericSocketClient: \"wait for termination\" requested, but the client was not started (or a previous service termination was commanded) at the moment `termination_waiter()` was called"))
                }
            }
        })
    }

    /// Notifies the client it is time to terminate.\
    /// It is a good practice that the `connection_events_handler()` you provided when starting the client
    /// informs the server that a client-initiated disconnection (due to the call to this method) is happening
    /// -- and the protocol is encouraged to support that.
    pub fn terminate(mut self, timeout_ms: u32) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match self.client_termination_signaler.take() {
            Some(server_sender) => {
                warn!("GenericSocketClient: Shutdown asked & initiated for client connected @ {}:{} -- timeout: {timeout_ms}ms", self.ip, self.port);
                if let Err(_sent_value) = server_sender.send(timeout_ms) {
                    Err(Box::from("GenericSocketServer BUG: couldn't send termination signal to the network loop. Program is, likely, hanged. Please, investigate and fix!"))
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
    use crate::{config::ConstConfig, ron_deserializer, ron_serializer};
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
        assert!(matches!(atomic_client, CompositeSocketClient::Atomic(_)), "an Atomic Client couldn't be instantiated");

        let fullsync_client = new_socket_client!(
            ConstConfig {
                channel: Channels::FullSync,
                ..ConstConfig::default()
            },
            REMOTE_SERVER, 443, String, String);
        assert!(matches!(fullsync_client, CompositeSocketClient::FullSync(_)), "a FullSync Client couldn't be instantiated");

        let crossbeam_client = new_socket_client!(
            ConstConfig {
                channel: Channels::Crossbeam,
                ..ConstConfig::default()
            },
            REMOTE_SERVER, 443, String, String);
        assert!(matches!(crossbeam_client, CompositeSocketClient::Crossbeam(_)), "a Crossbeam Client couldn't be instantiated");
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
            client_messages_stream.map(|_payload| ())
        }
        let wait_for_termination = client.termination_waiter();
        client.terminate(200)?;
        wait_for_termination().await?;

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
        let wait_for_termination = client.termination_waiter();
        client.terminate(200)?;
        wait_for_termination().await?;

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
        let wait_for_termination = client.termination_waiter();
        client.terminate(200)?;
        wait_for_termination().await?;

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
        let mut client = CompositeGenericSocketClient :: <{CONFIG.into()},
                                                                                     DummyClientAndServerMessages,
                                                                                     DummyClientAndServerMessages,
                                                                                     ProcessorUniType,
                                                                                     SenderChannelType,
                                                                                     () >
                                                                                 :: new("66.45.249.218",443);
        client.spawn_unresponsive_processor(
            |_| future::ready(()),
            |_, _, _, client_messages_stream| client_messages_stream.map(|_payload| DummyClientAndServerMessages::FloodPing)
        ).await?;
        let wait_for_termination = client.termination_waiter();
        client.terminate(200)?;
        wait_for_termination().await?;

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
                .expect("socket_client.rs unit tests: No errors should have happened here!")
        }
        #[inline(always)]
        fn processor_error_message(err: String) -> DummyClientAndServerMessages {
            panic!("socket_client.rs unit tests: protocol error when none should have happened: {err}");
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