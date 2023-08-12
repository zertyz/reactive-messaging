//! Tokio version of the inspiring `message-io` crate, but improving it on the following (as of 2022-08):
//! 1) Tokio is used for async IO -- instead of something else `message-io` uses, which is out of Rust's async execution context;
//! 2) `message-io` has poor/no support for streams/textual protocols: it, eventually, breaks messages in overload scenarios,
//!    transforming 1 good message into 2 invalid ones -- its `FramedTCP` comm model is not subjected to that, for the length is prepended to the message;
//! 3) `message-io` is prone to DoS attacks: during flood tests, be it in Release or Debug mode, `message-io` answers to a single peer only -- the flooder.
//! 4) `message-io` uses ~3x more CPU and is single threaded -- throughput couldn't be measured for the network speed was the bottleneck; latency was not measured at all


use crate::ReactiveMessagingSerializer;

use super::{
    config::*,
    types::*,
    serde::ReactiveMessagingDeserializer,
};
use std::{
    fmt::Debug,
    future,
    future::Future,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering::Relaxed},
    },
    time::Duration,
    fmt::Formatter,
    net::{IpAddr, Ipv4Addr},
    str::FromStr,
};
use std::marker::PhantomData;
use std::time::SystemTime;
use reactive_mutiny::prelude::advanced::{Uni, GenericUni, ChannelCommon, ChannelUni, ChannelProducer, FullDuplexUniChannel, OgreAllocator};
use futures::{StreamExt, Stream, TryFutureExt};
use tokio::{
    io::{self,AsyncReadExt,AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use log::{trace, debug, warn, error};


// Contains abstractions, useful for clients and servers, for dealing with socket connections handled by Stream Processors:\
//   - handles all combinations of the Stream's output types returned by `dialog_processor_builder_fn()`: futures/non-future & fallible/non-fallible;
//   - handles unresponsive / responsive output types (also issued by the Streams created by `dialog_processor_builder_fn()`).
pub struct SocketConnectionHandler<const CONFIG:                    usize,
                                   RemoteMessagesType:              ReactiveMessagingDeserializer<RemoteMessagesType>                                   + Send + Sync + PartialEq + Debug + 'static,
                                   LocalMessagesType:               ReactiveMessagingSerializer<LocalMessagesType>                                      + Send + Sync + PartialEq + Debug + 'static,
                                   ProcessorUniType:                GenericUni<ItemType=RemoteMessagesType>                                             + Send + Sync + 'static,
                                   SenderChannelType:               FullDuplexUniChannel<ItemType=LocalMessagesType, DerivedItemType=LocalMessagesType> + Send + Sync + 'static> {
    _phantom:   PhantomData<(ProcessorUniType, SenderChannelType, RemoteMessagesType, LocalMessagesType)>,
}

impl<const CONFIG:                    usize,
     RemoteMessagesType:              ReactiveMessagingDeserializer<RemoteMessagesType>                                   + Send + Sync + PartialEq + Debug + 'static,
     LocalMessagesType:               ReactiveMessagingSerializer<LocalMessagesType>                                      + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:                GenericUni<ItemType=RemoteMessagesType>                                             + Send + Sync + 'static,
     SenderChannelType:               FullDuplexUniChannel<ItemType=LocalMessagesType, DerivedItemType=LocalMessagesType> + Send + Sync + 'static>
 SocketConnectionHandler<CONFIG, RemoteMessagesType, LocalMessagesType, ProcessorUniType, SenderChannelType> {

    //type ProcessorUniType = Uni<ProcessorUniType::ItemType, ProcessorUniType::UniChannelType, PROCESSOR_UNI_INSTRUMENTS, ProcessorUniType::DerivedItemType>   <== Rust 1.71 doesn't allow this yet
    //type SenderUniType    = Uni<SenderUniType::ItemType,    SenderUniType::UniChannelType,    SENDER_UNI_INSTRUMENTS,    SenderUniType::DerivedItemType>      <== Rust 1.71 doesn't allow this yet

    pub fn new() -> Self {
        Self {
            _phantom: PhantomData,
        }
    }

    // /// Type for the `Stream` we create when reading from the remote peer.\
    // /// This type is intended to be used only for the first level of `dialog_processor_builder()`s you pass to
    // /// the [SocketClient] or [SocketServer], as Rust Generics isn't able to infer a generic `Stream` type
    // /// in this situation (in which the `Stream` is created inside the generic function itself).\
    // /// If your logic uses functions that receive `Stream`s, you'll want flexibility to do whatever you want
    // /// with the `Stream` (which would no longer be a `MutinyStream`), so declare such functions as:
    // /// ```no_compile
    // ///     fn dialog_processor<RemoteStreamType: Stream<Item=YourSocketClientOrServerType::SocketProcessorDerivedType>>
    // ///                        (remote_messages_stream: RemoteStreamType) -> impl Stream<Item=LocalMessages> { ... }
    // pub type ProcessorRemoteStreamType
    //     = MutinyStream<'static, RemoteMessagesType,
    //                             <UniRemoteMessagesType as UniGenericTypes>::UniChannelType,
    //                             <UniRemoteMessagesType as UniGenericTypes>::DerivedItemType>;

    /// Serves textual protocols whose events/commands/sentences are separated by '\n':
    ///   - `dialog_processor_builder_fn()` builds streams that will receive `ClientMessages` but will generate no answers;
    ///   - sending messages back to the client will be done explicitly by `peer`;
    ///   - `shutdown_signaler` comes from a `tokio::sync::oneshot::channel`, which receives the maximum time, in
    ///     milliseconds: it will be passed to `connection_events_callback` and cause the server loop to exit.
    #[inline(always)]
    pub async fn server_loop_for_unresponsive_text_protocol<PipelineOutputType:                                                                                                                                                                                                                   Send + Sync + Debug + 'static,
                                                            OutputStreamType:               Stream<Item=PipelineOutputType>                                                                                                                                                                     + Send                + 'static,
                                                            ConnectionEventsCallbackFuture: Future<Output=()>                                                                                                                                                                                   + Send,
                                                            ConnectionEventsCallback:       Fn(/*server_event: */ConnectionEvent<SenderChannelType>)                                                                                                          -> ConnectionEventsCallbackFuture + Send + Sync         + 'static,
                                                            ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<SenderChannelType>>, /*client_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> OutputStreamType               + Send + Sync         + 'static>

                                                           (self,
                                                            listening_interface:         String,
                                                            listening_port:              u16,
                                                            mut shutdown_signaler:       tokio::sync::oneshot::Receiver<u32>,
                                                            connection_events_callback:  ConnectionEventsCallback,
                                                            dialog_processor_builder_fn: ProcessorBuilderFn)

                                                           -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

        let arc_self = Arc::new(self);
        let connection_events_callback = Arc::new(connection_events_callback);
        let listener = TcpListener::bind(&format!("{}:{}", listening_interface, listening_port)).await?;

        tokio::spawn( async move {

            loop {

                // wait for a connection -- or for a shutdown signal
                let (socket, addr) = if let Some(accepted_connection) = tokio::select! {
                    // incoming connection
                    acceptance_result = listener.accept() => {
                        if let Err(err) = acceptance_result {
                            error!("PROBLEM while accepting a connection: {:?}", err);
                            None
                        } else {
                            Some(acceptance_result.unwrap())
                        }
                    }
                    // shutdown signal
                    result = &mut shutdown_signaler => {
                        let timeout_ms = match result {
                            Ok(timeout_millis) => {
                                trace!("SocketServer: SHUTDOWN requested for server @ {listening_interface}:{listening_port} -- with timeout {}ms -- bailing out from the network loop", timeout_millis);
                                timeout_millis
                            },
                            Err(err) => {
                                error!("SocketServer: PROBLEM in the `shutdown signaler` for server @ {listening_interface}:{listening_port} (a server shutdown will be commanded now due to this occurrence): {:?}", err);
                                5000    // consider this as the "problematic shutdown timeout (millis) constant"
                            },
                        };
                        // issue the shutdown event
                        connection_events_callback(ConnectionEvent::ApplicationShutdown {timeout_ms}).await;
                        break
                    }
                } {
                    accepted_connection
                } else {
                    // error accepting -- not fatal: try again
                    continue
                };

                // prepares for the dialog to come with the (just accepted) connection
                let sender = SenderChannelType::new(format!("Sender for client {addr}"));
                let peer = Arc::new(Peer::new(sender, addr));
                let peer_ref1 = Arc::clone(&peer);
                let peer_ref2 = Arc::clone(&peer);

                // issue the connection event
                connection_events_callback(ConnectionEvent::PeerConnected {peer: peer.clone()}).await;

                let (client_ip, client_port) = match addr {
                    SocketAddr::V4(v4) => (v4.ip().to_string(), v4.port()),
                    SocketAddr::V6(v6) => (v6.ip().to_string(), v6.port()),
                };

                let connection_events_callback_ref = Arc::clone(&connection_events_callback);
                let processor_sender = ProcessorUniType::new(format!("Server processor for remote client {addr} @ {listening_interface}:{listening_port}"))
                    .spawn_non_futures_non_fallibles_executors(1,
                                                               |in_stream| dialog_processor_builder_fn(client_ip.clone(), client_port, peer_ref1.clone(), in_stream),
                                                               move |executor| async move {
                                                                // issue the async disconnect event
                                                                connection_events_callback_ref(ConnectionEvent::PeerDisconnected { peer: peer_ref2, stream_stats: executor }).await;
                                                            });

                // spawn a task to handle communications with that client
                let cloned_self = Arc::clone(&arc_self);
                tokio::spawn(tokio::task::unconstrained(cloned_self.dialog_loop_for_textual_protocol(socket, peer, processor_sender)));
            }
            debug!("SocketServer: bailing out of network loop -- we should be undergoing a shutdown...");
        });

        Ok(())
    }

    /// Executes a client for textual protocols whose events/commands/sentences are separated by '\n':
    ///   - `dialog_processor_builder_fn()` builds streams that will receive `ServerMessages` but will generate no answers;
    ///   - sending messages back to the server will be done explicitly by `peer`;
    ///   - `shutdown_signaler` comes from a `tokio::sync::oneshot::channel`, which receives the maximum time, in
    ///     milliseconds: it will be passed to `connection_events_callback` and cause the server loop to exit.
    #[inline(always)]
    pub async fn client_for_unresponsive_text_protocol<PipelineOutputType:                                                                                                                                                                                                                   Send + Sync + Debug + 'static,
                                                       OutputStreamType:               Stream<Item=PipelineOutputType>                                                                                                                                                                     + Send                + 'static,
                                                       ConnectionEventsCallbackFuture: Future<Output=()> + Send,
                                                       ConnectionEventsCallback:       Fn(/*server_event: */ConnectionEvent<SenderChannelType>)                                                                                                          -> ConnectionEventsCallbackFuture + Send + Sync         + 'static,
                                                       ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<SenderChannelType>>, /*server_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> OutputStreamType>

                                                      (self,
                                                       server_ipv4_addr:            String,
                                                       port:                        u16,
                                                       shutdown_signaler:           tokio::sync::oneshot::Receiver<u32>,
                                                       connection_events_callback:  ConnectionEventsCallback,
                                                       dialog_processor_builder_fn: ProcessorBuilderFn)

                                                      -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::from_str(server_ipv4_addr.as_str())?), port);
        let socket = TcpStream::connect(addr).await?;

        // prepares for the dialog to come with the (just accepted) connection
        let sender = SenderChannelType::new(format!("Sender for client {addr}"));
        let peer = Arc::new(Peer::new(sender, addr));
        let peer_ref1 = Arc::clone(&peer);
        let peer_ref2 = Arc::clone(&peer);
        let peer_ref3 = Arc::clone(&peer);

        // issue the connection event
        connection_events_callback(ConnectionEvent::PeerConnected {peer: peer.clone()}).await;

        let connection_events_callback_ref1 = Arc::new(connection_events_callback);
        let connection_events_callback_ref2 = Arc::clone(&connection_events_callback_ref1);
        let processor_sender = ProcessorUniType::new(format!("Client Processor for remote server @ {addr}"))
            .spawn_non_futures_non_fallibles_executors(1,
                                                    |in_stream| dialog_processor_builder_fn(server_ipv4_addr.clone(), port, peer_ref1.clone(), in_stream),
                                                    move |executor| async move {
                                                        // issue the async disconnect event
                                                        connection_events_callback_ref1(ConnectionEvent::PeerDisconnected { peer: peer_ref2, stream_stats: executor }).await;
                                                    });

        // spawn the processor
        let arc_self = Arc::new(self);
        tokio::spawn(tokio::task::unconstrained(arc_self.dialog_loop_for_textual_protocol(socket, peer, processor_sender)));
        // spawn the shutdown listener
        tokio::spawn(async move {
            let timeout_ms = match shutdown_signaler.await {
                Ok(timeout_millis) => {
                    trace!("reactive-messaging: SHUTDOWN requested for client connected to server @ {server_ipv4_addr}:{port} -- with timeout {}ms: notifying & dropping the connection", timeout_millis);
                    timeout_millis
                },
                Err(err) => {
                    error!("reactive-messaging: PROBLEM in the `shutdown signaler` client connected to server @ {server_ipv4_addr}:{port} (a client shutdown will be commanded now due to this occurrence): {:?}", err);
                    5000    // consider this as the "problematic shutdown timeout (millis) constant"
                },
            };
            // issue the shutdown event
            connection_events_callback_ref2(ConnectionEvent::ApplicationShutdown {timeout_ms}).await;
            // close the connection
            peer_ref3.sender.gracefully_end_all_streams(Duration::from_millis(timeout_ms as u64)).await;
        });

        Ok(())
    }

    /// Handles the "local" side of the peers dialog that is to take place once the connection is established -- provided the communications are done through a textual chat
    /// in which each event/command/sentence ends in '\n'.\
    /// The protocol is determined by `LocalMessages` & `RemoteMessages`, where and each one may either be the client messages or server messages, depending on who is the "local peer":
    ///   - `peer` represents the remote end of the connection;
    ///   - `processor_sender` is a [reactive_mutiny::Uni], to which incoming messages will be sent;
    ///   - conversely, `peer.sender` is the [reactive_mutiny::Uni] to receive outgoing messages.
    #[inline(always)]
    async fn dialog_loop_for_textual_protocol(self: Arc<Self>,
                                              mut textual_socket: TcpStream,
                                              peer:               Arc<Peer<SenderChannelType>>,
                                              processor_uni:      Arc<ProcessorUniType>)

                                             -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

        let config = ConstConfig::from(CONFIG);
        // socket
        if let Some(no_delay) = config.socket_options.no_delay {
            textual_socket.set_nodelay(no_delay).map_err(|err| format!("error setting nodelay({no_delay}) for the socket connected at {}:{}: {err}", peer.peer_address, peer.peer_id))?;
        }
        if let Some(hops_to_live) = config.socket_options.hops_to_live {
            let hops_to_live = hops_to_live.get() as u32;
            textual_socket.set_ttl(hops_to_live).map_err(|err| format!("error setting ttl({hops_to_live}) for the socket connected at {}:{}: {err}", peer.peer_address, peer.peer_id))?;
        }
        if let Some(millis) = config.socket_options.linger_millis {
            textual_socket.set_linger(Some(Duration::from_millis(millis as u64))).map_err(|err| format!("error setting linger({millis}ms) for the socket connected at {}:{}: {err}", peer.peer_address, peer.peer_id))?;
        }
        // buffers
        let mut read_buffer = Vec::with_capacity(config.msg_size_hint as usize);
        let mut serialization_buffer = Vec::with_capacity(config.msg_size_hint as usize);

        let (mut sender_stream, _) = peer.sender.create_stream();

        'connection: loop {
            // wait for the socket to be readable or until we have something to write
            tokio::select!(

                biased;     // sending has priority over receiving

                // send?
                to_send = sender_stream.next() => {
                    match to_send {
                        Some(to_send) => {
                            LocalMessagesType::serialize(&to_send, &mut serialization_buffer);
                            serialization_buffer.push(b'\n');
                            if let Err(err) = textual_socket.write_all(&serialization_buffer).await {
                                warn!("reactive-messaging: PROBLEM in the connection with {:#?} while WRITING: '{:?}' -- dropping it", peer, err);
                                break 'connection
                            }
                        },
                        None => {
                            debug!("reactive-messaging: Sender for {:#?} ended (most likely, .cancel_all_streams() was called on the `peer` by the processor.", peer);
                            break 'connection
                        }
                    }
                },

                // read?
                read = textual_socket.read_buf(&mut read_buffer) => {
                    match read {
                        Ok(n) if n > 0 => {
                            let mut next_line_index = 0;
                            let mut this_line_search_start = read_buffer.len() - n;
                            loop {
                                if let Some(mut eol_pos) = read_buffer[next_line_index+this_line_search_start..].iter().position(|&b| b == b'\n') {
                                    eol_pos += next_line_index+this_line_search_start;
                                    let line_bytes = &read_buffer[next_line_index..eol_pos];
                                    match RemoteMessagesType::deserialize(line_bytes) {
                                        Ok(client_message) => {
                                            if let Err((abort_processor, error_msg_processor)) = self.send_to_local_processor(&processor_uni, client_message).await {
                                                // log & send the error message to the remote peer
                                                error!("reactive-messaging: {} -- `dialog_processor` is full of unprocessed messages ({}/{})", error_msg_processor, processor_uni.pending_items_count(), processor_uni.buffer_size());
                                                if let Err((abort_sender, error_msg_sender)) = self.send_to_remote_peer(&peer.sender, LocalMessagesType::processor_error_message(error_msg_processor)).await {
                                                        warn!("reactive-messaging: {error_msg_sender} -- Slow reader {:?}", peer);
                                                    if abort_sender {
                                                        break 'connection
                                                    }
                                                }
                                                if abort_processor {
                                                    break 'connection
                                                }
                                            }
                                        },
                                        Err(err) => {
                                            let stripped_line = String::from_utf8_lossy(line_bytes);
                                            let error_message = format!("Unknown command received from {:?} (peer id {}): '{}': {}",
                                                                            peer.peer_address, peer.peer_id, stripped_line, err);
                                            // log & send the error message to the remote peer
                                            warn!("reactive-messaging: {error_message}");
                                            let outgoing_error = LocalMessagesType::processor_error_message(error_message);
                                            if let Err((abort, error_msg)) = self.send_to_remote_peer(&peer.sender, outgoing_error).await {
                                                if abort {
                                                    warn!("reactive-messaging: {error_msg} -- Slow reader {:?}", peer);
                                                    break 'connection
                                                }
                                            }
                                        }
                                    }
                                    next_line_index = eol_pos + 1;
                                    this_line_search_start = 0;
                                    if next_line_index >= read_buffer.len() {
                                        read_buffer.clear();
                                        break
                                    }
                                } else {
                                    if next_line_index > 0 {
                                        read_buffer.drain(0..next_line_index);
                                    }
                                    break
                                }
                            }
                        },
                        Ok(_) /* zero bytes received */ => {
                            warn!("reactive-messaging: PROBLEM with reading from {:?} (peer id {}) -- it is out of bytes! Dropping the connection", peer.peer_address, peer.peer_id);
                            break 'connection
                        },
                        Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {},
                        Err(err) => {
                            error!("reactive-messaging: ERROR in the connection with {:?} (peer id {}) while READING: '{:?}' -- dropping it", peer.peer_address, peer.peer_id, err);
                            break 'connection
                        },
                    }
                },
            );

        }
        _ = processor_uni.close(Duration::from_millis(config.graceful_close_timeout_millis as u64)).await;
        peer.sender.cancel_all_streams();
        textual_socket.flush().await.map_err(|err| format!("error flushing the textual socket connected to {}:{}: {}", peer.peer_address, peer.peer_id, err))?;
        textual_socket.shutdown().await.map_err(|err| format!("error flushing the textual socket connected to {}:{}: {}", peer.peer_address, peer.peer_id, err))?;
        Ok(())
    }


    // Responsive variants
    //////////////////////

    /// Similar to [server_loop_for_unresponsive_text_protocol()], but for when the provided `dialog_processor_builder_fn()` produces
    /// answers of type `ServerMessages`, which will be automatically routed to the clients.
    #[inline(always)]
    pub async fn server_loop_for_responsive_text_protocol<OutputStreamType:               Stream<Item=LocalMessagesType>                                                                                                                                                                      + Send        + 'static,
                                                          ConnectionEventsCallbackFuture: Future<Output=()>                                                                                                                                                                                   + Send,
                                                          ConnectionEventsCallback:       Fn(/*server_event: */ConnectionEvent<SenderChannelType>)                                                                                                          -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                                          ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<SenderChannelType>>, /*client_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> OutputStreamType               + Send + Sync + 'static>

                                                         (self,
                                                          listening_interface:         String,
                                                          listening_port:              u16,
                                                          shutdown_signaler:           tokio::sync::oneshot::Receiver<u32>,
                                                          connection_events_callback:  ConnectionEventsCallback,
                                                          dialog_processor_builder_fn: ProcessorBuilderFn)

                                                         -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

        // upgrade the "unresponsive" streams from `dialog_processor_builder_fn()` to "responsive" streams that will send the outcome to clients
//        let dialog_processor_builder_fn = move |client_addr, connected_port, peer, client_messages_stream| {
//            let dialog_processor_stream = dialog_processor_builder_fn(client_addr, connected_port, Arc::clone(&peer), client_messages_stream);
//            self.to_responsive_stream(peer, dialog_processor_stream)
//        };
        self.server_loop_for_unresponsive_text_protocol(listening_interface.clone(), listening_port, shutdown_signaler, connection_events_callback, dialog_processor_builder_fn).await
            .map_err(|err| Box::from(format!("error when starting server @ {listening_interface}:{listening_port} with `server_loop_for_unresponsive_text_protocol()`: {err}")))
    }

    /// Similar to [client_for_unresponsive_text_protocol()], but for when the provided `dialog_processor_builder_fn()` produces
    /// answers of type `ClientMessages`, which will be automatically routed to the server.
    #[inline(always)]
    pub async fn client_for_responsive_text_protocol<OutputStreamType:               Stream<Item=LocalMessagesType>                                                                                                                                                                      + Send +        'static,
                                                     ConnectionEventsCallbackFuture: Future<Output=()> + Send,
                                                     ConnectionEventsCallback:       Fn(/*server_event: */ConnectionEvent<SenderChannelType>)                                                                                                          -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                                     ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<SenderChannelType>>, /*server_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> OutputStreamType>

                                                    (self,
                                                     server_ipv4_addr:            String,
                                                     port:                        u16,
                                                     shutdown_signaler:           tokio::sync::oneshot::Receiver<u32>,
                                                     connection_events_callback:  ConnectionEventsCallback,
                                                     dialog_processor_builder_fn: ProcessorBuilderFn)

                                                    -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

//        // upgrade the "unresponsive" streams from `dialog_processor_builder_fn()` to "responsive" streams that will send the outcome to clients
//        let dialog_processor_builder_fn = |server_addr, port, peer, server_messages_stream| {
//            let dialog_processor_stream = dialog_processor_builder_fn(server_addr, port, Arc::clone(&peer), server_messages_stream);
//            self.to_responsive_stream(peer, dialog_processor_stream)
//        };
        self.client_for_unresponsive_text_protocol(server_ipv4_addr.clone(), port, shutdown_signaler, connection_events_callback, dialog_processor_builder_fn).await
            .map_err(|err| Box::from(format!("error when starting client for server @ {server_ipv4_addr}:{port} with `client_for_unresponsive_text_protocol()`: {err}")))
    }
/*
    /// upgrades the `request_processor_stream` (of non-fallible & non-future items) to a `Stream` which is also able send answers back to the `peer`
    #[inline(always)]
    fn to_responsive_stream<ResponsiveLocalMessagesType: ReactiveMessagingSerializer<ResponsiveLocalMessagesType> +
                                                         ResponsiveMessages<ResponsiveLocalMessagesType>          + Send + Sync + PartialEq + Debug + 'static>
                           (&self,
                            peer:                     Arc<Peer<SenderChannelType>>,
                            request_processor_stream: impl Stream<Item=ResponsiveLocalMessagesType>)

                           -> impl Stream<Item = ()> {

        request_processor_stream
            .map(move |outgoing| {
                // wired `if`s ahead to try to get some branch prediction optimizations -- assuming Rust will see the first `if` branches as the "usual case"
                let is_disconnect = ResponsiveLocalMessagesType::is_disconnect_message(&outgoing);
                let is_no_answer = ResponsiveLocalMessagesType::is_no_answer_message(&outgoing);
                // send the message, skipping messages that are programmed not to generate any response
                if !is_disconnect && !is_no_answer {
                    // send the answer
                    trace!("reactive-messaging: Sending Answer `{:?}` to {:?} (peer id {})", outgoing, peer.peer_address, peer.peer_id);
                    if let Err((abort, error_msg)) = self.send_to_remote_peer(&peer.sender, outgoing) {
                        // peer is slow-reading -- and, possibly, fast sending
                        warn!("reactive-messaging: {error_msg} -- Slow reader {:?}", peer);
                        if abort {
                            peer.sender.cancel_all_streams();
                        }
                    }
                } else if is_disconnect {
                    trace!("reactive-messaging: SocketServer: processor choose to drop connection with {} (peer id {}): '{:?}'", peer.peer_address, peer.peer_id, outgoing);
                    if !is_no_answer {
                        let _sent = peer.sender.try_send(|slot| unsafe { std::ptr::write(slot,  outgoing) } );
                        // TODO apply config.retrying_strategy here
                    }
                    peer.sender.cancel_all_streams();
                }
            })
    }

    /// upgrades the `request_processor_stream` (of fallible & non-future items) to a `Stream` which is also able send answers back to the `peer`
    #[inline(always)]
    fn _to_responsive_stream_of_fallibles<const CONST_CONFIG: usize,
                                        LocalMessages:      ReactiveMessagingSerializer<LocalMessages> +
                                                            ResponsiveMessages<LocalMessages>          + Send + Sync + PartialEq + Debug + 'static>

                                        (peer:                     Arc<Peer<CONST_CONFIG, LocalMessages>>,
                                        request_processor_stream: impl Stream<Item = Result<LocalMessages, Box<dyn std::error::Error + Sync + Send>>>)

                                        -> impl Stream<Item = ()> {

        let peer_ref1 = peer;
        let peer_ref2 = Arc::clone(&peer_ref1);
        // treat any errors
        let request_processor_stream = request_processor_stream
            .map(move |processor_response| {
                match processor_response {
                    Ok(outgoing) => {
                        outgoing
                    },
                    Err(err) => {
                        let err_string = format!("{:?}", err);
                        error!("SocketServer: processor connected with {} (peer id {}) yielded an error: {}", peer_ref1.peer_address, peer_ref1.peer_id, err_string);
                        LocalMessages::processor_error_message(err_string)
                    },
                }
            });
        // process the answer
        self.to_responsive_stream(peer_ref2, request_processor_stream)
    }
*/
    /// Routes a received message to the local processor, honoring the configured retrying options.\
    /// Returns `Ok` if sent, `Err(details)` if sending was not possible, where `details` contain:
    ///   - `(abort?, error_message, unsent_message)`
    #[inline(always)]
    async fn send_to_local_processor(&self,
                                     processor_uni: &ProcessorUniType,
                                     message:       RemoteMessagesType)
                                    -> Result<(), (/*abort?*/bool, /*error_message: */String)> {

        /// mapper for eventual first-time-being retrying attempts -- or for fatal errors that might happen during retrying
        fn retry_error_mapper(abort: bool, error_msg: String) -> ((), (bool, String) ) {
            ( (), (abort, error_msg) )
        }
        /// mapper for any fatal errors that happens on the first attempt (which should not happen in the current `reactive-mutiny` Uni Channel API)
        fn first_attempt_error_mapper<T>(_: T, _: ()) -> ((), (bool, String) ) {
            panic!("reactive-messaging: send_to_local_processor(): BUG! `Uni` channel is expected never to fail fatably. Please, fix!")
        }

        let config = ConstConfig::from(CONFIG);
        let retryable = processor_uni.send(message);
        match config.retrying_strategy {
            RetryingStrategies::DoNotRetry => {
                retryable
                    .map_inputs_and_errors(
                        first_attempt_error_mapper,
                        |message, _err|
                            retry_error_mapper(false, format!("Relaying received message '{:?}' to the internal processor failed. Won't retry (ignoring the error) due to retrying config {:?}",
                                                                              message, config.retrying_strategy)) )
                    .into_result()
            },
            RetryingStrategies::EndCommunications => {
                retryable
                    .map_inputs_and_errors(
                        first_attempt_error_mapper,
                        |message, _err|
                            retry_error_mapper(false, format!("Relaying received message '{:?}' to the internal processor failed. Connection will be aborted (without retrying) due to config {:?}",
                                                                              message, config.retrying_strategy)) )
                    .into_result()
            },
            RetryingStrategies::RetrySleepingArithmetically(steps) => {
                retryable
                    .map_input(|message| ( message, SystemTime::now()) )
                    .retry_with_async(|(message, retry_start)| future::ready(
                        processor_uni.send(message)
                            .map_input(|message| (message, retry_start) )
                    ))
                    // .with_delays((10..=(10*steps as u64)).step_by(10).map(|millis| Duration::from_millis(millis)))
.yielding_until_timeout(Duration::from_millis(steps as u64), || ())
                    .await
                    .map_inputs_and_errors(
                        |(message, retry_start), _fatal_err|
                            retry_error_mapper(true, format!("Relaying received message '{:?}' to the internal processor failed. Connection will be aborted (after exhausting all retries in {:?}) due to config {:?}",
                                                                             message, retry_start.elapsed(), config.retrying_strategy)),
                        |_| (false, String::with_capacity(0)) )
                    .into()
            },
            RetryingStrategies::RetryYieldingForUpToMillis(millis) => {
                retryable
                    .map_input(|message| ( message, SystemTime::now()) )
                    .retry_with_async(|(message, retry_start)| future::ready(
                        processor_uni.send(message)
                            .map_input(|message| (message, retry_start) )
                    ))
                    .yielding_until_timeout(Duration::from_millis(millis as u64), || ())
                    .await
                    .map_inputs_and_errors(
                        |(message, retry_start), _fatal_err|
                            retry_error_mapper(true, format!("Relaying received message '{:?}' to the internal processor failed. Connection will be aborted (after exhausting all retries in {:?}) due to config {:?}",
                                                                             message, retry_start.elapsed(), config.retrying_strategy)),
                        |_| (false, String::with_capacity(0)) )
                    .into()
            },
            RetryingStrategies::RetrySpinningForUpToMillis(millis) => {
                panic!("deprecated")
            },
        }
    }

    /// Sends a message to the remote peer, honoring the configured retrying options.\
    /// Returns `Ok` if sent, `Err(details)` if sending was not possible, where `details` contain:
    ///   - `(abort?, error_message)`
    #[inline(always)]
    async fn send_to_remote_peer(&self,
                                 sender_channel: &SenderChannelType,
                                 message:        LocalMessagesType)
                                -> Result<(), (/*abort?*/bool, /*error_msg*/String)> {

        /// mapper for eventual first-time-being retrying attempts -- or for fatal errors that might happen during retrying
        fn retry_error_mapper(abort: bool, error_msg: String) -> ((), (bool, String) ) {
            ( (), (abort, error_msg) )
        }
        /// mapper for any fatal errors that happens on the first attempt (which should not happen in the current `reactive-mutiny` Uni Channel API)
        fn first_attempt_error_mapper<T>(_: T, _: ()) -> ((), (bool, String) ) {
            panic!("reactive-messaging: send_to_remote_peer(): BUG! `Uni` channel is expected never to fail fatably. Please, fix!")
        }

        let config = ConstConfig::from(CONFIG);
        let retryable = sender_channel.send(message);
        match config.retrying_strategy {
            RetryingStrategies::DoNotRetry => {
                retryable
                    .map_inputs_and_errors(
                        first_attempt_error_mapper,
                        |message, _err|
                            retry_error_mapper(false, format!("Sending '{:?}' failed. Won't retry (ignoring the error) due to retrying config {:?}",
                                                                              message, config.retrying_strategy)) )
                    .into_result()
            },
            RetryingStrategies::EndCommunications => {
                retryable
                    .map_inputs_and_errors(
                        first_attempt_error_mapper,
                        |message, _err|
                            retry_error_mapper(true, format!("Sending '{:?}' failed. Connection will be aborted (without retrying) due to config {:?}",
                                                                             message, config.retrying_strategy)) )
                    .into_result()
            },
            RetryingStrategies::RetrySleepingArithmetically(steps) => {
                retryable
                    .map_input(|message| ( message, SystemTime::now()) )
                    .retry_with_async(|(message, retry_start)| future::ready(
                        sender_channel.send(message)
                            .map_input(|message| (message, retry_start) )
                    ))
                    //.with_delays((10..=(10*steps as u64)).step_by(10).map(|millis| Duration::from_millis(millis)))
.yielding_until_timeout(Duration::from_millis(steps as u64), || ())
                    .await
                    .map_inputs_and_errors(
                        |(message, retry_start), _fatal_err|
                            retry_error_mapper(true, format!("Sending '{:?}' failed. Connection will be aborted (after exhausting all retries in {:?}) due to config {:?}",
                                                                             message, retry_start.elapsed(), config.retrying_strategy)),
                        |_| (false, String::with_capacity(0)) )
                    .into()
            },
            RetryingStrategies::RetryYieldingForUpToMillis(millis) => {
                retryable
                    .map_input(|message| ( message, SystemTime::now()) )
                    .retry_with_async(|(message, retry_start)| future::ready(
                        sender_channel.send(message)
                            .map_input(|message| (message, retry_start) )
                    ))
                    .yielding_until_timeout(Duration::from_millis(millis as u64), || ())
                    .await
                    .map_inputs_and_errors(
                        |(message, retry_start), _fatal_err|
                            retry_error_mapper(true, format!("Sending '{:?}' failed. Connection will be aborted (after exhausting all retries in {:?}) due to config {:?}",
                                                                             message, retry_start.elapsed(), config.retrying_strategy)),
                        |_| (false, String::with_capacity(0)) )
                    .into()
            },
            RetryingStrategies::RetrySpinningForUpToMillis(millis) => {
                panic!("deprecated")
            },
        }
    }
}


