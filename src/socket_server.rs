//! Resting place for [SocketServer]


use crate::ReactiveMessagingSerializer;
use super::{
    ResponsiveMessages,
    types::*,
    prelude::ProcessorRemoteStreamType,
    socket_connection_handler::{self, Peer},
    serde::ReactiveMessagingDeserializer,
};
use std::{
    sync::Arc,
    fmt::Debug,
    future::Future, time::Duration,
};
use futures::{Stream, future::BoxFuture};
use log::{warn,error};
use tokio::sync::Mutex;


/// The handle to define, start and shutdown a Reactive Server for Socket Connections.\
/// `BUFFERED_MESSAGES_PER_PEER_COUNT` is the number of messages that may be produced ahead of sending (to each client)
///                                    as well as the number of messages that this server may accumulate from each client before denying new ones
#[derive(Debug)]
pub struct SocketServer<const BUFFERED_MESSAGES_PER_PEER_COUNT: usize> {
    interface_ip:                String,
    port:                        u16,
    /// Signaler to stop the server
    server_shutdown_signaler:    Option<tokio::sync::oneshot::Sender<u32>>,
    /// Signaler to cause [wait_for_shutdown()] to return
    local_shutdown_receiver:     Option<tokio::sync::oneshot::Receiver<()>>,
}

