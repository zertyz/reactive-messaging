//! Resting place for [SocketServer]


use crate::ReactiveMessagingSerializer;
use super::{
    ResponsiveMessages,
    types::*,
    socket_connection_handler::{self, Peer},
    serde::ReactiveMessagingDeserializer,
};
use std::{
    sync::Arc,
    fmt::Debug,
    future::Future, time::Duration,
};
use std::marker::PhantomData;
use futures::{Stream, future::BoxFuture};
use log::{warn,error};
use reactive_mutiny::prelude::advanced::{Instruments, GenericUni, ChannelUniMoveAtomic, UniZeroCopyAtomic, FullDuplexUniChannel, UniZeroCopyFullSync, ChannelUniMoveFullSync, UniMoveCrossbeam, ChannelUniMoveCrossbeam};
use reactive_mutiny::prelude::{ChannelCommon, ChannelProducer};
use tokio::sync::Mutex;
use crate::config::{Channels, ConstConfig};
use crate::socket_connection_handler::SocketConnectionHandler;


pub struct ResponsiveSocketServer<const CONFIG:   usize,
                                  RemoteMessages: ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug + 'static,
                                  LocalMessages:  ReactiveMessagingSerializer<LocalMessages>    +
                                                  ResponsiveMessages<LocalMessages>             + Send + Sync + PartialEq + Debug + 'static> {

    /// The interface to listen to incoming connections
    interface_ip:                String,
    /// The port to listen to incoming connections
    port:                        u16,
}

impl<const CONFIG:   usize,
     RemoteMessages: ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:  ReactiveMessagingSerializer<LocalMessages>    +
                     ResponsiveMessages<LocalMessages>             + Send + Sync + PartialEq + Debug + 'static>
ResponsiveSocketServer<CONFIG, RemoteMessages, LocalMessages> {

    /// Spawns a task to run a Server listening @ `self`'s `interface_ip` & `port` and returns, immediately,
    /// an object through which the caller may inquire some stats (if opted in) and request the server to shutdown.\
    /// The given `dialog_processor_builder_fn` will be called for each new client and will return a `reactive-mutiny` Stream
    /// that will produce non-futures & non-fallibles `ServerMessages` that will be sent to the clients.
    pub async fn spawn_responsive_processor<ServerStreamType:               Stream<Item=LocalMessages>                   + Send + 'static,
                                            ConnectionEventsCallbackFuture: Future<Output=()>                            + Send,
                                            ConnectionEventsCallback:       Fn(/*server_event: */Self::ConnectionEventType)                                                                                                  -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                            ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<Self::SenderChannelType>>, /*client_messages_stream: */Self::StreamType) -> ServerStreamType               + Send + Sync + 'static>

                                           (mut self,
                                            connection_events_callback:  ConnectionEventsCallback,
                                            dialog_processor_builder_fn: ProcessorBuilderFn)

                                           -> Result<impl SocketServerController, Box<dyn std::error::Error + Sync + Send + 'static>> {
        const CONST_CONFIG: ConstConfig = ConstConfig::from(CONFIG);
        match CONST_CONFIG.channel {
            Channels::Atomic => {
                type SocketServer = CustomResponsiveSocketServer<CONFIG,
                                                                 {CONST_CONFIG.executor_instruments.into()},
                                                                 {CONST_CONFIG.receiver_buffer as usize},
                                                                 RemoteMessages,
                                                                 LocalMessages,
                                                                 UniZeroCopyAtomic<RemoteMessages, {CONST_CONFIG.receiver_buffer as usize}, {CONST_CONFIG.executor_instruments.into()}>,
                                                                 ChannelUniMoveAtomic<LocalMessages, {CONST_CONFIG.sender_buffer as usize}, 1> >;
                let server = SocketServer::new(self.interface_ip, self.port);
                server.spawn_responsive_processor(connection_events_callback, dialog_processor_builder_fn)
            },
            Channels::FullSync => {
                type SocketServer = CustomResponsiveSocketServer<CONFIG,
                                                                 {CONST_CONFIG.executor_instruments.into()},
                                                                 {CONST_CONFIG.receiver_buffer as usize},
                                                                 RemoteMessages,
                                                                 LocalMessages,
                                                                 UniZeroCopyFullSync<RemoteMessages, {CONST_CONFIG.receiver_buffer as usize}, {CONST_CONFIG.executor_instruments.into()}>,
                                                                 ChannelUniMoveFullSync<LocalMessages, {CONST_CONFIG.sender_buffer as usize}, 1> >;
                let server = SocketServer::new(self.interface_ip, self.port);
                server.spawn_responsive_processor(connection_events_callback, dialog_processor_builder_fn)
            },
            Channels::Crossbean => {
                type SocketServer = CustomResponsiveSocketServer<CONFIG,
                                                                 {CONST_CONFIG.executor_instruments.into()},
                                                                 {CONST_CONFIG.receiver_buffer as usize},
                                                                 RemoteMessages,
                                                                 LocalMessages,
                                                                 UniMoveCrossbeam<RemoteMessages, {CONST_CONFIG.receiver_buffer as usize}, {CONST_CONFIG.executor_instruments.into()}>,
                                                                 ChannelUniMoveCrossbeam<LocalMessages, {CONST_CONFIG.sender_buffer as usize}, 1> >;
                let server = SocketServer::new(self.interface_ip, self.port);
                server.spawn_responsive_processor(connection_events_callback, dialog_processor_builder_fn)
            },
        }
        todo!()
    }

}

