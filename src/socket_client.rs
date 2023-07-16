//! Resting place for [SocketClient]


use crate::{
    types::ConnectionEvent,
    prelude::ProcessorRemoteStreamType,
    socket_connection_handler::{self, Peer},
    ReactiveMessagingDeserializer,
    ResponsiveMessages, ReactiveMessagingSerializer,
};
use std::{
    fmt::Debug,
    future::Future,
    sync::{
        Arc, 
        atomic::AtomicBool,
        atomic::Ordering::Relaxed,
    },
};
use futures::Stream;
use tokio::sync::oneshot::Sender;
use log::warn;


/// The handle to define, start and shutdown a Reactive Client for Socket Connections
#[derive(Debug)]
pub struct SocketClient {
    /// false if a disconnection happened, as tracked by the socket logic
    connected: Arc<AtomicBool>,
    /// the server ip of the connection
    ip:        String,
    /// the server port of the connection 
    port:      u16,
    /// Signaler to shutdown the client
    processor_shutdown_signaler: Sender<u32>,
}

impl SocketClient {

    /// Spawns a task to connect to the server @ `ip` & `port` and returns, immediately,
    /// an object through which the caller may inquire some stats (if opted in) and request
    /// the client to disconnect.\
    /// The given `dialog_processor` will produce non-futures & non-fallibles `ClientMessages` that will be sent to the server.
    #[must_use = "the client won't do a thing if its value isn't hold until the disconnection time"]
    pub async fn spawn_responsive_processor<ServerMessages:                 ReactiveMessagingDeserializer<ServerMessages> + Send + Sync + PartialEq + Debug + 'static,
                                            ClientMessages:                 ReactiveMessagingSerializer<ClientMessages>   +
                                                                            ResponsiveMessages<ClientMessages>            + Send + Sync + PartialEq + Debug + 'static,
                                            ConnectionEventsCallbackFuture: Future<Output=()>                             + Send,
                                            ClientStreamType:               Stream<Item=ClientMessages>                   + Send + 'static,
                                            IntoString:                     Into<String>>
                                           (ip:                         IntoString,
                                            port:                       u16,
                                            connection_events_callback: impl Fn(ConnectionEvent<ClientMessages>) -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                            processor_stream_builder:   impl Fn(/*server_addr: */String, /*port: */u16, /*peer: */Arc<Peer<ClientMessages>>, /*remote_messages_stream: */ProcessorRemoteStreamType<ServerMessages>) -> ClientStreamType + Send + Sync + 'static)
                                           -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        let ip = ip.into();
        let (processor_shutdown_sender, processor_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let connected_state = Arc::new(AtomicBool::new(false));
        let connection_events_callback = upgrade_to_connected_state_tracking(&connected_state, connection_events_callback);
        socket_connection_handler::client_for_responsive_text_protocol(ip.clone(), port, processor_shutdown_receiver, connection_events_callback, processor_stream_builder).await?;
        let socket_client = Self { connected: connected_state, ip, port, processor_shutdown_signaler: processor_shutdown_sender };
        Ok(socket_client)
    }

    /// Spawns a task to connect to the server @ `ip` & `port` and returns, immediately,
    /// an object through which the caller may inquire some stats (if opted in) and request
    /// the client to disconnect.\
    /// The given `dialog_processor` will produce non-futures & non-fallibles items that won't be sent to the server
    /// -- if you want the processor to produce "answer messages" to the server, see [SocketClient::spawn_responsive_processor()].
    #[must_use = "the client won't do a thing if its value isn't hold until the disconnection time"]
    pub async fn spawn_unresponsive_processor<ServerMessages:                 ReactiveMessagingDeserializer<ServerMessages> + Send + Sync + PartialEq + Debug + 'static,
                                              ClientMessages:                 ReactiveMessagingSerializer<ClientMessages>   + Send + Sync + PartialEq + Debug + 'static,
                                              OutputStreamItemsType:                                                          Send + Sync             + Debug + 'static,
                                              OutputStreamType:               Stream<Item=OutputStreamItemsType>            + Send                            + 'static,
                                              ConnectionEventsCallbackFuture: Future<Output=()>                             + Send,
                                              IntoString:                     Into<String>>
                                             (ip:                         IntoString,
                                              port:                       u16,
                                              connection_events_callback: impl Fn(ConnectionEvent<ClientMessages>) -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                              processor_stream_builder:   impl Fn(/*server_addr: */String, /*port: */u16, /*peer: */Arc<Peer<ClientMessages>>, /*remote_messages_stream: */ProcessorRemoteStreamType<ServerMessages>) -> OutputStreamType + Send + Sync + 'static)
                                             -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        let ip = ip.into();
        let (processor_shutdown_sender, processor_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let connected_state = Arc::new(AtomicBool::new(false));
        let connection_events_callback = upgrade_to_connected_state_tracking(&connected_state, connection_events_callback);
        socket_connection_handler::client_for_unresponsive_text_protocol(ip.clone(), port, processor_shutdown_receiver, connection_events_callback, processor_stream_builder).await?;
        let socket_client = Self { connected: connected_state, ip, port, processor_shutdown_signaler: processor_shutdown_sender };
        Ok(socket_client)
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Relaxed)
    }

    pub fn shutdown(self, timeout_ms: u32) -> Result<(), Box<dyn std::error::Error>> {
        warn!("Socket Client: Shutdown asked & initiated for client connected @ {}:{} -- timeout: {timeout_ms}ms", self.ip, self.port);
        if let Err(_err) = self.processor_shutdown_signaler.send(timeout_ms) {
            Err(Box::from("Socket Client BUG: couldn't send shutdown signal to the network loop. Program is, likely, hanged. Please, investigate and fix!"))
        } else {
            Ok(())
        }
    }

}

/// Upgrades the user provided `connection_events_callback` into a callback able to keep track of disconnection events
fn upgrade_to_connected_state_tracking<ClientMessages:                 ReactiveMessagingSerializer<ClientMessages>   + Send + Sync + PartialEq + Debug + 'static,
                                       ConnectionEventsCallbackFuture: Future<Output=()>                        + Send>
                                      (connected_state:                          &Arc<AtomicBool>,
                                       user_provided_connection_events_callback: impl Fn(ConnectionEvent<ClientMessages>) -> ConnectionEventsCallbackFuture + Send + Sync + 'static)
                                      -> impl Fn(ConnectionEvent<ClientMessages>) -> ConnectionEventsCallbackFuture + Send + Sync + 'static {
    let connected_state = Arc::clone(connected_state);
    move |connection_event | {
        if let ConnectionEvent::PeerConnected { .. } = connection_event {
            connected_state.store(true, Relaxed);
        } else if let ConnectionEvent::PeerDisconnected {..} = connection_event {
            connected_state.store(false, Relaxed);
        }
        user_provided_connection_events_callback(connection_event)
    }
}