impl<const BUFFERED_MESSAGES_PER_PEER_COUNT: usize> SocketServer<BUFFERED_MESSAGES_PER_PEER_COUNT> {


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
        }
    }

    /// Spawns a task to run a Server listening @ `self`'s `interface_ip` & `port` and returns, immediately,
    /// an object through which the caller may inquire some stats (if opted in) and request the server to shutdown.\
    /// The given `dialog_processor_builder_fn` will be called for each new client and will return a `reactive-mutiny` Stream
    /// that will produce non-futures & non-fallibles `ServerMessages` that will be sent to the clients.
    pub async fn spawn_responsive_processor<ClientMessages:                         ReactiveMessagingDeserializer<ClientMessages> + Send + Sync + PartialEq + Debug + 'static,
                                            ServerMessages:                         ReactiveMessagingSerializer<ServerMessages>   +
                                                                                    ResponsiveMessages<ServerMessages>            + Send + Sync + PartialEq + Debug + 'static,
                                            ServerStreamType:                       Stream<Item=ServerMessages>                   + Send + 'static,
                                            ConnectionEventsCallbackFuture:         Future<Output=()> + Send,
                                            ConnectionEventsCallback:               Fn(/*server_event: */ConnectionEvent<BUFFERED_MESSAGES_PER_PEER_COUNT, ServerMessages>)                                                                                                                                              -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                            ProcessorBuilderFn:                     Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<BUFFERED_MESSAGES_PER_PEER_COUNT, ServerMessages>>, /*client_messages_stream: */ProcessorRemoteStreamType<BUFFERED_MESSAGES_PER_PEER_COUNT, ClientMessages>) -> ServerStreamType + Send + Sync + 'static>

                                           (&mut self,
                                            connection_events_callback:  ConnectionEventsCallback,
                                            dialog_processor_builder_fn: ProcessorBuilderFn)

                                           -> Result<(), Box<dyn std::error::Error + Sync + Send + 'static>> {

        let (server_shutdown_sender, server_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (local_shutdown_sender, local_shutdown_receiver) = tokio::sync::oneshot::channel::<()>();
        self.server_shutdown_signaler = Some(server_shutdown_sender);
        self.local_shutdown_receiver = Some(local_shutdown_receiver);
        let listening_interface = self.interface_ip.to_string();
        let port = self.port;

        let connection_events_callback = upgrade_to_shutdown_tracking(local_shutdown_sender, connection_events_callback);

        socket_connection_handler::server_loop_for_responsive_text_protocol::
            <BUFFERED_MESSAGES_PER_PEER_COUNT, _, _, _, _, _, _>
            (listening_interface.to_string(),
             port,
             server_shutdown_receiver,
             connection_events_callback,
             dialog_processor_builder_fn).await
            .map_err(|err| Box::from(format!("Error starting SocketServer @ {listening_interface}:{port}: {:?}", err)))
    }

    /// Spawns a task to run a Server listening @ `self`'s `interface_ip` & `port` and returns, immediately,
    /// an object through which the caller may inquire some stats (if opted in) and request the server to shutdown.\
    /// The given `dialog_processor_builder_fn` will be called for each new client and will return a `reactive-mutiny` Stream
    /// that will produce non-futures & non-fallibles items that won't be sent to the clients
    /// -- if you want the processor to produce "answer messages" to the clients, see [SocketServer::spawn_responsive_processor()].
    pub async fn spawn_unresponsive_processor<ClientMessages:                 ReactiveMessagingDeserializer<ClientMessages> + Send + Sync + PartialEq + Debug + 'static,
                                              ServerMessages:                 ReactiveMessagingSerializer<ServerMessages>   + Send + Sync + PartialEq + Debug + 'static,
                                              OutputStreamItemsType:                                                          Send + Sync             + Debug + 'static,
                                              ServerStreamType:               Stream<Item=OutputStreamItemsType>            + Send + 'static,
                                              ConnectionEventsCallbackFuture: Future<Output=()> + Send,
                                              ConnectionEventsCallback:       Fn(/*server_event: */ConnectionEvent<BUFFERED_MESSAGES_PER_PEER_COUNT, ServerMessages>)                                                                                                                                              -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                              ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<BUFFERED_MESSAGES_PER_PEER_COUNT, ServerMessages>>, /*client_messages_stream: */ProcessorRemoteStreamType<BUFFERED_MESSAGES_PER_PEER_COUNT, ClientMessages>) -> ServerStreamType + Send + Sync + 'static>

                                             (&mut self,
                                              connection_events_callback:  ConnectionEventsCallback,
                                              dialog_processor_builder_fn: ProcessorBuilderFn)

                                             -> Result<(), Box<dyn std::error::Error + Sync + Send + 'static>> {

        let (server_shutdown_sender, server_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (local_shutdown_sender, local_shutdown_receiver) = tokio::sync::oneshot::channel::<()>();
        self.server_shutdown_signaler = Some(server_shutdown_sender);
        self.local_shutdown_receiver = Some(local_shutdown_receiver);
        let listening_interface = self.interface_ip.to_string();
        let port = self.port;

        let connection_events_callback = upgrade_to_shutdown_tracking(local_shutdown_sender, connection_events_callback);

        socket_connection_handler::server_loop_for_unresponsive_text_protocol(listening_interface.to_string(),
                                                                              port,
                                                                              server_shutdown_receiver,
                                                                              connection_events_callback,
                                                                              dialog_processor_builder_fn).await
            .map_err(|err| Box::from(format!("Error starting SocketServer @ {listening_interface}:{port}: {:?}", err)))
    }

    /// Returns an async closure that blocks until [shutdown()] is called.
    /// Example:
    /// ```no_compile
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
                        Err(err) => Err(Box::from(format!("SocketServer::wait_for_shutdown(): It is no longer possible to tell when the server will be shutdown: `one_shot` signal error: {err}")))
                    }
                } else {
                    Err(Box::from("SocketServer: \"wait for shutdown\" requested, but the service was not started (or a previous shutdown was commanded) at the moment `shutdown_waiter()` was called"))
                }
            }
        })
    }

    /// Notifies the server it is time to shutdown.\
    /// The provided `connection_events` callback will be executed for up to `timeout_ms` milliseconds
    /// -- in which case it is a good practice to inform all the clients that a server-initiated disconnection (due to a shutdown) is happening.
    pub fn shutdown(mut self, timeout_ms: u32) -> Result<(), Box<dyn std::error::Error>> {
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

/// Upgrades the user provided `connection_events_callback` into a callback able to keep track of the shutdown event
/// -- so the "shutdown is complete" signal may be sent
fn upgrade_to_shutdown_tracking<const BUFFERED_MESSAGES_PER_PEER_COUNT: usize,
                                ServerMessages:                         ReactiveMessagingSerializer<ServerMessages>   + Send + Sync + PartialEq + Debug + 'static,
                                ConnectionEventsCallbackFuture:         Future<Output=()>                             + Send>

                               (shutdown_is_complete_signaler:            tokio::sync::oneshot::Sender<()>,
                                user_provided_connection_events_callback: impl Fn(ConnectionEvent<BUFFERED_MESSAGES_PER_PEER_COUNT, ServerMessages>) -> ConnectionEventsCallbackFuture + Send + Sync + 'static)

                               -> impl Fn(ConnectionEvent<BUFFERED_MESSAGES_PER_PEER_COUNT, ServerMessages>) -> BoxFuture<'static, ()> + Send + Sync + 'static {

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
    use futures::StreamExt;
    use reactive_mutiny::types::{ChannelProducer, ChannelCommon};
    use serde::{Serialize, Deserialize};
    use tokio::sync::Mutex;


    const LOCALHOST: &str = "127.0.0.1";

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
        let mut server = SocketServer::<2048>::new(LOCALHOST, PORT);
        server.spawn_responsive_processor(
            move |connection_event: ConnectionEvent<2048, DummyResponsiveClientAndServerMessages>| {
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
            move |_, _, _, client_messages: ProcessorRemoteStreamType<2048, DummyResponsiveClientAndServerMessages>| {
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
            |_: ConnectionEvent<2048, DummyResponsiveClientAndServerMessages>| async {},
            move |_, _, _, server_messages: ProcessorRemoteStreamType<2048, DummyResponsiveClientAndServerMessages>| {
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