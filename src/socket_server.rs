//! Resting place for [SocketServer]


use super::{
    types::*,
    prelude::ProcessorRemoteStreamType,
    socket_connection_handler::{self, Peer},
    serde::{SocketServerSerializer,SocketServerDeserializer}
};
use std::{
    sync::Arc,
    fmt::Debug,
    future::Future,
};
use futures::{Stream, future::BoxFuture};
use log::warn;


pub type SenderUniType<RemotePeerMessages> = reactive_mutiny::prelude::advanced::UniZeroCopyAtomic<RemotePeerMessages, 1024>;

/// The handle to define, start and shutdown a Reactive Server for Socket Connections
#[derive(Debug)]
pub struct SocketServer {
    interface_ip:                String,
    port:                        u16,
    /// Signaler to stop the server
    server_shutdown_signaler:    Option<tokio::sync::oneshot::Sender<u32>>,
    /// Signaler to cause [wait_for_shutdown()] to return
    local_shutdown_signaler:     Option<tokio::sync::oneshot::Sender<()>>,
    local_shutdown_receiver:     Option<tokio::sync::oneshot::Receiver<()>>,
}

impl SocketServer {

    /// Creates a new server instance listening on TCP/IP:
    ///   `interface_ip`:         the interface's IP to listen to -- 0.0.0.0 will cause listening to all network interfaces
    ///   `port`:                 what port to listen to
    ///   `processor_builder_fn`: a function to instantiate a new processor `Stream` whenever a new connection arrives
    pub fn new(interface_ip:                String,
               port:                        u16)
               -> Self {
        Self {
            interface_ip,
            port,
            server_shutdown_signaler: None,
            local_shutdown_signaler: None,
            local_shutdown_receiver: None,
        }
    }

    /// Spawns a task to run the Server listening @ `self`'s `interface_ip` & `port` and returns, immediately,
    /// an object through which the caller may inquire some stats (if opted in) and request the server to shutdown.\
    /// The given `dialog_processor_builder_fn` will be called for each new client and will return a `reactive-mutiny` Stream
    /// that will produce non-futures & non-fallibles `ServerMessages` that will be sent to the clients.
    pub async fn spawn_responsive_processor<RemotePeerMessages:             SocketServerDeserializer<RemotePeerMessages> + Send + Sync + PartialEq + Debug + 'static,
                                            LocalPeerMessages:              SocketServerSerializer<LocalPeerMessages>    + Send + Sync + PartialEq + Debug + 'static,
                                            LocalStreamType:                Stream<Item=LocalPeerMessages>               + Send + 'static,
                                            ConnectionEventsCallbackFuture: Future<Output=()> + Send,
                                            ConnectionEventsCallback:       Fn(/*server_event: */ConnectionEvent<LocalPeerMessages>) -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                            ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<LocalPeerMessages>>, /*client_messages_stream: */ProcessorRemoteStreamType<RemotePeerMessages>) -> LocalStreamType + Send + Sync + 'static>

                                           (&mut self,
                                            server_events_callback: ConnectionEventsCallback,
                                            dialog_processor_builder_fn: ProcessorBuilderFn)

                                           -> Result<(), Box<dyn std::error::Error + Sync + Send + 'static>> {

        let (server_shutdown_sender, server_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (local_shutdown_sender, local_shutdown_receiver) = tokio::sync::oneshot::channel::<()>();
        self.server_shutdown_signaler = Some(server_shutdown_sender);
        self.local_shutdown_signaler = Some(local_shutdown_sender);
        self.local_shutdown_receiver = Some(local_shutdown_receiver);
        let listening_interface = self.interface_ip.to_string();
        let port = self.port;

        socket_connection_handler::server_loop_for_responsive_text_protocol(listening_interface.to_string(),
                                                                            port,
                                                                            server_shutdown_receiver,
                                                                            server_events_callback,
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
                    Err(Box::from(format!("SocketServer: \"wait for shutdown\" requested, but the service was not started (or a previous shutdown was commanded) at the moment `shutdown_waiter()` was called")))
                }
            }
        })
    }

    pub fn shutdown(mut self) -> Result<(), Box<dyn std::error::Error>> {
        const TIMEOUT_MILLIS: u32 = 5000;
        match self.server_shutdown_signaler.take().zip(self.local_shutdown_signaler.take()) {
            Some((server_sender, local_sender)) => {
                warn!("Socket Server: Shutdown asked & initiated for server @ {}:{} -- timeout: {TIMEOUT_MILLIS}ms", self.interface_ip, self.port);
                if let Err(_err) = server_sender.send(TIMEOUT_MILLIS) {
                    Err(Box::from(format!("Socket Server BUG: couldn't send shutdown signal to the network loop. Program is, likely, hanged. Please, investigate and fix")))
                } else if let Err(_err) = local_sender.send(()) {
                    Err(Box::from(format!("Socket Server BUG: couldn't send shutdown signal to the local `one_shot` channel. Program is, likely, hanged. Please, investigate and fix")))
                } else {
                    Ok(())
                }
            }
            None => {
                Err(Box::from(format!("Socket Server: Shutdown requested, but the service was not started. Ignoring...")))
            }
        }
    }

}