//! Common & low-level code for reactive clients and servers


use crate::serde::ReactiveMessagingSerializer;

use crate::{
    config::*,
    types::*,
    serde::ReactiveMessagingDeserializer,
    socket_connection::{
        common::{ReactiveMessagingSender, ReactiveMessagingUniSender, upgrade_processor_uni_retrying_logic},
        peer::Peer,
    },
};
use std::{
    error::Error,
    fmt::Debug,
    future::Future,
    net::SocketAddr,
    sync::Arc,
    time::Duration,
    marker::PhantomData,
};
use reactive_mutiny::prelude::advanced::{GenericUni, FullDuplexUniChannel};
use futures::{StreamExt, Stream};
use tokio::io::{self,AsyncReadExt,AsyncWriteExt};
use log::{trace, debug, warn, error};
use crate::socket_connection::connection::SocketConnection;


/// Contains abstractions, useful for clients and servers, for dealing with socket connections handled by Stream Processors:\
///   - handles all combinations of the Stream's output types returned by `dialog_processor_builder_fn()`: futures/non-future & fallible/non-fallible;
///   - handles unresponsive / responsive output types (also issued by the Streams created by `dialog_processor_builder_fn()`).
pub struct SocketConnectionHandler<const CONFIG:        u64,
                                   RemoteMessagesType:  ReactiveMessagingDeserializer<RemoteMessagesType>                                   + Send + Sync + PartialEq + Debug + 'static,
                                   LocalMessagesType:   ReactiveMessagingSerializer<LocalMessagesType>                                      + Send + Sync + PartialEq + Debug + 'static,
                                   ProcessorUniType:    GenericUni<ItemType=RemoteMessagesType>                                             + Send + Sync                     + 'static,
                                   SenderChannel:       FullDuplexUniChannel<ItemType=LocalMessagesType, DerivedItemType=LocalMessagesType> + Send + Sync                     + 'static,
                                   StateType:                                                                                                 Send + Sync + Clone     + Debug + 'static = ()> {
    _phantom:   PhantomData<(RemoteMessagesType, LocalMessagesType, ProcessorUniType, SenderChannel, StateType)>,
}