static PEER_COUNTER: AtomicU32 = AtomicU32::new(0);
pub type PeerId = u32;
/// Represents a channel to a peer able to send out `MessageType` kinds of messages from "here" to "there"
pub struct Peer<SenderChannelType: FullDuplexUniChannel + Send + Sync> {
    pub peer_id:      PeerId,
    pub sender:       Arc<SenderChannelType>,
    pub peer_address: SocketAddr,
}

impl<SenderChannelType: FullDuplexUniChannel + Send + Sync>
Peer<SenderChannelType> {

    pub fn new(sender: Arc<SenderChannelType>, peer_address: SocketAddr) -> Self {
        Self {
            peer_id: PEER_COUNTER.fetch_add(1, Relaxed),
            sender,
            peer_address,
        }
    }

}

impl<SenderChannelType: FullDuplexUniChannel + Send + Sync>
Debug for
Peer<SenderChannelType> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Peer {{peer_id: {}, peer_address: '{}', sender: {}/{} pending messages}}",
                self.peer_id, self.peer_address, self.sender.pending_items_count(), self.sender.buffer_size())
    }
}

/// Adherents will, typically, also implement [ReactiveMessagingUnresponsiveSerializer].\
/// By upgrading your type with this trait, it is possible to build a "Responsive Processor", where the returned `Stream`
/// contains the messages to be sent as an answer to the remote peer.\
/// This trait, therefore, specifies (to the internal sender) how to handle special response cases, like "no answer" and "disconnection" messages.
pub trait ResponsiveMessages<LocalPeerMessages: ResponsiveMessages<LocalPeerMessages> + Send + PartialEq + Debug> {