/// The handle to define, start and shutdown a Reactive Server for Socket Connections.\
/// `BUFFERED_MESSAGES_PER_PEER_COUNT` is the number of messages that may be produced ahead of sending (to each client)
///                                    as well as the number of messages that this server may accumulate from each client before denying new ones
#[derive(Debug)]
pub struct CustomResponsiveSocketServer<const CONFIG:                         usize,
                                        const PROCESSOR_UNI_INSTRUMENTS:      usize,
                                        const PROCESSOR_BUFFER_SIZE_PER_PEER: usize,
                                        RemoteMessages:    ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug + 'static,
                                        LocalMessages:     ReactiveMessagingSerializer<LocalMessages>    +
                                                           ResponsiveMessages<LocalMessages>             + Send + Sync + PartialEq + Debug + 'static,
                                        ProcessorUniType:  GenericUni<PROCESSOR_UNI_INSTRUMENTS>,
                                        SenderChannelType: ChannelCommon<'static, LocalMessages, LocalMessages> + ChannelProducer<'static, LocalMessages, LocalMessages>> {
    /// The interface to listen to incoming connections
    interface_ip:                String,
    /// The port to listen to incoming connections
    port:                        u16,
    /// Signaler to stop the server
    server_shutdown_signaler:    Option<tokio::sync::oneshot::Sender<u32>>,
    /// Signaler to cause [wait_for_shutdown()] to return
    local_shutdown_receiver:     Option<tokio::sync::oneshot::Receiver<()>>,
    _phantom: PhantomData<(RemoteMessages,LocalMessages,ProcessorUniType,SenderChannelType)>
}

impl<const CONFIG:                         usize,
     const PROCESSOR_UNI_INSTRUMENTS:      usize,
     const PROCESSOR_BUFFER_SIZE_PER_PEER: usize,
     RemoteMessages:    ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:     ReactiveMessagingSerializer<LocalMessages>    +
                        ResponsiveMessages<LocalMessages>             + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:  GenericUni<PROCESSOR_UNI_INSTRUMENTS>,
     SenderChannelType: ChannelCommon<'static, LocalMessages, LocalMessages> + ChannelProducer<'static, LocalMessages, LocalMessages>>
