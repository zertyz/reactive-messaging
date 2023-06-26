//! Server-side handler the `reactive-sockets`'s connection


use super::{
    config::*,
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
use std::sync::atomic::AtomicI32;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;
use reactive_mutiny::prelude::{ChannelCommon, ChannelProducer, Uni};
use futures::{Stream, stream, StreamExt, future::BoxFuture};
use log::{trace, debug, info, warn, error};
use crate::prelude::ProcessorRemoteStreamType;


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
    ///     self.responsive_starter()().await;
    pub fn responsive_starter<RemotePeerMessages:             SocketServerDeserializer<RemotePeerMessages> + Send + Sync + PartialEq + Debug + 'static,
                              LocalPeerMessages:              SocketServerSerializer<LocalPeerMessages>    + Send + Sync + PartialEq + Debug + 'static,
                              LocalStreamType:                Stream<Item=LocalPeerMessages>               + Send + 'static,
                              ConnectionEventsCallbackFuture: Future<Output=()> + Send,
                              ConnectionEventsCallback:       Fn(/*server_event: */ConnectionEvent<LocalPeerMessages>) -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                              ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<LocalPeerMessages>>, /*client_messages_stream: */ProcessorRemoteStreamType<RemotePeerMessages>) -> LocalStreamType + Send + Sync + 'static>

                             (&mut self,
                              server_events_callback: ConnectionEventsCallback,
                              dialog_processor_builder_fn: ProcessorBuilderFn)

                             -> impl FnOnce() -> BoxFuture<'static, Result<(), Box<dyn std::error::Error + Sync + Send + 'static>>> {

        let (server_shutdown_sender, server_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (local_shutdown_sender, local_shutdown_receiver) = tokio::sync::oneshot::channel::<()>();
        self.server_shutdown_signaler = Some(server_shutdown_sender);
        self.local_shutdown_signaler = Some(local_shutdown_sender);
        self.local_shutdown_receiver = Some(local_shutdown_receiver);
        let listening_interface = self.interface_ip.to_string();
        let port = self.port;

        let runner = move || -> BoxFuture<'_, Result<(), Box<dyn std::error::Error + Sync + Send>>> {
            Box::pin(async move {
                let started_state = Arc::new(AtomicI32::new(0));     // 0: not started; 1: started; -1: error starting
                let join_handler = tokio::spawn({
                    let started_state = Arc::clone(&started_state);
                    let listening_interface = listening_interface.clone();
                    async move {
                        started_state.store(1, Relaxed);
                        let result = socket_connection_handler::server_loop_for_responsive_text_protocol(listening_interface.to_string(),
                                                                                                         port,
                                                                                                         server_shutdown_receiver,
                                                                                                         server_events_callback,
                                                                                                         dialog_processor_builder_fn).await;
                        if let Err(err) = result {
                            started_state.store(-1, Relaxed);
                            let msg = format!("Error starting SocketServer @ {listening_interface}:{port}: {:?}", err);
                            error!("{}", msg);
                            Err(msg)
                        } else {
                            Ok(())
                        }
                    }
                });

                // wait for up to 1 second to have any server starting errors to propagate to the caller
                let mut i = 1000;
                loop {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                    match started_state.load(Relaxed) {
                        0 => break Err(Box::from(join_handler.await.unwrap_err().to_string())),
                        1 => break Ok(()),
                        _ => (),
                    }
                    i -= 1;
                    if i == 0 {
                        break Err(Box::from(format!("SocketServer @ {listening_interface}:{port} didn't start within 1 second")))
                    }
                }
            })
        };

        runner
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

    pub fn shutdown(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        const TIMEOUT_MILLIS: u32 = 5000;
        match self.server_shutdown_signaler.take().zip(self.local_shutdown_signaler.take()) {
            Some((server_sender, local_sender)) => {
                warn!("Socket Server: Shutdown asked & initiated for server @ {}:{} -- timeout: {TIMEOUT_MILLIS}ms", self.interface_ip, self.port);
                if let Err(_err) = server_sender.send(TIMEOUT_MILLIS) {
                    Err(Box::from(format!("Socket Server BUG: couldn't send shutdown signal to the network loop. Program is, likely, hanged. Please, investigate and fix")))
                } else if let Err(_err) = local_sender.send(()) {
                    Err(Box::from(format!("Socket Server BUG: couldn't send shutdown signal to the local `one_shut` channel. Program is, likely, hanged. Please, investigate and fix")))
                } else {
                    Ok(())
                }
            }
            None => {
                Err(Box::from(format!("Socket Server: Shutdown requested, but the service was not started (or a previous shutdown is underway). Ignoring...")))
            }
        }
    }

}