    /// Informs the internal sender if the given `processor_answer` is a "disconnect" message & command (issued by the messages processor logic)\
    /// -- in which case, the network processor will send it and, immediately, close the connection.\
    /// IMPLEMENTORS: #[inline(always)]
    fn is_disconnect_message(processor_answer: &LocalPeerMessages) -> bool;

    /// Tells if internal sender if the given `processor_answer` represents a "no message" -- a message that should produce no answer to the peer.\
    /// IMPLEMENTORS: #[inline(always)]
    fn is_no_answer_message(processor_answer: &LocalPeerMessages) -> bool;
}


/// Unit tests the [tokio_message_io](self) module
#[cfg(any(test,doc))]
mod tests {
    use futures::stream;
    use tokio::sync::Mutex;

    use super::*;
    use std::{
        time::SystemTime, sync::atomic::AtomicBool,
    };
    use reactive_mutiny::prelude::advanced::{UniZeroCopyAtomic, ChannelUniMoveAtomic, ChannelUniZeroCopyAtomic};
    use reactive_mutiny::prelude::Instruments;

    #[cfg(debug_assertions)]
    const DEBUG: bool = true;
    #[cfg(not(debug_assertions))]
    const DEBUG: bool = false;

    const DEFAULT_TEST_CONFIG: ConstConfig           = ConstConfig::default();
    const DEFAULT_TEST_CONFIG_USIZE: usize           = DEFAULT_TEST_CONFIG.into();
    const DEFAULT_TEST_UNI_INSTRUMENTS: usize        = DEFAULT_TEST_CONFIG.executor_instruments.into();
    type DefaultTestUni<PayloadType = String>        = UniZeroCopyAtomic<PayloadType, {DEFAULT_TEST_CONFIG.receiver_buffer as usize}, 1, DEFAULT_TEST_UNI_INSTRUMENTS>;
    type DefaultTestUniChannel<PayloadType = String> = ChannelUniMoveAtomic<PayloadType, {DEFAULT_TEST_CONFIG.sender_buffer as usize}, 1>;