CustomResponsiveSocketServer<CONFIG, PROCESSOR_UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE_PER_PEER, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannelType> {


    /// Creates a new server instance listening on TCP/IP:
    ///   `interface_ip`:         the interface's IP to listen to -- 0.0.0.0 will cause listening to all network interfaces
    ///   `port`:                 what port to listen to
    ///   `processor_builder_fn`: a function to instantiate a new processor `Stream` whenever a new connection arrives
    pub fn new<IntoString: Into<String>>
              (interface_ip: IntoString,
               port:         u16)
               -> Self {
        Self {
            interface_ip: interface_ip.into(),
            port,
            server_shutdown_signaler: None,
            local_shutdown_receiver: None,
            _phantom: PhantomData,
        }
    }

    /// Useful for Generic Programming, shares the internal generic parameter
    pub const fn uni_instruments() -> usize {
        Self::PROCESSOR_UNI_INSTRUMENTS
    }

    /// Spawns a task to run a Server listening @ `self`'s `interface_ip` & `port` and returns, immediately,
    /// an object through which the caller may inquire some stats (if opted in) and request the server to shutdown.\
    /// The given `dialog_processor_builder_fn` will be called for each new client and will return a `reactive-mutiny` Stream
    /// that will produce non-futures & non-fallibles `ServerMessages` that will be sent to the clients.
    pub async fn spawn_responsive_processor<ServerStreamType:               Stream<Item=LocalMessages>                   + Send + 'static,
                                            ConnectionEventsCallbackFuture: Future<Output=()>                            + Send,
                                            ConnectionEventsCallback:       Fn(/*server_event: */Self::ConnectionEventType)                                                                                                  -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                            ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<Self::SenderChannelType>>, /*client_messages_stream: */Self::StreamType) -> ServerStreamType               + Send + Sync + 'static>

                                           (mut self,
                                            connection_events_callback:  ConnectionEventsCallback,
                                            dialog_processor_builder_fn: ProcessorBuilderFn)

                                           -> Result<impl SocketServerController, Box<dyn std::error::Error + Sync + Send + 'static>> {

        let (server_shutdown_sender, server_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (local_shutdown_sender, local_shutdown_receiver) = tokio::sync::oneshot::channel::<()>();
        self.server_shutdown_signaler = Some(server_shutdown_sender);
        self.local_shutdown_receiver = Some(local_shutdown_receiver);
        let listening_interface = self.interface_ip.to_string();
        let port = self.port;

        let connection_events_callback = upgrade_to_shutdown_tracking(local_shutdown_sender, connection_events_callback);

        let socket_connection_handler = SocketConnectionHandler::<CONFIG, RemoteMessages, LocalMessages, Self::ProcessorUniType, Self::SenderChannelType, PROCESSOR_UNI_INSTRUMENTS>::new();
        socket_connection_handler.server_loop_for_responsive_text_protocol
            (listening_interface.to_string(),
             port,
             server_shutdown_receiver,
             connection_events_callback,
             dialog_processor_builder_fn).await
            .map_err(|err| Box::from(format!("Error starting SocketServer @ {listening_interface}:{port}: {:?}", err)))?;
        Ok(self)
    }

    /// Spawns a task to run a Server listening @ `self`'s `interface_ip` & `port` and returns, immediately,
    /// an object through which the caller may inquire some stats (if opted in) and request the server to shutdown.\
    /// The given `dialog_processor_builder_fn` will be called for each new client and will return a `reactive-mutiny` Stream
    /// that will produce non-futures & non-fallibles items that won't be sent to the clients
    /// -- if you want the processor to produce "answer messages" to the clients, see [SocketServer::spawn_responsive_processor()].
    pub async fn spawn_unresponsive_processor<OutputStreamItemsType:                                                          Send + Sync             + Debug + 'static,
                                              ServerStreamType:               Stream<Item=OutputStreamItemsType>            + Send + 'static,
                                              ConnectionEventsCallbackFuture: Future<Output=()>                             + Send,
                                              ConnectionEventsCallback:       Fn(/*server_event: */ConnectionEvent<Self::SenderChannelType>)                                                                                   -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                              ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<Self::SenderChannelType>>, /*client_messages_stream: */Self::StreamType) -> ServerStreamType               + Send + Sync + 'static>

                                             (mut self,
                                              connection_events_callback:  ConnectionEventsCallback,
                                              dialog_processor_builder_fn: ProcessorBuilderFn)

                                             -> Result<impl SocketServerController, Box<dyn std::error::Error + Sync + Send + 'static>> {

        let (server_shutdown_sender, server_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (local_shutdown_sender, local_shutdown_receiver) = tokio::sync::oneshot::channel::<()>();
        self.server_shutdown_signaler = Some(server_shutdown_sender);
        self.local_shutdown_receiver = Some(local_shutdown_receiver);
        let listening_interface = self.interface_ip.to_string();
        let port = self.port;

        let connection_events_callback = upgrade_to_shutdown_tracking(local_shutdown_sender, connection_events_callback);

        let socket_connection_handler = SocketConnectionHandler::<CONFIG, RemoteMessages, LocalMessages, Self::ProcessorUniType, Self::SenderChannelType, PROCESSOR_UNI_INSTRUMENTS>::new();
        socket_connection_handler.server_loop_for_unresponsive_text_protocol(listening_interface.to_string(),
                                                                             port,
                                                                             server_shutdown_receiver,
                                                                             connection_events_callback,
                                                                             dialog_processor_builder_fn).await
            .map_err(|err| Box::from(format!("Error starting SocketServer @ {listening_interface}:{port}: {:?}", err)))?;
        Ok(self)
    }
}


