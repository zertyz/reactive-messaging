//! Server-side handler the `reactive-sockets`'s connection


use super::{
    types::*,
    socket_connection_handler::{self, Peer},
    serde::{SocketServerSerializer,SocketServerDeserializer}
};
use std::{
    sync::Arc,
    net::{ToSocketAddrs,SocketAddr},
    fmt::Debug,
    future::Future,
    marker::PhantomData,
};
use std::fmt::Formatter;
use reactive_mutiny::prelude::{ChannelCommon, ChannelProducer, Uni};
use futures::{Stream, stream, StreamExt, future::BoxFuture};
use log::{trace, debug, info, warn, error};
use crate::socket_connection_handler::ProcessorRemoteStreamType;


/// The internal events a server shares with the user code.\
/// The user code may use those events to maintain a list of connected clients, be notified about shutdowns, init/deinit sessions, etc.
/// Note that the `Peer` objects received in those events may be used, at any time, to send messages to the clients -- like "Shutting down. Goodbye".
/// *When doing this on other occasions, make sure you won't break your own protocol.*
#[derive(Debug)]
pub enum ConnectionEvent<LocalPeerMessages:  'static + Send + Sync + PartialEq + Debug + SocketServerSerializer<LocalPeerMessages>> {
    PeerConnected {peer: Arc<Peer<LocalPeerMessages>>},
    PeerDisconnected {peer: Arc<Peer<LocalPeerMessages>>},
    ApplicationShutdown {timeout_ms: u32},
}


pub type SenderUniType<RemotePeerMessages> = reactive_mutiny::prelude::advanced::UniZeroCopyAtomic<RemotePeerMessages, 1024>;

/// The handle to define, start and shutdown a Socket Server
pub struct SocketServer {
    interface_ip:                String,
    port:                        u16,
    shutdown_signaler:           Option<tokio::sync::oneshot::Sender<u32>>,
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
            shutdown_signaler: None,
        }
    }

    fn sender_stream() {
        // create the uni
        // let server_messages_stream = self.processor_builder_fn(uni.stream)
        // return Self::to_client_stream(client_socket: Peer, server_messages_stream)
    }

    /// Returns a runner, which you may call to run `Server` and that will only return when
    /// the service is over.\
    /// When the returned closure is called, it will run the server by applying the `Stream`s returned by
    /// `dialog_processor_builder_fn()` for each connected client, where the output of the generated `Stream`s
    /// will be sent back to the clients.\
    /// Example:
    /// ```no_compile
    ///     self.runner()().await;
    pub async fn responsive_runner<RemotePeerMessages:             SocketServerDeserializer<RemotePeerMessages> + Send + Sync + PartialEq + Debug + 'static,
                                   LocalPeerMessages:              SocketServerSerializer<LocalPeerMessages>    + Send + Sync + PartialEq + Debug + 'static,
                                   LocalStreamType:                Stream<Item=LocalPeerMessages>               + Send + 'static,
                                   ConnectionEventsCallbackFuture: Future<Output=()> + Send,
                                   ConnectionEventsCallback:       Fn(/*server_event: */ConnectionEvent<LocalPeerMessages>) -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                   ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<LocalPeerMessages>>, /*client_messages_stream: */ProcessorRemoteStreamType<RemotePeerMessages>) -> LocalStreamType + Send + Sync + 'static>

                                  (&mut self,
                                   server_events_callback: ConnectionEventsCallback,
                                   dialog_processor_builder_fn: ProcessorBuilderFn)

                                   -> Result<impl FnOnce() -> BoxFuture<'static, Result<(),
                                                                                       Box<dyn std::error::Error + Sync + Send>>> + Sync + Send + 'static,
                                            Box<dyn std::error::Error + Sync + Send>> {

        let (shutdown_sender, shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        self.shutdown_signaler = Some(shutdown_sender);
        let listening_interface = self.interface_ip.to_string();
        let port = self.port;

        let runner = move || -> BoxFuture<'_, Result<(), Box<dyn std::error::Error + Send + Sync>>> {
            Box::pin(async move {
                tokio::spawn(async move {
                    let result = socket_connection_handler::server_loop_for_responsive_text_protocol(listening_interface.to_string(),
                                                                                                                         port,
                                                                                                                         shutdown_receiver,
                                                                                                                         server_events_callback,
                                                                                                                         dialog_processor_builder_fn).await;
                    if let Err(err) = result {
                        error!("Error starting SocketServer @ {listening_interface}:{port}: {:?}", err);
                    }
                }).await?;

                Ok(())
            })
        };

        Ok(runner)
    }

    pub fn shutdown(&mut self) {
        const TIMEOUT_MILLIS: u32 = 5000;
        match self.shutdown_signaler.take() {
            Some(sender) => {
                warn!("Socket Server: Shutdown asked & initiated -- timeout: {}ms", TIMEOUT_MILLIS);
                if let Err(err) = sender.send(TIMEOUT_MILLIS) {
                    error!("Socket Server BUG: couldn't send shutdown signal to the network loop. Program is, likely, hanged. Please, investigate and fix. Error: {}", err);
                }
            }
            None => {
                error!("Socket Server: Shutdown requested, but the service was not started (or a previous shutdown is underway). Ignoring...");
            }
        }
    }

}