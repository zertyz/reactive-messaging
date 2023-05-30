//! Server-side handler the `reactive-sockets`'s connection


use super::{
    types::*,
    connection::{self, Peer, SocketEvent},
    serde::{SocketServerSerializer,SocketServerDeserializer}
};
use std::{
    sync::Arc,
    net::{ToSocketAddrs,SocketAddr},
    fmt::Debug,
};
use futures::future::BoxFuture;
use futures::{Stream, stream, StreamExt};
use log::{trace, debug, info, warn, error};


/// The handle to define, start and shutdown a Socket Server
pub struct SocketServer<'a, RemotePeerMessages:           Send + SocketServerDeserializer<RemotePeerMessages>,
                            LocalPeerMessages:            Send + Debug + PartialEq + SocketServerSerializer<LocalPeerMessages, LocalPeerDisconnectionReason>,
                            LocalPeerDisconnectionReason: Send> {
    interface_ip:                      String,
    port:                              u16,
    workers:                           u16,
    shutdown_signaler:                 Option<tokio::sync::oneshot::Sender<u32>>,
    request_processor_stream_producer: Option<Box<dyn Fn(SocketEvent<RemotePeerMessages, LocalPeerMessages, LocalPeerDisconnectionReason>) -> bool + Send + Sync + 'a>>,
    request_processor_stream_closer:   Option<Box<dyn Fn() + Send + Sync + 'a>>,
}

impl<RemotePeerMessages:           'static + Send + SocketServerDeserializer<RemotePeerMessages>,
     LocalPeerMessages:            'static + Send + Sync + Debug + PartialEq + SocketServerSerializer<LocalPeerMessages, LocalPeerDisconnectionReason>,
     LocalPeerDisconnectionReason: 'static + Send + Sync>