/// Upgrades the user provided `connection_events_callback` into a callback able to keep track of the shutdown event
/// -- so the "shutdown is complete" signal may be sent
fn upgrade_to_shutdown_tracking<SenderChannelType:              FullDuplexUniChannel + Sync + Send + 'static,
                                ConnectionEventsCallbackFuture: Future<Output=()> + Send>

                               (shutdown_is_complete_signaler:            tokio::sync::oneshot::Sender<()>,
                                user_provided_connection_events_callback: impl Fn(ConnectionEvent<SenderChannelType>) -> ConnectionEventsCallbackFuture + Send + Sync + 'static)

                               -> impl Fn(ConnectionEvent<SenderChannelType>) -> BoxFuture<'static, ()> + Send + Sync + 'static {

    let shutdown_is_complete_signaler = Arc::new(Mutex::new(Option::Some(shutdown_is_complete_signaler)));
    let user_provided_connection_events_callback = Arc::new(user_provided_connection_events_callback);
    move |connection_event | {
        let shutdown_is_complete_signaler = Arc::clone(&shutdown_is_complete_signaler);
        let user_provided_connection_events_callback = Arc::clone(&user_provided_connection_events_callback);
        Box::pin(async move {
            if let ConnectionEvent::ApplicationShutdown { timeout_ms } = connection_event {
                let _ = tokio::time::timeout(Duration::from_millis(timeout_ms as u64), user_provided_connection_events_callback(connection_event)).await;
                let Some(shutdown_is_complete_signaler) = shutdown_is_complete_signaler.lock().await.take()
                else {
                    warn!("Socket Server: a shutdown was asked, but a previous shutdown seems to have already taken place. There is a bug in your shutdown logic. Ignoring the current shutdown request...");
                    return
                };
                if let Err(_sent_value) = shutdown_is_complete_signaler.send(()) {
                    error!("Socket Server BUG: couldn't send shutdown signal to the local `one_shot` channel. Program is, likely, hanged. Please, investigate and fix!");
                }
            } else {
                user_provided_connection_events_callback(connection_event).await;
            }
        })
    }
}


/// Controls a [SocketServer] -- shutting down, asking for stats, ....
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
impl<const CONFIG:                         usize,
     const PROCESSOR_UNI_INSTRUMENTS:      usize,
     const PROCESSOR_BUFFER_SIZE_PER_PEER: usize,
     RemoteMessages,
     LocalMessages,
     ProcessorUniType: GenericUni<PROCESSOR_UNI_INSTRUMENTS>,
     SenderChannelType: ChannelCommon<'static, LocalMessages, LocalMessages> + ChannelProducer<'static, LocalMessages, LocalMessages>>
SocketServerController for
CustomResponsiveSocketServer<CONFIG, PROCESSOR_UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE_PER_PEER, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannelType> {

    fn shutdown_waiter(&mut self) -> Box<dyn FnOnce() -> BoxFuture<'static, Result<(), Box<dyn std::error::Error + Send + Sync>>> > {
        let mut local_shutdown_receiver = self.local_shutdown_receiver.take();
        Box::new(move || Box::pin({
            async move {
                if let Some(local_shutdown_receiver) = local_shutdown_receiver.take() {
                    match local_shutdown_receiver.await {
                        Ok(()) => {
                            Ok(())
                        },
                        Err(err) => Err(Box::from(format!("SocketServer::wait_for_shutdown(): It is no longer possible to tell when the server will be shutdown: `one_shot` signal error: {err}")))
                    }
                } else {
                    Err(Box::from("SocketServer: \"wait for shutdown\" requested, but the service was not started (or a previous shutdown was commanded) at the moment `shutdown_waiter()` was called"))
                }
            }
        }))
    }

    fn shutdown(mut self, timeout_ms: u32) -> Result<(), Box<dyn std::error::Error>> {
        match self.server_shutdown_signaler.take() {
            Some(server_sender) => {
                warn!("Socket Server: Shutdown asked & initiated for server @ {}:{} -- timeout: {timeout_ms}ms", self.interface_ip, self.port);
                if let Err(_sent_value) = server_sender.send(timeout_ms) {
                    Err(Box::from("Socket Server BUG: couldn't send shutdown signal to the network loop. Program is, likely, hanged. Please, investigate and fix!"))
                } else {
                    Ok(())
                }
            }
            None => {
                Err(Box::from("Socket Server: Shutdown requested, but the service was not started. Ignoring..."))
            }
        }
    }

}