    #[ctor::ctor]
    fn suite_setup() {
        simple_logger::SimpleLogger::new().with_utc_timestamps().init().unwrap_or_else(|_| eprintln!("--> LOGGER WAS ALREADY STARTED"));
    }

    /// Checks if the test configs make sense & the libraries are still working under the same contract,
    /// popping up any errors, as soon as possible, that would be difficult to debug otherwise
    #[cfg_attr(not(doc),tokio::test)]
    async fn sanity_check() {

        // check configs
        ////////////////
        println!("Original object: {:#?}", DEFAULT_TEST_CONFIG);
        println!("Reconverted:     {:#?}", ConstConfig::from(DEFAULT_TEST_CONFIG_USIZE));
        assert_eq!(ConstConfig::from(DEFAULT_TEST_CONFIG_USIZE), DEFAULT_TEST_CONFIG, "Configs don't match");
        // try to create the objects based on the config (a compilation error is expected if wrong const generic parameters are provided to the `Uni` type)
        let uni    = DefaultTestUni::<String>::new("Can it be instantiated?");
        let sender = DefaultTestUniChannel::<String>::new("Can it be instantiated?");

        // `reactive-mutiny` checks
        ///////////////////////////
        // channel (move)
        let channel_payload = String::from("Sender Payload");
        let (mut stream, _stream_id) = sender.create_stream();
        assert!(sender.send(channel_payload.clone()).is_ok(), "`reactive-mutiny`: channel: couldn't send");
        assert_eq!(stream.next().await, Some(channel_payload),      "`reactive-mutiny`: channel: couldn't receive");
        assert_eq!(sender.gracefully_end_all_streams(Duration::from_millis(100)).await, 1, "`reactive-mutiny` Streams should only be reported as 'gracefully ended' after they are consumed to exhaustion");
        assert_eq!(stream.next().await, None, "`reactive-mutiny`: channel: couldn't end the Stream");
        // channel (zero-copy)
        let sender = ChannelUniZeroCopyAtomic::<String, {DEFAULT_TEST_CONFIG.sender_buffer as usize}, 1>::new("Please work also...");
        let channel_payload = String::from("Sender Payload");
        let (mut stream, _stream_id) = sender.create_stream();
        assert!(sender.send(channel_payload.clone()).is_ok(), "`reactive-mutiny`: channel: couldn't send");
        assert_eq!(stream.next().await.expect("empty"), channel_payload, "`reactive-mutiny`: channel: couldn't receive");
        assert_eq!(sender.gracefully_end_all_streams(Duration::from_millis(100)).await, 1, "`reactive-mutiny` Streams should only be reported as 'gracefully ended' after they are consumed to exhaustion");
        assert!(stream.next().await.is_none(), "`reactive-mutiny`: channel: couldn't end the Stream");
        // uni
        let uni = uni.spawn_non_futures_non_fallibles_executors(1,
                                                                            |stream_in| stream_in.inspect(|e| println!("Received {}", e)),
                                                                            |_| future::ready(println!("ENDED")));
        assert!(uni.send(String::from("delivered?")).is_ok(), "`reactive-mutiny`: Uni: couldn't send");
        assert!(uni.close(Duration::from_millis(100)).await, "`reactive-mutiny` Streams being executed by an `Uni` should be reported as 'gracefully ended' as they should be promptly consumed to exhaustion");
    }