SocketServer<'static, RemotePeerMessages, LocalPeerMessages, LocalPeerDisconnectionReason> {

    /// Creates a new server instance listening on TCP/IP:
    ///   `interface_ip`: the interface's IP to listen to -- 0.0.0.0 will cause listening to all network interfaces
    ///   `port`: what port to listen to
    ///   `workers`: How many tokio async tasks should be used to process the incoming requests?
    ///              If you delegate it to events (or similar), this should be 1;
    ///              If you fully process the request in the worker task (questionable practice), measure and pick your optimal number.
    pub fn new(interface_ip: String, port: u16, workers: u16) -> Self {
        Self {
            interface_ip,
            port,
            workers,
            shutdown_signaler:                 None,
            request_processor_stream_producer: None,
            request_processor_stream_closer:   None,
        }
    }

    /// Attaches a request processor to this Socket Server, comprising of:
    ///   - `request_processor_stream`: this is a stream yielding [ServerMessages] -- most likely mapping [ClientMessages] to it. See [processor::processor()] for an implementation
    ///   - `request_processor_stream_producer`: a `sync` function to feed in [ClientMessages] to the `request_stream_processor`
    ///   - `request_processor_stream_closer`: this closes the stream and is called when the server is shutdown
    pub fn set_processor(&mut self,
                         request_processor_stream:          impl Stream<Item = Result<(Arc<Peer<LocalPeerMessages, LocalPeerDisconnectionReason>>, LocalPeerMessages),
                                                                                      (Arc<Peer<LocalPeerMessages, LocalPeerDisconnectionReason>>, Box<dyn std::error::Error + Sync + Send>)>> + Send + Sync + 'static,
                         request_processor_stream_producer: impl Fn(SocketEvent<RemotePeerMessages, LocalPeerMessages, LocalPeerDisconnectionReason>) -> bool + Send + Sync + 'static,
                         request_processor_stream_closer:   impl Fn() + Send + Sync + 'static)
                        -> impl Stream<Item = (Arc<Peer<LocalPeerMessages, LocalPeerDisconnectionReason>>, bool)> + Send + Sync + 'static {
        self.request_processor_stream_producer = Some(Box::new(request_processor_stream_producer));
        self.request_processor_stream_closer   = Some(Box::new(request_processor_stream_closer));
        Self::to_sender_stream(request_processor_stream)
    }

    /// returns a runner, which you may call to run `Server` and that will only return when
    /// the service is over -- this special semantics allows holding the mutable reference to `self`
    /// as little as possible.\
    /// Example:
    /// ```no_compile
    ///     self.runner()().await;
    pub async fn runner(&mut self) -> Result<impl FnOnce() -> BoxFuture<'static, Result<(),
                                                                                        Box<dyn std::error::Error + Sync + Send>>> + Sync + Send + 'static,
                                             Box<dyn std::error::Error + Sync + Send>> {

        let request_processor_stream_producer = self.request_processor_stream_producer.take();
        let request_processor_stream_closer = self.request_processor_stream_closer.take();

        if request_processor_stream_producer.is_none() || request_processor_stream_closer.is_none() {
            return Err(Box::from(format!("Request processor fields are not present. Was `set_processor()` called? ... or was this server already executed?")));
        }

        let request_processor_stream_producer = request_processor_stream_producer.unwrap();
        let request_processor_stream_closer = request_processor_stream_closer.unwrap();

        let (shutdown_sender, shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();

        self.shutdown_signaler = Some(shutdown_sender);
        let listening_interface = self.interface_ip.to_string();
        let port = self.port;

        let runner = move || -> BoxFuture<'_, Result<(), Box<dyn std::error::Error + Send + Sync>>> {
            Box::pin(async move {
                tokio::spawn(async move {
                    //run(handler, listener.unwrap(), addr, request_processor_stream_producer, request_processor_stream_closer)
                    connection::server_network_loop_for_text_protocol(listening_interface,
                                                                      port,
                                                                      shutdown_receiver,
                                                                      move |network_event| {
                                                                          request_processor_stream_producer(network_event);
                                                                          async move {}
                                                                      }).await;
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

    /// upgrades the `request_processor_stream` to a `Stream` able to both process requests & send back answers to the clients
    fn to_sender_stream(request_processor_stream: impl Stream<Item = Result<(Arc<Peer<LocalPeerMessages, LocalPeerDisconnectionReason>>, LocalPeerMessages),
                                                                            (Arc<Peer<LocalPeerMessages, LocalPeerDisconnectionReason>>, Box<dyn std::error::Error + Sync + Send>)>>)
                        -> impl Stream<Item = (Arc<Peer<LocalPeerMessages, LocalPeerDisconnectionReason>>, bool)> {

        request_processor_stream
            .filter_map(move |processor_response| async {
                let (peer, outgoing) = match processor_response {
                    Ok((peer, outgoing)) => {
                        trace!("Sending Answer `{:?}` to {:?} (peer id {})", outgoing, peer.peer_address, peer.peer_id);
                        (peer, outgoing)
                    },
                    Err((peer, err)) => {
                        let err_string = format!("{:?}", err);
                        error!("Socket Server's processor yielded an error: {}", err_string);
                        let error_message = LocalPeerMessages::processor_error_message(err_string);
                        (peer, error_message)
                    },
                };
                // send the message, skipping messages that are programmed not to generate any response
                if !LocalPeerMessages::is_no_answer_message(&outgoing) {
                    if let Some(_kickout_response) = LocalPeerMessages::is_disconnect_message(&outgoing) {
                        trace!("Server choose to drop connection with {:?} (peer id {}): '{:?}'", peer.peer_address, peer.peer_id, outgoing);
                        peer.sender.send(outgoing).await;   // sending back `outgoing` instead of `kickout_response`... ? If the latter is not needed, we may simplify the trait
                        peer.sender.close();
                    } else {
                        peer.sender.send(outgoing).await;
                    }
                    Some((peer, true))
                } else {
                    None
                }
            })
    }

}