/// Helps to infer some types used by the [SocketServer] -- also helping to simplify Generic Programming when used as a Generic Parameter:
/// ```nocompile
///     type THE_TYPE_I_WANT = <SocketServer<...> as GenericSocketServer>::THE_TYPE_YOU_WANT
pub trait SocketServerAssociatedTypes<const PROCESSOR_UNI_INSTRUMENTS: usize,
                            const PROCESSOR_BUFFER_SIZE_PER_PEER: usize> {
    const PROCESSOR_UNI_INSTRUMENTS: usize;
    const CONFIG: usize;
    type RemoteMessages;
    type LocalMessages;
    type ProcessorUniType: GenericUni<PROCESSOR_UNI_INSTRUMENTS>;
    type SenderChannelType;
    type ConnectionEventType;
    type StreamItemType;
    type StreamType;
}
impl<const CONFIG:                         usize,
     const PROCESSOR_UNI_INSTRUMENTS:      usize,
     const PROCESSOR_BUFFER_SIZE_PER_PEER: usize,
     RemoteMessages,
     LocalMessages,
     ProcessorUniType: GenericUni<PROCESSOR_UNI_INSTRUMENTS>,
     SenderChannelType: ChannelCommon<'static, LocalMessages, LocalMessages> + ChannelProducer<'static, LocalMessages, LocalMessages>>
SocketServerAssociatedTypes<PROCESSOR_UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE_PER_PEER> for
CustomResponsiveSocketServer<CONFIG, PROCESSOR_UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE_PER_PEER, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannelType> {
    const PROCESSOR_UNI_INSTRUMENTS: usize = PROCESSOR_UNI_INSTRUMENTS;
    const CONFIG: usize = CONFIG;
    type RemoteMessages      = RemoteMessages;
    type LocalMessages       = LocalMessages;
    type ProcessorUniType    = ProcessorUniType;
    type SenderChannelType   = SenderChannelType;
    type ConnectionEventType = ConnectionEvent<SenderChannelType>;
    type StreamItemType      = ProcessorUniType::DerivedItemType;
    type StreamType          = MessagingMutinyStream<PROCESSOR_UNI_INSTRUMENTS,
                                                     ProcessorUniType>;
}