    /// assures connection & dialogs work for either the server and client, using the "unresponsive" flavours of the `Stream` processors
    #[cfg_attr(not(doc),tokio::test)]
    async fn unresponsive_dialogs() {
        const LISTENING_INTERFACE: &str = "127.0.0.1";
        const PORT               : u16  = 8570;
        let client_secret = String::from("open, sesame");
        let server_secret = String::from("now the 40 of you may enter");
        let observed_secret = Arc::new(Mutex::new(None));

        let (server_shutdown_sender, server_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (_client_shutdown_sender, client_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();

        // server
        let client_secret_ref = client_secret.clone();
        let server_secret_ref = server_secret.clone();
        let socket_connection_handler = SocketConnectionHandler::<DEFAULT_TEST_CONFIG_USIZE, String, String, DefaultTestUni, DefaultTestUniChannel>::new();
        socket_connection_handler.server_loop_for_unresponsive_text_protocol
                                                    (LISTENING_INTERFACE.to_string(), PORT, server_shutdown_receiver,
                                                     |connection_event| {
                                                         match connection_event {
                                                             ConnectionEvent::PeerConnected { peer } => {
                                                                 assert!(peer.sender.send(String::from("Welcome! State your business!")).is_ok(), "couldn't send");
                                                             },
                                                             ConnectionEvent::PeerDisconnected { peer: _, stream_stats: _ } => {},
                                                             ConnectionEvent::ApplicationShutdown { timeout_ms } => {
                                                                 println!("Test Server: shutdown was requested ({timeout_ms}ms timeout)... No connection will receive the drop message (nor will be even closed) because I, the lib caller, intentionally didn't keep track of the connected peers for this test!");
                                                             }
                                                         }
                                                         future::ready(())
                                                     },
                                                     move |_client_addr, _client_port, peer, client_messages_stream| {
                                                         let client_secret_ref = client_secret_ref.clone();
                                                         let server_secret_ref = server_secret_ref.clone();
                                                         client_messages_stream.inspect(move |client_message| {
                                                             assert!(peer.sender.send(format!("Client just sent '{}'", client_message)).is_ok(), "couldn't send");
                                                             if *client_message == client_secret_ref {
                                                                 assert!(peer.sender.send(server_secret_ref.clone()).is_ok(), "couldn't send");
                                                             } else {
                                                                 panic!("Client sent the wrong secret: '{}' -- I was expecting '{}'", client_message, client_secret_ref);
                                                             }
                                                         })
                                                     }
        ).await.expect("Starting the server");

        println!("### Waiting a little for the server to start...");
        tokio::time::sleep(Duration::from_millis(10)).await;

        // client
        let client_secret = client_secret.clone();
        let observed_secret_ref = Arc::clone(&observed_secret);
        let client_connection_handler = SocketConnectionHandler::<DEFAULT_TEST_CONFIG_USIZE, String, String, DefaultTestUni, DefaultTestUniChannel>::new();
        client_connection_handler.client_for_unresponsive_text_protocol
                                               (LISTENING_INTERFACE.to_string(), PORT, client_shutdown_receiver,
                                                move |connection_event| {
                                                    match connection_event {
                                                        ConnectionEvent::PeerConnected { peer } => {
                                                            assert!(peer.sender.send(client_secret.clone()).is_ok(), "couldn't send");
                                                        },
                                                        ConnectionEvent::PeerDisconnected { peer, stream_stats: _ } => {
                                                            println!("Test Client: connection with {} (peer_id #{}) was dropped -- should not happen in this test", peer.peer_address, peer.peer_id);
                                                        },
                                                        ConnectionEvent::ApplicationShutdown { timeout_ms: _ } => {}
                                                    }
                                                    future::ready(())
                                                },
                                                move |_client_addr, _client_port, _peer, server_messages_stream| {
                                                    let observed_secret_ref = Arc::clone(&observed_secret_ref);
                                                    server_messages_stream.then(move |server_message| {
                                                        let observed_secret_ref = Arc::clone(&observed_secret_ref);
                                                        async move {
                                                            println!("Server said: '{}'", server_message);
                                                            let _ = observed_secret_ref.lock().await.insert(server_message);
                                                        }
                                                    })
                                                }
        ).await.expect("Starting the client");
        println!("### Started a client -- which is running concurrently, in the background... it has 100ms to do its thing!");

        tokio::time::sleep(Duration::from_millis(100)).await;
        server_shutdown_sender.send(500).expect("sending server shutdown signal");

        println!("### Waiting a little for the shutdown signal to reach the server...");
        tokio::time::sleep(Duration::from_millis(10)).await;

        let locked_observed_secret = observed_secret.lock().await;
        let observed_secret = locked_observed_secret.as_ref().expect("Server secret has not been computed");
        assert_eq!(*observed_secret, server_secret, "Communications didn't go according the plan");
    }
/*
    /// Assures the minimum acceptable latency values -- for either Debug & Release modes.\
    /// One sends Ping(n); the other receives it and send Pong(n); the first receives it and sends Ping(n+1) and so on...\
    /// Latency is computed dividing the number of seconds per n*2 (we care about the server leg of the latency, while here we measure the round trip client<-->server)
    #[cfg_attr(not(doc),tokio::test(flavor = "multi_thread"))]
    #[ignore]   // convention for this project: ignored tests are to be run by a single thread
    async fn latency_measurements() {
        const TEST_DURATION_MS:    u64  = 2000;
        const TEST_DURATION_NS:    u64  = TEST_DURATION_MS * 1e6 as u64;
        const LISTENING_INTERFACE: &str = "127.0.0.1";
        const PORT               : u16  = 8571;
        let (server_shutdown_sender, server_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (client_shutdown_sender, client_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();

        // server
        server_loop_for_unresponsive_text_protocol::<1024, _, _, _, _, _, _, _>
                                                    (LISTENING_INTERFACE.to_string(), PORT, server_shutdown_receiver,
                                                     |_connection_event| async {},
                                                     |_listening_interface, _listening_port, peer, client_messages: ProcessorRemoteStreamType<1024, String>| {
                                                         client_messages.inspect(move |client_message| {
                                                             // Receives: "Ping(n)"
                                                             let n_str = &client_message[5..(client_message.len() - 1)];
                                                             let n = str::parse::<u32>(n_str).unwrap_or_else(|err| panic!("could not convert '{}' to number. Original client message: '{}'. Parsing error: {:?}", n_str, client_message, err));
                                                             // Answers:  "Pong(n)"
                                                             assert!(peer.sender.try_send_movable(format!("Pong({})", n)), "couldn't send");
                                                         })
                                                     }
        ).await.expect("Starting the server");

        println!("### Waiting a little for the server to start...");
        tokio::time::sleep(Duration::from_millis(10)).await;

        let counter = Arc::new(AtomicU32::new(0));
        let counter_ref = Arc::clone(&counter);
        // client
        client_for_unresponsive_text_protocol::<1024, _, _, _, _, _, _, _>
                                               (LISTENING_INTERFACE.to_string(), PORT, client_shutdown_receiver,
                                                |connection_event| {
                                                    match connection_event {
                                                        ConnectionEvent::PeerConnected { peer } => {
                                                            // conversation starter
                                                            assert!(peer.sender.try_send_movable(String::from("Ping(0)")), "couldn't send");
                                                        },
                                                        ConnectionEvent::PeerDisconnected { .. } => {},
                                                        ConnectionEvent::ApplicationShutdown { .. } => {},
                                                    }
                                                    future::ready(())
                                                },
                                                move |_listening_interface, _listening_port, peer, server_messages: ProcessorRemoteStreamType<1024, String>| {
                                                    let counter_ref = Arc::clone(&counter_ref);
                                                    server_messages.inspect(move |server_message| {
                                                        // Receives: "Pong(n)"
                                                        let n_str = &server_message[5..(server_message.len()-1)];
                                                        let n = str::parse::<u32>(n_str).unwrap_or_else(|err| panic!("could not convert '{}' to number. Original server message: '{}'. Parsing error: {:?}", n_str, server_message, err));
                                                        let current_count = counter_ref.fetch_add(1, Relaxed);
                                                        if n != current_count {
                                                            panic!("Received '{}', where Client was expecting 'Pong({})'", server_message, current_count);
                                                        }
                                                        // Answers: "Ping(n+1)"
                                                        assert!(peer.sender.try_send_movable(format!("Ping({})", current_count+1)), "couldn't send");
                                                    })
                                                }
        ).await.expect("Starting the client");

        println!("### Measuring latency for {TEST_DURATION_MS} milliseconds...");
        tokio::time::sleep(Duration::from_millis(TEST_DURATION_MS)).await;
        server_shutdown_sender.send(500).expect("sending server shutdown signal");
        client_shutdown_sender.send(500).expect("sending client shutdown signal");

        println!("### Waiting a little for the shutdown signal to reach the server...");
        tokio::time::sleep(Duration::from_millis(10)).await;

        let counter = counter.load(Relaxed);
        let latency = Duration::from_nanos((TEST_DURATION_NS as f64 / (2.0 * counter as f64)) as u64);
        println!("Round trips counter: {}", counter);
        println!("Measured latency: {:?} ({})", latency, if DEBUG {"Debug mode"} else {"Release mode"});
        if DEBUG {
            assert!(counter > 30000, "Latency regression detected: we used to make 36690 round trips in 2 seconds (Debug mode) -- now only {} were made", counter);
        } else {
            assert!(counter > 200000, "Latency regression detected: we used to make 276385 round trips in 2 seconds (Release mode) -- now only {} were made", counter);

        }
    }

    /// When a client floods the server with messages, it should, at most, screw just that client up... or, maybe, not even that!\
    /// This test works like the latency test, but we don't wait for the answer to come to send another one -- we just keep sending like crazy\
    #[cfg_attr(not(doc),tokio::test(flavor = "multi_thread"))]
    #[ignore]   // convention for this project: ignored tests are to be run by a single thread
    async fn message_flooding_throughput() {
        const TEST_DURATION_MS:    u64  = 2000;
        const LISTENING_INTERFACE: &str = "127.0.0.1";
        const PORT               : u16  = 8572;
        let (server_shutdown_sender, server_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (client_shutdown_sender, client_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();

        // server -- do not answer to (flood) messages (just parses & counts them, making sure they are received in the right order)
        // message format is "DoNotAnswer(n)", where n should be sent by the client in natural order, starting from 0
        let received_messages_count = Arc::new(AtomicU32::new(0));
        let unordered = Arc::new(AtomicU32::new(0));    // if non-zero, will contain the last message received before the ordering went kaputt
        let received_messages_count_ref = Arc::clone(&received_messages_count);
        let unordered_ref = Arc::clone(&unordered);
        server_loop_for_unresponsive_text_protocol::<1024, _, _, _, _, _, _, _>
                                                    (LISTENING_INTERFACE.to_string(), PORT, server_shutdown_receiver,
                                                      |_connection_event: ConnectionEvent<1024, String>| async {},
                                                      move |_listening_interface, _listening_port, _peer, client_messages: ProcessorRemoteStreamType<1024, String>| {
                                                          let received_messages_count = Arc::clone(&received_messages_count_ref);
                                                          let unordered = Arc::clone(&unordered_ref);
                                                          client_messages.inspect(move |client_message| {
                                                              // Message format: DoNotAnswer(n)
                                                              let n_str = &client_message[12..(client_message.len()-1)];
                                                              let n = str::parse::<u32>(n_str).unwrap_or_else(|_| panic!("could not convert '{}' to number. Original message: '{}'", n_str, client_message));
                                                              let count = received_messages_count.fetch_add(1, Relaxed);
                                                              if count != n && unordered.compare_exchange(0, count, Relaxed, Relaxed).is_ok() {
                                                                  println!("Server: ERROR: received order of messages broke at message #{}", count);
                                                              }
                                                          })
                                                      }
        ).await.expect("Starting the server");

        println!("### Waiting a little for the server to start...");
        tokio::time::sleep(Duration::from_millis(10)).await;

        // client
        let sent_messages_count = Arc::new(AtomicU32::new(0));
        let sent_messages_count_ref = Arc::clone(&sent_messages_count);
        client_for_unresponsive_text_protocol::<1024, _, _, _, _, _, _, _>
                                               (LISTENING_INTERFACE.to_string(), PORT,
                                                client_shutdown_receiver,
                                                move |connection_event| {
                                                    let sent_messages_count = Arc::clone(&sent_messages_count_ref);
                                                    match connection_event {
                                                        ConnectionEvent::PeerConnected { peer } => {
                                                            tokio::spawn(async move {
                                                                let start = SystemTime::now();
                                                                let mut n = 0;
                                                                loop {
                                                                    assert!(peer.sender.try_send_movable(format!("DoNotAnswer({})", n)), "couldn't send");
                                                                    n += 1;
                                                                    // flush & bailout check for timeout every 1024 messages
                                                                    if n % (1<<10) == 0 {
                                                                        peer.sender.flush(Duration::from_millis(50)).await;
                                                                        if start.elapsed().unwrap().as_millis() as u64 >= TEST_DURATION_MS  {
                                                                            println!("Client sent {} messages before bailing out", n);
                                                                            sent_messages_count.store(n, Relaxed);
                                                                            break;
                                                                        }
                                                                    }
                                                                }
                                                            });
                                                        },
                                                        ConnectionEvent::PeerDisconnected { .. } => {},
                                                        ConnectionEvent::ApplicationShutdown { .. } => {},
                                                    }
                                                    future::ready(())
                                                },
                                                move |_listening_interface, _listening_port, _peer, server_messages: ProcessorRemoteStreamType<1024, String>| {
                                                    server_messages
                                                }
        ).await.expect("Starting the client");
        println!("### Measuring latency for 2 seconds...");

        tokio::time::sleep(Duration::from_millis(TEST_DURATION_MS)).await;
        server_shutdown_sender.send(500).expect("sending server shutdown signal");
        client_shutdown_sender.send(500).expect("sending client shutdown signal");

        println!("### Waiting a little for the shutdown signal to reach the server...");
        tokio::time::sleep(Duration::from_millis(1000)).await;

        println!("### Server saw:");
        let received_messages_count = received_messages_count.load(Relaxed);
        let unordered = unordered.load(Relaxed);
        let sent_messages_count = sent_messages_count.load(Relaxed);
        let received_messages_percent = 100.0 * (received_messages_count as f64 / sent_messages_count as f64);
        println!("    {} received messages {}", received_messages_count, if unordered == 0 {"in order".into()} else {format!("unordered -- ordering broke at message #{}", unordered)});
        println!("    {:.2}% of sent ones", received_messages_percent);

        assert_eq!(unordered, 0, "Server should have received messages in order, but it was broken at message #{} -- total received was {}", unordered, received_messages_count);
        assert!(received_messages_percent >= 99.99, "Client flooding regression detected: the server used to receive 100% of the sent messages -- now only {:.2}% made it through", received_messages_percent);
        if DEBUG {
            assert!(received_messages_count > 400000, "Client flooding throughput regression detected: we used to send/receive 451584 flood messages in this test (Debug mode) -- now only {} were made", received_messages_count);
        } else {
            assert!(received_messages_count > 400000, "Client flooding throughput regression detected: we used to send/receive 500736 flood messages in this test (Release mode) -- now only {} were made", received_messages_count);

        }

    }

    /// assures connection & dialogs work for either the server and client, using the "responsive" flavours of the `Stream` processors
    #[cfg_attr(not(doc),tokio::test)]
    async fn responsive_dialogs() {
        const LISTENING_INTERFACE: &str = "127.0.0.1";
        const PORT               : u16  = 8573;
        let client_secret = String::from("open, sesame");
        let server_secret = String::from("now the 40 of you may enter");
        let observed_secret = Arc::new(Mutex::new(None));

        let (server_shutdown_sender, server_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (_client_shutdown_sender, client_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();

        // server
        let client_secret_ref = client_secret.clone();
        let server_secret_ref = server_secret.clone();
        server_loop_for_responsive_text_protocol::<1024, _, _, _, _, _, _>(LISTENING_INTERFACE.to_string(), PORT, server_shutdown_receiver,
            |_connection_event| future::ready(()),
            move |_client_addr, _client_port, peer, client_messages_stream| {
               let client_secret_ref = client_secret_ref.clone();
               let server_secret_ref = server_secret_ref.clone();
                assert!(peer.sender.try_send_movable(String::from("Welcome! State your business!")), "couldn't send");
                client_messages_stream.flat_map(move |client_message: SocketProcessorDerivedType<1024, String>| {
                   stream::iter([
                       format!("Client just sent '{}'", client_message),
                       if *client_message == client_secret_ref {
                           server_secret_ref.clone()
                       } else {
                           panic!("Client sent the wrong secret: '{}' -- I was expecting '{}'", client_message, client_secret_ref);
                       }])
               })
            }
        ).await.expect("Starting the server");

        println!("### Waiting a little for the server to start...");
        tokio::time::sleep(Duration::from_millis(10)).await;

        // client
        let client_secret = client_secret.clone();
        let observed_secret_ref = Arc::clone(&observed_secret);
        client_for_responsive_text_protocol::<1024, _, _, _, _, _, _>(LISTENING_INTERFACE.to_string(), PORT, client_shutdown_receiver,
            move |_connection_event| future::ready(()),
            move |_client_addr, _client_port, peer, server_messages_stream: ProcessorRemoteStreamType<1024, String>| {
                let observed_secret_ref = Arc::clone(&observed_secret_ref);
                assert!(peer.sender.try_send_movable(client_secret.clone()), "couldn't send");
                server_messages_stream
                    .then(move |server_message| {
                        let observed_secret_ref = Arc::clone(&observed_secret_ref);
                        async move {
                            println!("Server said: '{}'", server_message);
                            let _ = observed_secret_ref.lock().await.insert(server_message);
                        }
                    })
                    .map(|_server_message| ".".to_string())
            }
        ).await.expect("Starting the client");
        println!("### Started a client -- which is running concurrently, in the background... it has 100ms to do its thing!");

        tokio::time::sleep(Duration::from_millis(100)).await;
        server_shutdown_sender.send(500).expect("sending server shutdown signal");

        println!("### Waiting a little for the shutdown signal to reach the server...");
        tokio::time::sleep(Duration::from_millis(10)).await;

        let locked_observed_secret = observed_secret.lock().await;
        let observed_secret = locked_observed_secret.as_ref().expect("Server secret has not been computed");
        assert_eq!(*observed_secret, server_secret, "Communications didn't go according the plan");
    }

    /// assures that shutting down the client causes the connection to be dropped, as perceived by the server
    #[cfg_attr(not(doc),tokio::test)]
    async fn client_shutdown() {
        const LISTENING_INTERFACE: &str = "127.0.0.1";
        const PORT               : u16  = 8574;
        let server_disconnected = Arc::new(AtomicBool::new(false));
        let server_disconnected_ref = Arc::clone(&server_disconnected);
        let client_disconnected = Arc::new(AtomicBool::new(false));
        let client_disconnected_ref = Arc::clone(&client_disconnected);

        let (server_shutdown_sender, server_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (client_shutdown_sender, client_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();

        server_loop_for_unresponsive_text_protocol::<1024, _, _, _, _, _, _, _>(LISTENING_INTERFACE.to_string(), PORT, server_shutdown_receiver,
            move |connection_event| {
                let server_disconnected = Arc::clone(&server_disconnected_ref);
                async move {
                    if let ConnectionEvent::PeerDisconnected { .. } = connection_event {
                        server_disconnected.store(true, Relaxed);
                    }
                }
            },
            move |_client_addr, _client_port, _peer: Arc<Peer<1024, String>>, client_messages_stream: ProcessorRemoteStreamType<1024, String>| client_messages_stream
        ).await.expect("Starting the server");

        // wait a bit for the server to start
        tokio::time::sleep(Duration::from_millis(10)).await;

        client_for_unresponsive_text_protocol::<1024, _, _, _, _, _, _, _>(LISTENING_INTERFACE.to_string(), PORT, client_shutdown_receiver,
            move |connection_event| {
                let client_disconnected = Arc::clone(&client_disconnected_ref);
                async move {
                    if let ConnectionEvent::PeerDisconnected { .. } = connection_event {
                        client_disconnected.store(true, Relaxed);
                    }
                }
            },
            move |_client_addr, _client_port, _peer: Arc<Peer<1024, String>>, server_messages_stream: ProcessorRemoteStreamType<1024, String>| server_messages_stream
        ).await.expect("Starting the client");

        // wait a bit for the connection, shutdown the client, wait another bit
        tokio::time::sleep(Duration::from_millis(100)).await;
        client_shutdown_sender.send(50).expect("sending client shutdown signal");
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(client_disconnected.load(Relaxed), "Client didn't drop the connection with the server");
        assert!(server_disconnected.load(Relaxed), "Server didn't notice the drop in the connection made by the client");

        // unneeded, but avoids error logs on the oneshot channel being dropped
        server_shutdown_sender.send(50).expect("sending server shutdown signal");
    }
*/
    /// Test implementation for our text-only protocol
    impl ReactiveMessagingSerializer<String> for String {
        #[inline(always)]
        fn serialize(message: &String, buffer: &mut Vec<u8>) {
            buffer.clear();
            buffer.extend_from_slice(message.as_bytes());
        }
        #[inline(always)]
        fn processor_error_message(err: String) -> String {
            let msg = format!("ServerBug! Please, fix! Error: {}", err);
            panic!("SocketServerSerializer<String>::processor_error_message(): {}", msg);
            // msg
        }
    }

    /// Our test text-only protocol's messages may also be used by "Responsive Processors"
    impl ResponsiveMessages<String> for String {
        #[inline(always)]
        fn is_disconnect_message(processor_answer: &String) -> bool {
            // for String communications, an empty line sent by the messages processor signals that the connection should be closed
            processor_answer.is_empty()
        }
        #[inline(always)]
        fn is_no_answer_message(processor_answer: &String) -> bool {
            processor_answer == "."
        }
    }

    /// Testable implementation for our text-only protocol
    impl ReactiveMessagingDeserializer<String> for String {
        #[inline(always)]
        fn deserialize(message: &[u8]) -> Result<String, Box<dyn std::error::Error + Sync + Send + 'static>> {
            Ok(String::from_utf8_lossy(message).to_string())
        }
    }

}