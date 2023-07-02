//! Reactive client for socket connections


use crate::{
    types::ConnectionEvent,
    prelude::ProcessorRemoteStreamType,
    socket_connection_handler::{self, Peer},
    SocketServerDeserializer,
    SocketServerSerializer,
};
use std::{
    fmt::Debug,
    future::Future,
    sync::Arc,
};
use futures::Stream;
use tokio::sync::oneshot::Sender;
use log::warn;


/// The handle to define, start and shutdown a Socket Client
pub struct SocketClient {
    ip: String,
    port: u16,
    /// Signaler to stop the client
    processor_shutdown_signaler: Sender<u32>,
}

impl SocketClient {

    /// Spawns a task to connect to the server @ `ip` & `port` and returns, immediately,
    /// an object through which the caller may inquire some stats (if opted in) and request
    /// the client to disconnect.\
    /// The given `dialog_processor` will produce non-futures & non-fallibles `ServerMessages` that will be sent to the server.
    pub async fn spawn_responsive_processor<ServerMessages:                 SocketServerDeserializer<ServerMessages> + Send + Sync + PartialEq + Debug + 'static,
                                            ClientMessages:                 SocketServerSerializer<ClientMessages>   + Send + Sync + PartialEq + Debug + 'static,
                                            ConnectionEventsCallbackFuture: Future<Output=()>                        + Send,
                                            ClientStreamType:               Stream<Item=ClientMessages>              + Send + 'static>
                                           (ip:                         String,
                                            port:                       u16,
                                            connection_events_callback: impl Fn(ConnectionEvent<ClientMessages>) -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                            processor_stream_builder:   impl Fn(/*server_addr: */String, /*port: */u16, /*peer: */Arc<Peer<ClientMessages>>, /*remote_messages_stream: */ProcessorRemoteStreamType<ServerMessages>) -> ClientStreamType + Send + Sync + 'static)
                                           -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        let (processor_shutdown_sender, processor_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let socket_client = Self { ip: ip.clone(), port, processor_shutdown_signaler: processor_shutdown_sender };
        socket_connection_handler::client_for_responsive_text_protocol(ip, port, processor_shutdown_receiver, connection_events_callback, processor_stream_builder).await?;
        Ok(socket_client)
    }

    pub fn shutdown(self) -> Result<(), Box<dyn std::error::Error>> {
        const TIMEOUT_MILLIS: u32 = 5000;
        warn!("Socket Client: Shutdown asked & initiated for client connected @ {}:{} -- timeout: {TIMEOUT_MILLIS}ms", self.ip, self.port);
        if let Err(_err) = self.processor_shutdown_signaler.send(TIMEOUT_MILLIS) {
            Err(Box::from(format!("Socket Client BUG: couldn't send shutdown signal to the network loop. Program is, likely, hanged. Please, investigate and fix")))
        } else {
            Ok(())
        }
    }

}