/// Unit tests the [socket_server](self) module
#[cfg(any(test,doc))]
mod tests {
    use super::*;
    use crate::{
        SocketClient,
        ron_serializer,
        ron_deserializer,
    };
    use std::{
        sync::atomic::{
        AtomicU32,
        Ordering::Relaxed,
        },
        time::Duration,
    };
    use std::num::NonZeroU8;
    use futures::StreamExt;
    use reactive_mutiny::prelude::Uni;
    use reactive_mutiny::types::{ChannelProducer, ChannelCommon};
    use serde::{Serialize, Deserialize};
    use tokio::sync::Mutex;
    use crate::config::{RetryingStrategies, SocketOptions};


    const LOCALHOST: &str = "127.0.0.1";
    const PROCESSOR_UNI_INSTRUMENTS:      Instruments = Instruments::LogsWithExpensiveMetrics;
    const PROCESSOR_BUFFER_SIZE_PER_PEER: usize       = 1024;
    const SENDER_BUFFER_SIZE_PER_PEER:    usize       = 1024;
    const CONFIG:                         ConstConfig = ConstConfig {
        msg_size_hint:                 1024,
        graceful_close_timeout_millis: 256,
        retrying_strategy:             RetryingStrategies::DoNotRetry,
        socket_options:                SocketOptions {
            hops_to_live: NonZeroU8::new(30),
            linger_millis: Some(128),
            no_delay: Some(true),
        },
    };
    type ProcessorUniType<RemoteMessages> = UniZeroCopyAtomic<RemoteMessages, PROCESSOR_BUFFER_SIZE_PER_PEER, {PROCESSOR_UNI_INSTRUMENTS.into()}>;
    type SenderChannelType<LocalMessages> = ChannelUniMoveAtomic<LocalMessages, SENDER_BUFFER_SIZE_PER_PEER, 1>;
    type TestServer<RemoteMessages,
                    LocalMessages>
        = CustomResponsiveSocketServer<{CONFIG.into()},
                             {PROCESSOR_UNI_INSTRUMENTS.into()},
                             PROCESSOR_BUFFER_SIZE_PER_PEER,
                             RemoteMessages,
                             LocalMessages,
                             ProcessorUniType<RemoteMessages>,
                             SenderChannelType<LocalMessages>>;

    #[ctor::ctor]
    fn suite_setup() {
        simple_logger::SimpleLogger::new().with_utc_timestamps().init().unwrap_or_else(|_| eprintln!("--> LOGGER WAS ALREADY STARTED"));
    }