impl<const CONFIG:        u64,
     RemoteMessagesType:  ReactiveMessagingDeserializer<RemoteMessagesType>                                   + Send + Sync + PartialEq + Debug + 'static,
     LocalMessagesType:   ReactiveMessagingSerializer<LocalMessagesType>                                      + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:    GenericUni<ItemType=RemoteMessagesType>                                             + Send + Sync                     + 'static,
     SenderChannel:       FullDuplexUniChannel<ItemType=LocalMessagesType, DerivedItemType=LocalMessagesType> + Send + Sync                     + 'static,
     StateType:                                                                                                 Send + Sync + Clone     + Debug + 'static>
 SocketConnectionHandler<CONFIG, RemoteMessagesType, LocalMessagesType, ProcessorUniType, SenderChannel, StateType> {

    const CONST_CONFIG: ConstConfig = ConstConfig::from(CONFIG);


    pub fn new() -> Self {
        Self {
            _phantom: PhantomData,
        }
    }

    /// Serves textual protocols whose events/commands/sentences are separated by '\n':
    ///   - `dialog_processor_builder_fn()` builds streams that will receive `ClientMessages` but will generate no answers;
    ///   - sending messages back to the client will be done explicitly by `peer`;
    ///   - `shutdown_signaler` comes from a `tokio::sync::oneshot::channel`, which receives the maximum time, in
    ///     milliseconds: it will be passed to `connection_events_callback` and cause the server loop to exit.
    #[inline(always)]
    pub async fn server_loop_for_unresponsive_text_protocol<PipelineOutputType:                                                                                                                                                                                                                                                     Send + Sync + Debug + 'static,
                                                            OutputStreamType:               Stream<Item=PipelineOutputType>                                                                                                                                                                                                       + Send                + 'static,
                                                            ConnectionEventsCallbackFuture: Future<Output=()>                                                                                                                                                                                                                     + Send,
                                                            ConnectionEventsCallback:       Fn(/*server_event: */SingleProtocolEvent<CONFIG, LocalMessagesType, SenderChannel, StateType>)                                                                                                          -> ConnectionEventsCallbackFuture + Send + Sync         + 'static,
                                                            ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<CONFIG, LocalMessagesType, SenderChannel, StateType>>, /*client_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> OutputStreamType               + Send + Sync         + 'static>

                                                           (self,
                                                            listening_interface:         &str,
                                                            listening_port:              u16,
                                                            mut connection_source:       tokio::sync::mpsc::Receiver<SocketConnection<StateType>>,
                                                            connection_sink:             tokio::sync::mpsc::Sender<SocketConnection<StateType>>,
                                                            connection_events_callback:  ConnectionEventsCallback,
                                                            dialog_processor_builder_fn: ProcessorBuilderFn)

                                                            -> Result<(), Box<dyn Error + Sync + Send>> {

        let arc_self = Arc::new(self);
        let connection_events_callback = Arc::new(connection_events_callback);
        let connection_sink = Arc::new(connection_sink);
        let listening_interface_and_port = format!("{}:{}", listening_interface, listening_port);

        tokio::spawn( async move {

            while let Some(socket_connection) = connection_source.recv().await {

                // client info
                let addr = match socket_connection.connection().peer_addr() {
                    Ok(addr) => addr,
                    Err(err) => {
                        error!("`reactive-messaging::SocketServer`: ERROR in server @ {listening_interface_and_port} while determining an incoming client's address: {err}");
                        continue;
                    },
                };
                let (client_ip, client_port) = match addr {
                    SocketAddr::V4(v4) => (v4.ip().to_string(), v4.port()),
                    SocketAddr::V6(v6) => (v6.ip().to_string(), v6.port()),
                };

                // prepares for the dialog to come with the (just accepted) connection
                let sender = ReactiveMessagingSender::<CONFIG, LocalMessagesType, SenderChannel>::new(format!("Sender for client {addr}"));
                let peer = Arc::new(Peer::new(sender, addr, &socket_connection));//       THIS LINE IS HANGING!!
                let peer_ref1 = Arc::clone(&peer);
                let peer_ref2 = Arc::clone(&peer);

                // issue the connection event
                connection_events_callback(SingleProtocolEvent::PeerConnected {peer: peer.clone()}).await;

                let connection_events_callback_ref = Arc::clone(&connection_events_callback);
                let processor_sender = ProcessorUniType::new(format!("Server processor for remote client {addr} @ {listening_interface_and_port}"))
                    .spawn_non_futures_non_fallibles_executors(1,
                                                               |in_stream| dialog_processor_builder_fn(client_ip.clone(), client_port, peer_ref1.clone(), in_stream),
                                                               move |executor| async move {
                                                                   // issue the async disconnect event
                                                                   connection_events_callback_ref(SingleProtocolEvent::PeerDisconnected { peer: peer_ref2, stream_stats: executor }).await;
                                                               });
                let processor_sender = upgrade_processor_uni_retrying_logic::<CONFIG, RemoteMessagesType, ProcessorUniType::DerivedItemType, ProcessorUniType>
                    (processor_sender);
                // spawn a task to handle communications with that client
                let cloned_self = Arc::clone(&arc_self);
                let listening_interface_and_port = listening_interface_and_port.clone();
                let cloned_connection_sink = Arc::clone(&connection_sink);
                tokio::spawn(tokio::task::unconstrained(async move {
                    let socket_connection = match cloned_self.dialog_loop_for_textual_protocol(socket_connection, peer.clone(), processor_sender).await {
                        Ok(socket_connection) => socket_connection,
                        Err(err) => {
                            error!("`reactive-messaging::SocketServer`: ERROR in server @ {listening_interface_and_port} while starting the dialog with client {client_ip}:{client_port}: {err}");
                            return
                        }
                    };
                    if let Err(_) = cloned_connection_sink.send(socket_connection).await {
                        error!("`reactive-messaging::SocketServer`: ERROR in server @ {listening_interface_and_port} while returning the connection with {client_ip}:{client_port} to the caller. It will now be forcibly closed.");
                    };
                    // if let Err(err) = connection.shutdown().await {
                    //     error!("`reactive-messaging::SocketServer`: ERROR in server @ {listening_interface_and_port} while shutting down the socket with client {client_ip}:{client_port}: {err}");
                    // }
                }));
            }
            debug!("`reactive-messaging::SocketServer`: bailing out of network loop -- we should be undergoing a shutdown...");
            // issue the shutdown event
            connection_events_callback(SingleProtocolEvent::LocalServiceTermination).await;
        });

        Ok(())
    }

    /// Executes a client for textual protocols whose events/commands/sentences are separated by '\n':
    ///   - `dialog_processor_builder_fn()` builds streams that will receive `ServerMessages` but will generate no answers;
    ///   - sending messages back to the server will be done explicitly by `peer`;
    ///   - `shutdown_signaler` comes from a `tokio::sync::oneshot::channel`, which receives the maximum time, in
    ///     milliseconds: it will be passed to `connection_events_callback` and cause the server loop to exit.
    // TODO 2024-01-03: make this able to process the same connection as many times as needed, for symmetry with the server -- practically, allowing connection reuse
    #[inline(always)]
    pub async fn client_for_unresponsive_text_protocol<PipelineOutputType:                                                                                                                                                                                                                                                     Send + Sync + Debug + 'static,
                                                       OutputStreamType:               Stream<Item=PipelineOutputType>                                                                                                                                                                                                       + Send                + 'static,
                                                       ConnectionEventsCallbackFuture: Future<Output=()>                                                                                                                                                                                                                     + Send,
                                                       ConnectionEventsCallback:       Fn(/*server_event: */SingleProtocolEvent<CONFIG, LocalMessagesType, SenderChannel, StateType>)                                                                                                          -> ConnectionEventsCallbackFuture + Send + Sync         + 'static,
                                                       ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<CONFIG, LocalMessagesType, SenderChannel, StateType>>, /*server_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> OutputStreamType>

                                                      (self,
                                                       socket_connection:           SocketConnection<StateType>,
                                                       mut shutdown_signaler:       tokio::sync::broadcast::Receiver<()>,
                                                       connection_events_callback:  ConnectionEventsCallback,
                                                       dialog_processor_builder_fn: ProcessorBuilderFn)

                                                      -> Result<SocketConnection<StateType>, Box<dyn Error + Sync + Send>> {

        let addr = socket_connection.connection().peer_addr()?;
        let sender = ReactiveMessagingSender::<CONFIG, LocalMessagesType, SenderChannel>::new(format!("Sender for client {addr}"));
        let peer = Arc::new(Peer::new(sender, addr, &socket_connection));
        let peer_ref1 = Arc::clone(&peer);
        let peer_ref2 = Arc::clone(&peer);
        let peer_ref3 = Arc::clone(&peer);

        // issue the connection event
        connection_events_callback(SingleProtocolEvent::PeerConnected {peer: peer.clone()}).await;

        let connection_events_callback_ref1 = Arc::new(connection_events_callback);
        let connection_events_callback_ref2 = Arc::clone(&connection_events_callback_ref1);
        let processor_sender = ProcessorUniType::new(format!("Client Processor for remote server @ {addr}"))
            .spawn_non_futures_non_fallibles_executors(1,
                                                    |in_stream| dialog_processor_builder_fn(addr.ip().to_string(), addr.port(), peer_ref1.clone(), in_stream),
                                                    move |executor| async move {
                                                        // issue the async disconnect event
                                                        connection_events_callback_ref1(SingleProtocolEvent::PeerDisconnected { peer: peer_ref2, stream_stats: executor }).await;
                                                    });
        let processor_sender = upgrade_processor_uni_retrying_logic::<CONFIG, RemoteMessagesType, ProcessorUniType::DerivedItemType, ProcessorUniType>
                                                                                               (processor_sender);

        // spawn the shutdown listener
        let addr = addr.to_string();
        tokio::spawn(async move {
            match shutdown_signaler.recv().await {
                Ok(())             => trace!("reactive-messaging: SHUTDOWN requested for client connected to server @ {addr} -- notifying & dropping the connection"),
                Err(err) => error!("reactive-messaging: PROBLEM in the `shutdown signaler` client connected to server @ {addr} (a client shutdown will be commanded now due to this occurrence): {err:?}"),
            };
            // issue the shutdown event
            connection_events_callback_ref2(SingleProtocolEvent::LocalServiceTermination).await;
            // close the connection
            peer_ref3.flush_and_close(Duration::from_millis(100)).await;
        });

        // the processor
        let arc_self = Arc::new(self);
        arc_self.dialog_loop_for_textual_protocol(socket_connection, peer.clone(), processor_sender).await
    }

    /// Handles the "local" side of the peers dialog that is to take place once the connection is established -- provided the communications are done through a textual chat
    /// in which each event/command/sentence ends in '\n'.\
    /// The protocol is determined by `LocalMessages` & `RemoteMessages`, where and each one may either be the client messages or server messages, depending on who is the "local peer":
    ///   - `peer` represents the remote end of the connection;
    ///   - `processor_sender` is a [reactive_mutiny::Uni], to which incoming messages will be sent;
    ///   - conversely, `peer.sender` is the [reactive_mutiny::Uni] to receive outgoing messages.
    ///   - after the processor is done with the `textual_socket`, the method returns it back to the caller if no errors had happened.
    #[inline(always)]
    async fn dialog_loop_for_textual_protocol(self: Arc<Self>,
                                              mut socket_connection: SocketConnection<StateType>,
                                              peer:                  Arc<Peer<CONFIG, LocalMessagesType, SenderChannel, StateType>>,
                                              processor_sender:      ReactiveMessagingUniSender<CONFIG, RemoteMessagesType, ProcessorUniType::DerivedItemType, ProcessorUniType>)

                                             -> Result<SocketConnection<StateType>, Box<dyn Error + Sync + Send>> {
        // socket
        if let Some(no_delay) = Self::CONST_CONFIG.socket_options.no_delay {
            socket_connection.connection_mut().set_nodelay(no_delay).map_err(|err| format!("error setting nodelay({no_delay}) for the socket connected at {}:{}: {err}", peer.peer_address, peer.peer_id))?;
        }
        if let Some(hops_to_live) = Self::CONST_CONFIG.socket_options.hops_to_live {
            let hops_to_live = hops_to_live.get() as u32;
            socket_connection.connection_mut().set_ttl(hops_to_live).map_err(|err| format!("error setting ttl({hops_to_live}) for the socket connected at {}:{}: {err}", peer.peer_address, peer.peer_id))?;
        }
        if let Some(millis) = Self::CONST_CONFIG.socket_options.linger_millis {
            socket_connection.connection_mut().set_linger(Some(Duration::from_millis(millis as u64))).map_err(|err| format!("error setting linger({millis}ms) for the socket connected at {}:{}: {err}", peer.peer_address, peer.peer_id))?;
        }
        // buffers
        let mut read_buffer = Vec::with_capacity(Self::CONST_CONFIG.msg_size_hint as usize);
        let mut serialization_buffer = Vec::with_capacity(Self::CONST_CONFIG.msg_size_hint as usize);

        let (mut sender_stream, _) = peer.create_stream();

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
                            if let Err(err) = socket_connection.connection_mut().write_all(&serialization_buffer).await {
                                warn!("`dialog_loop_for_textual_protocol`: PROBLEM in the connection with {peer:#?} while WRITING '{to_send:?}': {err:?}");
                                break 'connection
                            }
                        },
                        None => {
                            debug!("`dialog_loop_for_textual_protocol`: Sender for {peer:#?} ended (most likely, either `peer.flush_and_close()` or `peer.cancel_and_close()` was called on the `peer` by the unresponsive processor OR a disconnection message was previously answered by the responsive processor)");
                            break 'connection
                        }
                    }
                },

                // receive?
                read = socket_connection.connection_mut().read_buf(&mut read_buffer) => {
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
                                            if let Err((abort_processor, error_msg_processor)) = processor_sender.send(client_message).await {
                                                // log & send the error message to the remote peer
                                                error!("`dialog_loop_for_textual_protocol`: {} -- `dialog_processor` is full of unprocessed messages ({}/{})", error_msg_processor, processor_sender.pending_items_count(), processor_sender.buffer_size());
                                                if let Err((abort_sender, error_msg_sender)) = peer.send_async(LocalMessagesType::processor_error_message(error_msg_processor)).await {
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
                                            warn!("`dialog_loop_for_textual_protocol`:  {error_message}");
                                            let outgoing_error = LocalMessagesType::processor_error_message(error_message);
                                            if let Err((abort, error_msg)) = peer.send_async(outgoing_error).await {
                                                if abort {
                                                    warn!("`dialog_loop_for_textual_protocol`:  {error_msg} -- Slow reader {:?}", peer);
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
                        Ok(_) /* zero bytes received -- the other end probably closed the connection */ => {
                            debug!("`dialog_loop_for_textual_protocol`: PROBLEM with reading from {:?} (peer id {}) -- it is out of bytes! Dropping the connection", peer.peer_address, peer.peer_id);
                            break 'connection
                        },
                        Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {},
                        Err(err) => {
                            error!("`dialog_loop_for_textual_protocol`: ERROR in the connection with {:?} (peer id {}) while READING: '{:?}' -- dropping it", peer.peer_address, peer.peer_id, err);
                            break 'connection
                        },
                    }
                },
            );

        }
        _ = processor_sender.close(Duration::from_millis(Self::CONST_CONFIG.graceful_close_timeout_millis as u64)).await;
        peer.cancel_and_close();
        socket_connection.connection_mut().flush().await.map_err(|err| format!("error flushing the textual socket connected to {}:{}: {}", peer.peer_address, peer.peer_id, err))?;
        peer.take_state().await
            .map(|state| socket_connection.set_state(state));
        Ok(socket_connection)
    }

    /// Similar to [UnresponsiveSocketConnectionHandle::server_loop_for_unresponsive_text_protocol()], but for when the provided `dialog_processor_builder_fn()` produces
    /// answers of type `ServerMessages`, which will be automatically routed to the clients.
    #[inline(always)]
    pub async fn server_loop_for_responsive_text_protocol<OutputStreamType:               Stream<Item=LocalMessagesType>                                                                                                                                                                                                        + Send        + 'static,
                                                          ConnectionEventsCallbackFuture: Future<Output=()>                                                                                                                                                                                                                     + Send,
                                                          ConnectionEventsCallback:       Fn(/*server_event: */SingleProtocolEvent<CONFIG, LocalMessagesType, SenderChannel, StateType>)                                                                                                          -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                                          ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<CONFIG, LocalMessagesType, SenderChannel, StateType>>, /*client_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> OutputStreamType               + Send + Sync + 'static>

                                                         (self,
                                                          listening_interface:         &str,
                                                          listening_port:              u16,
                                                          connection_source:           tokio::sync::mpsc::Receiver<SocketConnection<StateType>>,
                                                          connection_sink:             tokio::sync::mpsc::Sender<SocketConnection<StateType>>,
                                                          connection_events_callback:  ConnectionEventsCallback,
                                                          dialog_processor_builder_fn: ProcessorBuilderFn)

                                                         -> Result<(), Box<dyn Error + Sync + Send>>

                                                         where LocalMessagesType: ResponsiveMessages<LocalMessagesType> {

        // upgrade the "unresponsive" streams from `dialog_processor_builder_fn()` to "responsive" streams that will send the outcome to clients
       let dialog_processor_builder_fn = move |client_addr, connected_port, peer, client_messages_stream| {
           let dialog_processor_stream = dialog_processor_builder_fn(client_addr, connected_port, Arc::clone(&peer), client_messages_stream);
           self.to_responsive_stream(peer, dialog_processor_stream)
       };
        let unresponsive_socket_connection_handler = SocketConnectionHandler::<CONFIG, RemoteMessagesType, LocalMessagesType, ProcessorUniType, SenderChannel, StateType>::new();
        unresponsive_socket_connection_handler.server_loop_for_unresponsive_text_protocol(listening_interface, listening_port, connection_source, connection_sink, connection_events_callback, dialog_processor_builder_fn).await
            .map_err(|err| Box::from(format!("error when starting server @ {listening_interface}:{listening_port} with `server_loop_for_unresponsive_text_protocol()`: {err}")))
    }

    /// Similar to [Self::client_for_unresponsive_text_protocol()], but for when the provided `dialog_processor_builder_fn()` produces
    /// answers of type `ClientMessages`, which will be automatically routed to the server.
    // TODO 2024-01-03: make this able to process the same connection as many times as needed, for symmetry with the server -- practically, allowing connection reuse
    #[inline(always)]
    pub async fn client_for_responsive_text_protocol<OutputStreamType:               Stream<Item=LocalMessagesType>                                                                                                                                                                                                        + Send +        'static,
                                                     ConnectionEventsCallbackFuture: Future<Output=()>                                                                                                                                                                                                                     + Send,
                                                     ConnectionEventsCallback:       Fn(/*server_event: */SingleProtocolEvent<CONFIG, LocalMessagesType, SenderChannel, StateType>)                                                                                                          -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                                     ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<CONFIG, LocalMessagesType, SenderChannel, StateType>>, /*server_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> OutputStreamType>

                                                    (self,
                                                     socket_connection:           SocketConnection<StateType>,
                                                     shutdown_signaler:           tokio::sync::broadcast::Receiver<()>,
                                                     connection_events_callback:  ConnectionEventsCallback,
                                                     dialog_processor_builder_fn: ProcessorBuilderFn)

                                                    -> Result<SocketConnection<StateType>, Box<dyn Error + Sync + Send>>

                                                    where LocalMessagesType: ResponsiveMessages<LocalMessagesType> {

        let addr = socket_connection.connection().peer_addr()?;
        // upgrade the "unresponsive" streams from `dialog_processor_builder_fn()` to "responsive" streams that will send the outcome to clients
        let dialog_processor_builder_fn = |server_addr, port, peer, server_messages_stream| {
            let dialog_processor_stream = dialog_processor_builder_fn(server_addr, port, Arc::clone(&peer), server_messages_stream);
            self.to_responsive_stream(peer, dialog_processor_stream)
        };
        let unresponsive_socket_connection_handler = SocketConnectionHandler::<CONFIG, RemoteMessagesType, LocalMessagesType, ProcessorUniType, SenderChannel, StateType>::new();
        unresponsive_socket_connection_handler.client_for_unresponsive_text_protocol(socket_connection, shutdown_signaler, connection_events_callback, dialog_processor_builder_fn).await
            .map_err(|err| Box::from(format!("error when starting client for server @ {addr} with `client_for_unresponsive_text_protocol()`: {err}")))
    }

    /// upgrades the `request_processor_stream` (of non-fallible & non-future items) to a `Stream` which is also able to send answers back to the `peer`
    #[inline(always)]
    fn to_responsive_stream(&self,
                            peer:                     Arc<Peer<CONFIG, LocalMessagesType, SenderChannel, StateType>>,
                            request_processor_stream: impl Stream<Item=LocalMessagesType>)

                           -> impl Stream<Item = ()>

                           where LocalMessagesType: ResponsiveMessages<LocalMessagesType> {

        request_processor_stream
            .map(move |outgoing| {
                // wired `if`s ahead to try to get some branch prediction optimizations -- assuming Rust will see the first `if` branches as the "usual case"
                let is_disconnect = LocalMessagesType::is_disconnect_message(&outgoing);
                let is_no_answer = LocalMessagesType::is_no_answer_message(&outgoing);
                // send the message, skipping messages that are programmed not to generate any response
                if !is_disconnect && !is_no_answer {
                    // send the answer
                    trace!("`to_responsive_stream()`: Sending Answer `{:?}` to {:?} (peer id {})", outgoing, peer.peer_address, peer.peer_id);
                    if let Err((abort, error_msg)) = peer.send(outgoing) {
                        // peer is slow-reading -- and, possibly, fast sending
                        warn!("`to_responsive_stream()`: Slow reader detected while sending to {:?}: {error_msg}", peer);
                        if abort {
                            peer.cancel_and_close();
                        }
                    }
                } else if is_disconnect {
                    debug!("`to_responsive_stream()`: Processor choose to drop connection with {} (peer id {}) by answering with the DISCONNECT MESSAGE '{:?}'", peer.peer_address, peer.peer_id, outgoing);
                    if !is_no_answer {
                        if let Err((_abort, error_msg)) = peer.send(outgoing) {
                            warn!("reactive-messaging: Slow reader detected while sending the closing message to {:?}: {error_msg}", peer);
                        }
                    }
                    // TODO 2023-08-25: there should be a proper method to call out of async context (in Peer) that will allow us not to forcibly sleep here.
                    std::thread::sleep(Duration::from_millis(100));
                    peer.cancel_and_close();
                }
            })
    }

    /// upgrades the `request_processor_stream` (of fallible & non-future items) to a `Stream` which is also able to send answers back to the `peer`
    #[inline(always)]
    fn _to_responsive_stream_of_fallibles(&self,
                                          peer:                     Arc<Peer<CONFIG, LocalMessagesType, SenderChannel, StateType>>,
                                          request_processor_stream: impl Stream<Item = Result<LocalMessagesType, Box<dyn Error + Sync + Send>>>)

                                         -> impl Stream<Item = ()>

                                         where LocalMessagesType: ResponsiveMessages<LocalMessagesType> {

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
                        LocalMessagesType::processor_error_message(err_string)
                    },
                }
            });
        // process the answer
        self.to_responsive_stream(peer_ref2, request_processor_stream)
    }

}


/// Unit tests the [tokio_message_io](self) module
#[cfg(any(test,doc))]
mod tests {
    use super::*;
    use std::{
        future,
        time::SystemTime,
        sync::atomic::{
            AtomicBool,
            AtomicU32,
            Ordering::Relaxed,
        },
        net::ToSocketAddrs,
    };
    use reactive_mutiny::{prelude::advanced::{UniZeroCopyAtomic, ChannelUniMoveAtomic, ChannelUniZeroCopyAtomic}, types::{ChannelCommon, ChannelUni, ChannelProducer}};
    use futures::stream;
    use tokio::net::TcpStream;
    use tokio::sync::Mutex;
    use crate::socket_connection::connection_provider::ServerConnectionHandler;


    #[cfg(debug_assertions)]
    const DEBUG: bool = true;
    #[cfg(not(debug_assertions))]
    const DEBUG: bool = false;

    const DEFAULT_TEST_CONFIG: ConstConfig         = ConstConfig {
        //retrying_strategy: RetryingStrategies::DoNotRetry,    // uncomment to see `message_flooding_throughput()` fail due to unsent messages
        retrying_strategy: RetryingStrategies::RetryYieldingForUpToMillis(30),
        ..ConstConfig::default()
    };
    const DEFAULT_TEST_CONFIG_U64:  u64            = DEFAULT_TEST_CONFIG.into();
    const DEFAULT_TEST_UNI_INSTRUMENTS: usize      = DEFAULT_TEST_CONFIG.executor_instruments.into();
    type DefaultTestUni<PayloadType = String>      = UniZeroCopyAtomic<PayloadType, {DEFAULT_TEST_CONFIG.receiver_buffer as usize}, 1, DEFAULT_TEST_UNI_INSTRUMENTS>;
    type SenderChannel<PayloadType = String>       = ChannelUniMoveAtomic<PayloadType, {DEFAULT_TEST_CONFIG.sender_buffer as usize}, 1>;


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
        println!("Reconverted:     {:#?}", ConstConfig::from(DEFAULT_TEST_CONFIG_U64));
        assert_eq!(ConstConfig::from(DEFAULT_TEST_CONFIG_U64), DEFAULT_TEST_CONFIG, "Configs don't match");
        // try to create the objects based on the config (a compilation error is expected if wrong const generic parameters are provided to the `Uni` type)
        let uni    = DefaultTestUni::<String>::new("Can it be instantiated?");
        let sender = SenderChannel::<String>::new("Can it be instantiated?");

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

        // Rust checks
        //////////////
        // `Futures` are `Send` -- see compiler bug https://github.com/rust-lang/rust/issues/102211
        let mut connection_provider = ServerConnectionHandler::new("127.0.0.1", 8579, ()).await
            .expect("couldn't start the Connection Provider server event loop");
        let new_connections_source = connection_provider.connection_receiver()
            .expect("couldn't move the Connection Receiver out of the Connection Provider");
        let (returned_connections_sink, _returned_connections_source) = tokio::sync::mpsc::channel::<SocketConnection<()>>(2);
        let socket_communications_handler = SocketConnectionHandler::<DEFAULT_TEST_CONFIG_U64, String, String, DefaultTestUni, SenderChannel>::new();
        socket_communications_handler.server_loop_for_unresponsive_text_protocol(
            "127.0.0.1", 8579, new_connections_source, returned_connections_sink,
            |connection_event| async move {
                match connection_event {
                    SingleProtocolEvent::PeerConnected { .. }       => tokio::time::sleep(Duration::from_millis(100)).await,
                    SingleProtocolEvent::PeerDisconnected { .. }    => tokio::time::sleep(Duration::from_millis(100)).await,
                    SingleProtocolEvent::LocalServiceTermination { .. } => tokio::time::sleep(Duration::from_millis(100)).await,
                }
            },
            move |_client_addr, _client_port, _peer, client_messages_stream|
                client_messages_stream
        ).await.expect("Starting the server");

    }


    /// assures connection & dialogs work for either the server and client, using the "unresponsive" flavours of the `Stream` processors
    #[cfg_attr(not(doc),tokio::test)]
    async fn unresponsive_dialogs() {
        const LISTENING_INTERFACE: &str = "127.0.0.1";
        const PORT               : u16  = 8570;
        let client_secret = String::from("open, sesame");
        let server_secret = String::from("now the 40 of you may enter");
        let observed_secret = Arc::new(Mutex::new(None));

        let (_client_shutdown_sender, client_shutdown_receiver) = tokio::sync::broadcast::channel(1);

        // server
        let client_secret_ref = client_secret.clone();
        let server_secret_ref = server_secret.clone();
        let mut connection_provider = ServerConnectionHandler::new(LISTENING_INTERFACE, PORT, ()).await
            .expect("Sanity Check: couldn't start the Connection Provider server event loop");
        let new_connections_source = connection_provider.connection_receiver()
            .expect("Sanity Check: couldn't move the Connection Receiver out of the Connection Provider");
        let socket_communications_handler = SocketConnectionHandler::<DEFAULT_TEST_CONFIG_U64, String, String, DefaultTestUni, SenderChannel>::new();
        let (returned_connections_sink, mut server_connections_source) = tokio::sync::mpsc::channel::<SocketConnection<()>>(2);
        socket_communications_handler.server_loop_for_unresponsive_text_protocol(
            LISTENING_INTERFACE, PORT, new_connections_source, returned_connections_sink,
            |connection_event| {
                 match connection_event {
                     SingleProtocolEvent::PeerConnected { peer } => {
                         assert!(peer.send(String::from("Welcome! State your business!")).is_ok(), "couldn't send");
                     },
                     SingleProtocolEvent::PeerDisconnected { peer: _, stream_stats: _ } => {},
                     SingleProtocolEvent::LocalServiceTermination => {
                         println!("Test Server: shutdown was requested... No connection will receive the drop message (nor will be even closed) because I, the lib caller, intentionally didn't keep track of the connected peers for this test!");
                     }
                 }
                 future::ready(())
            },
            move |_client_addr, _client_port, peer, client_messages_stream| {
                let client_secret_ref = client_secret_ref.clone();
                let server_secret_ref = server_secret_ref.clone();
                client_messages_stream.inspect(move |client_message| {
                    assert!(peer.send(format!("Client just sent '{}'", client_message)).is_ok(), "couldn't send");
                    if *client_message == client_secret_ref {
                        assert!(peer.send(server_secret_ref.clone()).is_ok(), "couldn't send");
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
        let tokio_connection = TcpStream::connect(format!("{}:{}", LISTENING_INTERFACE, PORT).to_socket_addrs().expect("Error resolving address").into_iter().next().unwrap()).await.expect("Error connecting");
        let socket_connection = SocketConnection::new(tokio_connection, ());
        let client_communications_handler = SocketConnectionHandler::<DEFAULT_TEST_CONFIG_U64, String, String, DefaultTestUni, SenderChannel>::new();
        tokio::spawn(
            client_communications_handler.client_for_unresponsive_text_protocol(
                socket_connection, client_shutdown_receiver,
                move |connection_event| {
                    match connection_event {
                        SingleProtocolEvent::PeerConnected { peer } => {
                            assert!(peer.send(client_secret.clone()).is_ok(), "couldn't send");
                        },
                        SingleProtocolEvent::PeerDisconnected { peer, stream_stats: _ } => {
                            println!("Test Client: connection with {} (peer_id #{}) was dropped -- should not happen in this test", peer.peer_address, peer.peer_id);
                        },
                        SingleProtocolEvent::LocalServiceTermination => {}
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
            )
        );
        println!("### Started a client -- which is running concurrently, in the background... it has 100ms to do its thing!");

        tokio::time::sleep(Duration::from_millis(100)).await;
        server_connections_source.close();  // terminates the server

        println!("### Waiting a little for the shutdown signal to reach the server...");
        tokio::time::sleep(Duration::from_millis(10)).await;

        let locked_observed_secret = observed_secret.lock().await;
        let observed_secret = locked_observed_secret.as_ref().expect("Server secret has not been computed");
        assert_eq!(*observed_secret, server_secret, "Communications didn't go according the plan");
    }

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

        let (client_shutdown_sender, client_shutdown_receiver) = tokio::sync::broadcast::channel(1);

        // server
        let mut connection_provider = ServerConnectionHandler::new(LISTENING_INTERFACE, PORT, ()).await
            .expect("couldn't start the Connection Provider server event loop");
        let new_connections_source = connection_provider.connection_receiver()
            .expect("couldn't move the Connection Receiver out of the Connection Provider");
        let (returned_connections_sink, mut server_connections_source) = tokio::sync::mpsc::channel::<SocketConnection<()>>(2);
        let socket_communications_handler = SocketConnectionHandler::<DEFAULT_TEST_CONFIG_U64, String, String, DefaultTestUni, SenderChannel>::new();
        socket_communications_handler.server_loop_for_unresponsive_text_protocol(
            LISTENING_INTERFACE, PORT, new_connections_source, returned_connections_sink,
            |_connection_event| async {},
            |_listening_interface, _listening_port, peer, client_messages| {
                 client_messages.inspect(move |client_message| {
                     // Receives: "Ping(n)"
                     let n_str = &client_message[5..(client_message.len() - 1)];
                     let n = str::parse::<u32>(n_str).unwrap_or_else(|err| panic!("could not convert '{}' to number. Original client message: '{}'. Parsing error: {:?}", n_str, client_message, err));
                     // Answers:  "Pong(n)"
                     assert!(peer.send(format!("Pong({})", n)).is_ok(), "couldn't send");
                 })
            }
        ).await.expect("Starting the server");

        println!("### Waiting a little for the server to start...");
        tokio::time::sleep(Duration::from_millis(10)).await;

        let counter = Arc::new(AtomicU32::new(0));
        let counter_ref = Arc::clone(&counter);
        // client
        let tokio_connection = TcpStream::connect(format!("{}:{}", LISTENING_INTERFACE, PORT).to_socket_addrs().expect("Error resolving address").into_iter().next().unwrap()).await.expect("Error connecting");
        let socket_connection = SocketConnection::new(tokio_connection, ());
        let socket_communications_handler = SocketConnectionHandler::<DEFAULT_TEST_CONFIG_U64, String, String, DefaultTestUni, SenderChannel>::new();
        tokio::spawn(
            socket_communications_handler.client_for_unresponsive_text_protocol(
                socket_connection, client_shutdown_receiver,
                |connection_event| {
                    match connection_event {
                        SingleProtocolEvent::PeerConnected { peer } => {
                            // conversation starter
                            assert!(peer.send(String::from("Ping(0)")).is_ok(), "couldn't send");
                        },
                        SingleProtocolEvent::PeerDisconnected { .. } => {},
                        SingleProtocolEvent::LocalServiceTermination { .. } => {},
                    }
                    future::ready(())
                },
                move |_listening_interface, _listening_port, peer, server_messages| {
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
                        assert!(peer.send(format!("Ping({})", current_count+1)).is_ok(), "couldn't send");
                    })
                }
            )
        );

        println!("### Measuring latency for {TEST_DURATION_MS} milliseconds...");
        tokio::time::sleep(Duration::from_millis(TEST_DURATION_MS)).await;
        server_connections_source.close();  // terminates the server
        client_shutdown_sender.send(()).expect("sending client termination signal");

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
        let (client_shutdown_sender, client_shutdown_receiver) = tokio::sync::broadcast::channel(1);

        // server -- do not answer to (flood) messages (just parses & counts them, making sure they are received in the right order)
        // message format is "DoNotAnswer(n)", where n should be sent by the client in natural order, starting from 0
        let received_messages_count = Arc::new(AtomicU32::new(0));
        let unordered = Arc::new(AtomicU32::new(0));    // if non-zero, will contain the last message received before the ordering went kaput
        let received_messages_count_ref = Arc::clone(&received_messages_count);
        let unordered_ref = Arc::clone(&unordered);
        let mut connection_provider = ServerConnectionHandler::new(LISTENING_INTERFACE, PORT, ()).await
            .expect("couldn't start the Connection Provider server event loop");
        let new_connections_source = connection_provider.connection_receiver()
            .expect("couldn't move the Connection Receiver out of the Connection Provider");
        let (returned_connections_sink, mut server_connections_source) = tokio::sync::mpsc::channel::<SocketConnection<()>>(2);
        let socket_communications_handler = SocketConnectionHandler::<DEFAULT_TEST_CONFIG_U64, String, String, DefaultTestUni<String>, SenderChannel<String>>::new();
        socket_communications_handler.server_loop_for_unresponsive_text_protocol(
            LISTENING_INTERFACE, PORT, new_connections_source, returned_connections_sink,
            |_connection_event| async {},
            move |_listening_interface, _listening_port, _peer, client_messages| {
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
        let tokio_connection = TcpStream::connect(format!("{}:{}", LISTENING_INTERFACE, PORT).to_socket_addrs().expect("Error resolving address").into_iter().next().unwrap()).await.expect("Error connecting");
        let socket_connection = SocketConnection::new(tokio_connection, ());
        let socket_connection_handler = SocketConnectionHandler::<DEFAULT_TEST_CONFIG_U64, String, String, DefaultTestUni<String>, SenderChannel<String>>::new();
        tokio::spawn(
            socket_connection_handler.client_for_unresponsive_text_protocol(
                socket_connection,
                client_shutdown_receiver,
                move |connection_event| {
                    let sent_messages_count = Arc::clone(&sent_messages_count_ref);
                    match connection_event {
                        SingleProtocolEvent::PeerConnected { peer } => {
                            tokio::spawn(async move {
                                let start = SystemTime::now();
                                let mut n = 0;
                                loop {
                                    let send_result = peer.send_async(format!("DoNotAnswer({})", n)).await;
                                    assert!(send_result.is_ok(), "couldn't send: {:?}", send_result.unwrap_err());
                                    n += 1;
                                    // flush & bailout check for timeout every 128k messages
                                    if n % (1<<17) == 0 && start.elapsed().unwrap().as_millis() as u64 >= TEST_DURATION_MS {
                                        println!("Client sent {} messages before bailing out", n);
                                        sent_messages_count.store(n, Relaxed);
                                        assert_eq!(peer.flush_and_close(Duration::from_secs(1)).await, 0, "couldn't flush!");
                                        break;
                                    }
                                }
                            });
                        },
                        SingleProtocolEvent::PeerDisconnected { .. } => {},
                        SingleProtocolEvent::LocalServiceTermination { .. } => {},
                    }
                    future::ready(())
                },
                move |_listening_interface, _listening_port, _peer, server_messages| {
                    server_messages
                }
            )
        );
        println!("### Measuring latency for 2 seconds...");

        tokio::time::sleep(Duration::from_millis((TEST_DURATION_MS as f64 * 1.01) as u64)).await;    // allow the test to take 1% more than the necessary to avoid silly errors
        client_shutdown_sender.send(()).expect("sending client termination signal");
        server_connections_source.close();  // terminates the server

        println!("### Waiting a little for the shutdown signal to reach the server...");
        tokio::time::sleep(Duration::from_millis(505)).await;

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

        let (_client_shutdown_sender, client_shutdown_receiver) = tokio::sync::broadcast::channel(1);

        // server
        let client_secret_ref = client_secret.clone();
        let server_secret_ref = server_secret.clone();
        let mut connection_provider = ServerConnectionHandler::new(LISTENING_INTERFACE, PORT, ()).await
            .expect("couldn't start the Connection Provider server event loop");
        let new_connections_source = connection_provider.connection_receiver()
            .expect("couldn't move the Connection Receiver out of the Connection Provider");
        let (returned_connections_sink, mut server_connections_source) = tokio::sync::mpsc::channel::<SocketConnection<()>>(2);
        let socket_communications_handler = SocketConnectionHandler::<DEFAULT_TEST_CONFIG_U64, String, String, DefaultTestUni, SenderChannel>::new();
        socket_communications_handler.server_loop_for_responsive_text_protocol(LISTENING_INTERFACE, PORT, new_connections_source, returned_connections_sink,
                                                                               |_connection_event| future::ready(()),
                                                                               move |_client_addr, _client_port, peer, client_messages_stream| {
               let client_secret_ref = client_secret_ref.clone();
               let server_secret_ref = server_secret_ref.clone();
                assert!(peer.send(String::from("Welcome! State your business!")).is_ok(), "couldn't send");
                client_messages_stream.flat_map(move |client_message| {
                   stream::iter([
                       format!("Client just sent '{}'", client_message),
                       if *client_message == client_secret_ref {
                           server_secret_ref.clone()
                       } else {
                           panic!("Client sent the wrong secret: '{}' -- I was expecting '{}'", client_message, client_secret_ref);
                       }])
               })
            }
        ).await.expect("ERROR starting the server");

        println!("### Waiting a little for the server to start...");
        tokio::time::sleep(Duration::from_millis(10)).await;

        // client
        let client_secret = client_secret.clone();
        let observed_secret_ref = Arc::clone(&observed_secret);
        let tokio_connection = TcpStream::connect(format!("{}:{}", LISTENING_INTERFACE, PORT).to_socket_addrs().expect("Error resolving address").into_iter().next().unwrap()).await.expect("Error connecting");
        let socket_connection = SocketConnection::new(tokio_connection, ());
        let socket_communications_handler = SocketConnectionHandler::<DEFAULT_TEST_CONFIG_U64, String, String, DefaultTestUni, SenderChannel>::new();
        tokio::spawn(
            socket_communications_handler.client_for_responsive_text_protocol(socket_connection, client_shutdown_receiver,
                                                                              move |_connection_event| future::ready(()),
                                                                              move |_client_addr, _client_port, peer, server_messages_stream| {
                    let observed_secret_ref = Arc::clone(&observed_secret_ref);
                    assert!(peer.send(client_secret.clone()).is_ok(), "couldn't send");
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
            )
        );
        println!("### Started a client -- which is running concurrently, in the background... it has 100ms to do its thing!");

        tokio::time::sleep(Duration::from_millis(100)).await;
        server_connections_source.close();  // sends the server termination signal

        println!("### Waiting a little for the shutdown signal to reach the server...");
        tokio::time::sleep(Duration::from_millis(10)).await;

        let locked_observed_secret = observed_secret.lock().await;
        let observed_secret = locked_observed_secret.as_ref().expect("Server secret has not been computed");
        assert_eq!(*observed_secret, server_secret, "Communications didn't go according the plan");
    }

    /// assures that shutting down the client causes the connection to be dropped, as perceived by the server
    #[cfg_attr(not(doc),tokio::test)]
    async fn client_termination() {
        const LISTENING_INTERFACE: &str = "127.0.0.1";
        const PORT               : u16  = 8574;
        let server_disconnected = Arc::new(AtomicBool::new(false));
        let server_disconnected_ref = Arc::clone(&server_disconnected);
        let client_disconnected = Arc::new(AtomicBool::new(false));
        let client_disconnected_ref = Arc::clone(&client_disconnected);

        let (client_shutdown_sender, client_shutdown_receiver) = tokio::sync::broadcast::channel(2);

        let mut connection_provider = ServerConnectionHandler::new(LISTENING_INTERFACE, PORT, ()).await
            .expect("couldn't start the Connection Provider server event loop");
        let new_connections_source = connection_provider.connection_receiver()
            .expect("couldn't move the Connection Receiver out of the Connection Provider");
        let (returned_connections_sink, mut server_connections_source) = tokio::sync::mpsc::channel::<SocketConnection<()>>(2);
        let socket_communications_handler = SocketConnectionHandler::<DEFAULT_TEST_CONFIG_U64, String, String, DefaultTestUni, SenderChannel>::new();
        socket_communications_handler.server_loop_for_unresponsive_text_protocol(
            LISTENING_INTERFACE, PORT, new_connections_source, returned_connections_sink,
            move |connection_event| {
                let server_disconnected = Arc::clone(&server_disconnected_ref);
                async move {
                    if let SingleProtocolEvent::PeerDisconnected { .. } = connection_event {
                        server_disconnected.store(true, Relaxed);
                    }
                }
            },
            move |_client_addr, _client_port, _peer, client_messages_stream| client_messages_stream
        ).await.expect("ERROR starting the server");

        // wait a bit for the server to start
        tokio::time::sleep(Duration::from_millis(10)).await;

        let tokio_connection = TcpStream::connect(format!("{}:{}", LISTENING_INTERFACE, PORT).to_socket_addrs().expect("Error resolving address").into_iter().next().unwrap()).await.expect("Error connecting");
        let socket_connection = SocketConnection::new(tokio_connection, ());
        let socket_communications_handler = SocketConnectionHandler::<DEFAULT_TEST_CONFIG_U64, String, String, DefaultTestUni, SenderChannel>::new();

        tokio::spawn(
            socket_communications_handler.client_for_unresponsive_text_protocol(
                socket_connection, client_shutdown_receiver,
                move |connection_event| {
                    let client_disconnected = Arc::clone(&client_disconnected_ref);
                    async move {
                        if let SingleProtocolEvent::PeerDisconnected { .. } = connection_event {
                            client_disconnected.store(true, Relaxed);
                        }
                    }
                },
                move |_client_addr, _client_port, _peer, server_messages_stream| server_messages_stream
            )
        );

        // wait a bit for the connection, shutdown the client, wait another bit
        tokio::time::sleep(Duration::from_millis(1000)).await;
        client_shutdown_sender.send(()).expect("sending client shutdown signal");
        tokio::time::sleep(Duration::from_millis(1000)).await;

        assert!(client_disconnected.load(Relaxed), "Client didn't drop the connection with the server");
        assert!(server_disconnected.load(Relaxed), "Server didn't notice the drop in the connection made by the client");

        // unneeded, but placed here for symmetry
        server_connections_source.close();  // sends the server termination command
    }
}