    /// assures the shutdown process is able to:
    ///   1) communicate with all clients
    ///   2) wait for up to the given timeout for them to gracefully disconnect
    ///   3) forcibly disconnect, if needed
    ///   4) notify any waiter on the server (after all the above steps are done) within the given timeout
    #[cfg_attr(not(doc),tokio::test(flavor = "multi_thread"))]
    async fn shutdown_process() {
        const PORT: u16 = 8070;

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
        type ServerType = TestServer::<DummyResponsiveClientAndServerMessages, DummyResponsiveClientAndServerMessages>;
        let mut server = ServerType::new(LOCALHOST, PORT);
        server.spawn_responsive_processor(
            move |connection_event: ServerType::ConnectionEventType| {
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
                            let _ = client_peer.sender.try_send_movable(DummyResponsiveClientAndServerMessages::FloodPing);
                            client_peer.sender.gracefully_end_all_streams(Duration::from_millis(timeout_ms as u64)).await;
                            // guarantees this operation will take slightly more than the timeout+tolerance to complete
                            tokio::time::sleep(Duration::from_millis((timeout_ms+tollerance_ms+10) as u64)).await;
                        }
                    }
                }
            },
            move |_, _, _, client_messages: ServerType::StreamType| {
                let server_received_messages_count = Arc::clone(&server_received_messages_count_ref1);
                client_messages.map(move |client_message| {
                    std::mem::forget(client_message);   // TODO 2023-07-15: investigate this reactive-mutiny related bug: it seems OgreUnique doesn't like the fact that this type doesn't need dropping? (no internal strings)... or is it a reactive-messaging bug?
                    server_received_messages_count.fetch_add(1, Relaxed);
                    DummyResponsiveClientAndServerMessages::FloodPing
                })
            }
        ).await.expect("Starting the server");

        // start a client that will engage in a flood ping with the server when provoked (never closing the connection)
        let _client = SocketClient::spawn_responsive_processor(
            LOCALHOST,
            PORT,
            |_: ConnectionEvent<DefaultUniChannel<DummyResponsiveClientAndServerMessages>>| async {},
            move |_, _, _, server_messages: MessagingMutinyStream<2048, DummyResponsiveClientAndServerMessages>| {
                let client_received_messages_count = Arc::clone(&client_received_messages_count_ref1);
                server_messages.map(move |server_message| {
                    std::mem::forget(server_message);   // TODO 2023-07-15: investigate this reactive-mutiny related bug: it seems OgreUnique doesn't like the fact that this type doesn't need dropping? (no internal strings)... or is it a reactive-messaging bug?
                    client_received_messages_count.fetch_add(1, Relaxed);
                    DummyResponsiveClientAndServerMessages::FloodPing
                })
            }
        ).await.expect("Starting the client");

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
        assert!(client_received_messages_count_ref2.load(Relaxed) > 1, "The client didn't receive any messages (no 'server is shutting down' notification)");
        assert!(server_received_messages_count_ref2.load(Relaxed) > 1, "The server didn't receive any messages (no 'gracefully disconnecting' after being notified that the server is shutting down)");
        assert!(elapsed_ms.abs_diff(expected_max_shutdown_duration_ms as u128) < tollerance_ms as u128,
                "The server shutdown (of a never compling client) didn't complete in a reasonable time, meaning the shutdown code is wrong. Timeout: {}ms; Tollerance: {}ms; Measured Time: {}ms",
                expected_max_shutdown_duration_ms, tollerance_ms, elapsed_ms);
    }

    /// The pretended messages produced by the server's "Unresponsive Processor"
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    enum DummyResponsiveClientAndServerMessages {
        FloodPing,
    }
    impl ReactiveMessagingSerializer<DummyResponsiveClientAndServerMessages> for DummyResponsiveClientAndServerMessages {
        #[inline(always)]
        fn serialize(remote_message: &DummyResponsiveClientAndServerMessages, buffer: &mut Vec<u8>) {
            ron_serializer(remote_message, buffer)
                .expect("socket_server.rs unit tests: No errors should have happened here!")
        }
        #[inline(always)]
        fn processor_error_message(err: String) -> DummyResponsiveClientAndServerMessages {
            panic!("socket_server.rs unit tests: protocol error when none should have happened: {err}");
        }
    }
    impl ResponsiveMessages<DummyResponsiveClientAndServerMessages> for DummyResponsiveClientAndServerMessages {
        #[inline(always)]
        fn is_disconnect_message(_processor_answer: &DummyResponsiveClientAndServerMessages) -> bool {
            false
        }
        #[inline(always)]
        fn is_no_answer_message(_processor_answer: &DummyResponsiveClientAndServerMessages) -> bool {
            false
        }
    }
    impl ReactiveMessagingDeserializer<DummyResponsiveClientAndServerMessages> for DummyResponsiveClientAndServerMessages {
        #[inline(always)]
        fn deserialize(local_message: &[u8]) -> Result<DummyResponsiveClientAndServerMessages, Box<dyn std::error::Error + Sync + Send>> {
            ron_deserializer(local_message)
        }
    }
}