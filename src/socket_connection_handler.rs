//! Tokio version of the inspiring `message-io` crate, but improving it on the following (as of 2022-08):
//! 1) Tokio is used for async IO -- instead of something else `message-io` uses, which is out of Rust's async execution context;
//! 2) `message-io` has poor/no support for streams/textual protocols: it, eventually, breaks messages in overload scenarios,
//!    transforming 1 good message into 2 invalid ones -- its `FramedTCP` comm model is not subjected to that, for the length is prepended to the message;
//! 3) `message-io` is prone to DoS attacks: during flood tests, be it in Release or Debug mode, `message-io` answers to a single peer only -- the flooder.
//! 4) `message-io` uses ~3x more CPU and is single threaded -- throughput couldn't be measured for the network speed was the bottleneck; latency was not measured at all


use super::{
    config::*,
    types::*,
    serde::{SocketServerDeserializer, SocketServerSerializer},
};
use crate::{SenderUniType, SocketServer};
use std::{cell::Cell, collections::VecDeque, fmt::Debug, future, future::Future, marker::PhantomData, net::SocketAddr, ops::Deref, sync::{Arc, atomic::{AtomicBool, AtomicU32, AtomicU8}, atomic::Ordering::Relaxed}, task::{Context, Poll, Waker}, time::Duration};
use std::fmt::Formatter;
use std::hash::{Hash, Hasher};
use std::net::{IpAddr, Ipv4Addr};
use std::str::FromStr;
use reactive_mutiny::prelude::advanced::{MutinyStream, UniZeroCopyAtomic, ChannelCommon, ChannelUni, ChannelProducer, Instruments, ChannelUniZeroCopyAtomic, OgreUnique, AllocatorAtomicArray};
use futures::{StreamExt, Stream, stream, SinkExt};
use tokio::{
    io::{self,AsyncWriteExt, AsyncBufReadExt, BufReader, BufWriter, Interest},
    net::{TcpListener, TcpStream, tcp::WriteHalf},
    sync::Mutex,
};
use tokio::sync::oneshot::error::RecvError;
use log::{trace, debug, warn, error};
use tokio::io::AsyncReadExt;
use crate::prelude::ProcessorRemoteStreamType;


// base functions
/////////////////
// in this section, the base functions for all variants are declared -- where the variants will be:
//   - all combinations of the Stream's output types returned by `dialog_processor_builder_fn()`: futures/non-future & fallible/non-fallible;
//   - unresponsive / responsive output types (also issued by the Streams created by `dialog_processor_builder_fn()`).
// The base functions will implement the "unresponsive" & futures & fallibles variant only.

/// Serves textual protocols whose events/commands/sentences are separated by '\n':
///   - `dialog_processor_builder_fn()` builds streams that will receive `ClientMessages` but will generate no answers;
///   - sending messages back to the client will be done explicitly by `peer`;
///   - `shutdown_signaler` comes from a `tokio::sync::oneshot::channel`, which receives the maximum time, in
///     milliseconds: it will be passed to `connection_events_callback` and cause the server loop to exit.
#[inline(always)]
pub async fn server_loop_for_unresponsive_text_protocol<ClientMessages:                 SocketServerDeserializer<ClientMessages>              + Send + Sync + PartialEq + Debug + 'static,
                                                        ServerMessages:                 SocketServerSerializer<ServerMessages>                + Send + Sync + PartialEq + Debug + 'static,
                                                        PipelineOutputType:                                                                     Send + Sync +             Debug + 'static,
                                                        OutputStreamType:               Stream<Item=PipelineOutputType>                       + Send                            + 'static,
                                                        ConnectionEventsCallbackFuture: Future<Output=()>,
                                                        ConnectionEventsCallback:       Fn(/*server_event: */ConnectionEvent<ServerMessages>) -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                                        ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<ServerMessages>>, /*client_messages_stream: */ProcessorRemoteStreamType<ClientMessages>) -> OutputStreamType>

                                                       (listening_interface:         String,
                                                        listening_port:              u16,
                                                        mut shutdown_signaler:       tokio::sync::oneshot::Receiver<u32>,
                                                        connection_events_callback:  ConnectionEventsCallback,
                                                        dialog_processor_builder_fn: ProcessorBuilderFn)

                                                       -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

    let connection_events_callback = Arc::new(connection_events_callback);
    let listener = TcpListener::bind(&format!("{}:{}", listening_interface, listening_port)).await?;

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
        let sender = SenderChannel::<ServerMessages, SENDER_BUFFER_SIZE>::new(format!("Sender for client {addr}"));
        let (mut sender_stream, _) = Arc::clone(&sender).create_stream();
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
        let processor_sender = SocketProcessorUniType::<ClientMessages>::new(format!("Server processor for remote client {addr} @ {listening_interface}:{listening_port}"))
            .spawn_non_futures_non_fallibles_executors(1,
                                                       |in_stream| dialog_processor_builder_fn(client_ip.clone(), client_port, peer_ref1.clone(), in_stream),
                                                       move |executor| {
                                                           // issue the async disconnect event
                                                           futures::executor::block_on(connection_events_callback_ref(ConnectionEvent::PeerDisconnected { peer: peer_ref2 }));
                                                           future::ready(())
                                                       });

        // spawn a task to handle communications with that client
        tokio::spawn(dialog_loop_for_textual_protocol(socket, peer, processor_sender));
    }

    debug!("SocketServer: bailing out of network loop -- we should be undergoing a shutdown...");

    Ok(())
}

/// Executes a client for textual protocols whose events/commands/sentences are separated by '\n':
///   - `dialog_processor_builder_fn()` builds streams that will receive `ServerMessages` but will generate no answers;
///   - sending messages back to the server will be done explicitly by `peer`;
///   - `shutdown_signaler` comes from a `tokio::sync::oneshot::channel`, which receives the maximum time, in
///     milliseconds: it will be passed to `connection_events_callback` and cause the server loop to exit.
#[inline(always)]
pub async fn client_for_unresponsive_text_protocol<ClientMessages:                 SocketServerSerializer<ClientMessages>                + Send + Sync + PartialEq + Debug + 'static,
                                                   ServerMessages:                 SocketServerDeserializer<ServerMessages>              + Send + Sync + PartialEq + Debug + 'static,
                                                   PipelineOutputType:                                                              Send + Sync +             Debug + 'static,
                                                   OutputStreamType:               Stream<Item=PipelineOutputType>                       + Send + 'static,
                                                   ConnectionEventsCallbackFuture: Future<Output=()>,
                                                   ConnectionEventsCallback:       Fn(/*server_event: */ConnectionEvent<ClientMessages>) -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                                   ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<ClientMessages>>, /*server_messages_stream: */ProcessorRemoteStreamType<ServerMessages>) -> OutputStreamType>

                                                  (server_ipv4_addr:            String,
                                                   port:                        u16,
                                                   mut shutdown_signaler:       tokio::sync::oneshot::Receiver<u32>,
                                                   connection_events_callback:  ConnectionEventsCallback,
                                                   dialog_processor_builder_fn: ProcessorBuilderFn)

                                                  -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::from_str(server_ipv4_addr.as_str())?), port);
    let socket = TcpStream::connect(addr).await?;

    // prepares for the dialog to come with the (just accepted) connection
    let sender = SenderChannel::<ClientMessages, SENDER_BUFFER_SIZE>::new(format!("Sender for client {addr}"));
    let peer = Arc::new(Peer::new(sender, addr));
    let peer_ref1 = Arc::clone(&peer);
    let peer_ref2 = Arc::clone(&peer);
    let peer_ref3 = Arc::clone(&peer);

    // issue the connection event
    connection_events_callback(ConnectionEvent::PeerConnected {peer: peer.clone()}).await;

    let processor_sender = SocketProcessorUniType::<ServerMessages>::new(format!("Client Processor for remote server @ {addr}"))
        .spawn_non_futures_non_fallibles_executors(1,
                                                   |in_stream| dialog_processor_builder_fn(server_ipv4_addr.clone(), port, peer_ref1.clone(), in_stream),
                                                   move |executor| {
                                                       // issue the async disconnect event
                                                       futures::executor::block_on(connection_events_callback(ConnectionEvent::PeerDisconnected { peer: peer_ref2 }));
                                                       future::ready(())
                                                   });

    // spawn the processor
    tokio::spawn(dialog_loop_for_textual_protocol(socket, peer, processor_sender));
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
async fn dialog_loop_for_textual_protocol<RemoteMessages: SocketServerDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug + 'static,
                                          LocalMessages:  SocketServerSerializer<LocalMessages>    + Send + Sync + PartialEq + Debug + 'static>

                                         (mut textual_socket: TcpStream,
                                          peer:               Arc<Peer<LocalMessages>>,
                                          processor_uni:      Arc<SocketProcessorUniType<RemoteMessages>>)

                                         -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

    textual_socket.set_nodelay(true).map_err(|err| format!("error setting nodelay() for the socket connected at {}:{}: {err}", peer.peer_address, peer.peer_id))?;
    textual_socket.set_ttl(30).map_err(|err| format!("error setting ttl(30) for the socket connected at {}:{}: {err}", peer.peer_address, peer.peer_id))?;
    textual_socket.set_linger(Some(Duration::from_secs(3))).map_err(|err| format!("error setting linger(Some(Duration::from_secs(3))) for the socket connected at {}:{}: {err}", peer.peer_address, peer.peer_id))?;
    let mut read_buffer = Vec::with_capacity(CHAT_MSG_SIZE_HINT);
    let mut serialization_buffer = Vec::with_capacity(CHAT_MSG_SIZE_HINT);

    let (mut sender_stream, _) = peer.sender.create_stream();

    'connection: loop {
        // wait for the socket to be readable or until we have something to write
        tokio::select!(

            biased;     // sending has priority over receiving

            // send?
            to_send = sender_stream.next() => {
                match to_send {
                    Some(to_send) => {
                        LocalMessages::serialize(&to_send, &mut serialization_buffer);
                        serialization_buffer.push('\n' as u8);
                        if let Err(err) = textual_socket.write_all(&serialization_buffer).await {
                            warn!("reactive-messaging: PROBLEM in the connection with {:#?} while WRITING: '{:?}' -- dropping it", peer, err);
                            peer.sender.cancel_all_streams();
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
                            if let Some(mut eol_pos) = read_buffer[next_line_index+this_line_search_start..].iter().position(|&b| b == '\n' as u8) {
                                eol_pos += next_line_index+this_line_search_start;
                                let line_bytes = &read_buffer[next_line_index..eol_pos];
                                match RemoteMessages::deserialize(&line_bytes) {
                                    Ok(client_message) => {
                                        if !processor_uni.try_send(|slot| unsafe { std::ptr::write(slot, client_message) }) {
                                            // breach in protocol: we cannot process further messages (they were received too fast)
                                            let msg = format!("Input message ignored: local processor is too busy -- `dialog_processor` is full of unprocessed messages ({}/{}) while attempting to enqueue another message from {:?} (peer id {})",
                                                                     processor_uni.channel.pending_items_count(), processor_uni.channel.buffer_size(), peer.peer_address, peer.peer_id);
                                            peer.sender.try_send(|slot| unsafe { std::ptr::write(slot, LocalMessages::processor_error_message(msg.clone())) });
                                            error!("reactive-messaging: {}", msg);
                                        }
                                    },
                                    Err(err) => {
                                        let stripped_line = String::from_utf8_lossy(line_bytes);
                                        let error_message = format!("Unknown command received from {:?} (peer id {}): '{}'",
                                                                           peer.peer_address, peer.peer_id, stripped_line);
                                        warn!("reactive-messaging: {error_message}");
                                        let outgoing_error = LocalMessages::processor_error_message(error_message);
                                        let sent = peer.sender.try_send(|slot| unsafe { std::ptr::write(slot, outgoing_error) });
                                        if !sent {
                                            // prevent bad clients from depleting our resources: if the out buffer is full due to bad input, we'll wait 1 sec to process the next message
                                            tokio::time::sleep(Duration::from_millis(1000)).await;
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
                        peer.sender.cancel_all_streams();
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
    let _ = processor_uni.close(GRACEFUL_STREAM_ENDING_TIMEOUT_DURATION).await;
    textual_socket.flush().await.map_err(|err| format!("error flushing the textual socket connected to {}:{}", peer.peer_address, peer.peer_id))?;
    textual_socket.shutdown().await.map_err(|err| format!("error flushing the textual socket connected to {}:{}", peer.peer_address, peer.peer_id))?;
    Ok(())
}


// Responsive variants
//////////////////////

/// Similar to [server_loop_for_unresponsive_text_protocol()], but for when the provided `dialog_processor_builder_fn()` produces
/// answers of type `ServerMessages`, which will be automatically routed to the clients.
#[inline(always)]
pub async fn server_loop_for_responsive_text_protocol<ClientMessages:                 SocketServerDeserializer<ClientMessages>              + Send + Sync + PartialEq + Debug + 'static,
                                                      ServerMessages:                 SocketServerSerializer<ServerMessages>                + Send + Sync + PartialEq + Debug + 'static,
                                                      OutputStreamType:               Stream<Item=ServerMessages>                           + Send + 'static,
                                                      ConnectionEventsCallbackFuture: Future<Output=()>,
                                                      ConnectionEventsCallback:       Fn(/*server_event: */ConnectionEvent<ServerMessages>) -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                                      ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<ServerMessages>>, /*client_messages_stream: */ProcessorRemoteStreamType<ClientMessages>) -> OutputStreamType>

                                                     (listening_interface:         String,
                                                      listening_port:              u16,
                                                      mut shutdown_signaler:       tokio::sync::oneshot::Receiver<u32>,
                                                      connection_events_callback:  ConnectionEventsCallback,
                                                      dialog_processor_builder_fn: ProcessorBuilderFn)

                                                     -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

    // upgrade the "unresponsive" streams from `dialog_processor_builder_fn()` to "responsive" streams that will send the outcome to clients
    let dialog_processor_builder_fn = |client_addr, connected_port, peer, client_messages_stream| {
        let dialog_processor_stream = dialog_processor_builder_fn(client_addr, connected_port, Arc::clone(&peer), client_messages_stream);
        to_responsive_stream(peer, dialog_processor_stream.map(|e| Ok(e)))
    };
    server_loop_for_unresponsive_text_protocol(listening_interface.clone(), listening_port, shutdown_signaler, connection_events_callback, dialog_processor_builder_fn).await
        .map_err(|err| Box::from(format!("error when starting server @ {listening_interface}:{listening_port} with `server_loop_for_unresponsive_text_protocol()`: {err}")))
}

/// Similar to [client_for_unresponsive_text_protocol()], but for when the provided `dialog_processor_builder_fn()` produces
/// answers of type `ClientMessages`, which will be automatically routed to the server.
#[inline(always)]
pub async fn client_for_responsive_text_protocol<ClientMessages:                 SocketServerSerializer<ClientMessages>                + Send + Sync + PartialEq + Debug + 'static,
                                                 ServerMessages:                 SocketServerDeserializer<ServerMessages>              + Send + Sync + PartialEq + Debug + 'static,
                                                 OutputStreamType:               Stream<Item=ClientMessages>                           + Send + 'static,
                                                 ConnectionEventsCallbackFuture: Future<Output=()>,
                                                 ConnectionEventsCallback:       Fn(/*server_event: */ConnectionEvent<ClientMessages>) -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                                 ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<ClientMessages>>, /*server_messages_stream: */ProcessorRemoteStreamType<ServerMessages>) -> OutputStreamType>

                                                (server_ipv4_addr:            String,
                                                 port:                        u16,
                                                 mut shutdown_signaler:       tokio::sync::oneshot::Receiver<u32>,
                                                 connection_events_callback:  ConnectionEventsCallback,
                                                 dialog_processor_builder_fn: ProcessorBuilderFn)

                                                -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

    // upgrade the "unresponsive" streams from `dialog_processor_builder_fn()` to "responsive" streams that will send the outcome to clients
    let dialog_processor_builder_fn = |server_addr, port, peer, server_messages_stream| {
        let dialog_processor_stream = dialog_processor_builder_fn(server_addr, port, Arc::clone(&peer), server_messages_stream);
        to_responsive_stream(peer, dialog_processor_stream.map(|e| Ok(e)))
    };
    client_for_unresponsive_text_protocol(server_ipv4_addr.clone(), port, shutdown_signaler, connection_events_callback, dialog_processor_builder_fn).await
        .map_err(|err| Box::from(format!("error when starting client for server @ {server_ipv4_addr}:{port} with `client_for_unresponsive_text_protocol()`: {err}")))
}

/// upgrades the `request_processor_stream` to a `Stream` which is also able send answers back to the `peer`
fn to_responsive_stream<LocalMessages: SocketServerSerializer<LocalMessages> + Send + Sync + PartialEq + Debug + 'static>
                       (peer:                     Arc<Peer<LocalMessages>>,
                        request_processor_stream: impl Stream<Item = Result<LocalMessages, Box<dyn std::error::Error + Sync + Send>>>)
                       -> impl Stream<Item = ()> {

    request_processor_stream
        .map(move |processor_response| {
            let outgoing = match processor_response {
                Ok(outgoing) => {
                    outgoing
                },
                Err(err) => {
                    let err_string = format!("{:?}", err);
                    error!("SocketServer: processor connected with {} (peer id {}) yielded an error: {}", peer.peer_address, peer.peer_id, err_string);
                    let error_message = LocalMessages::processor_error_message(err_string);
                    error_message
                },
            };
            // send the message, skipping messages that are programmed not to generate any response
            if LocalMessages::is_disconnect_message(&outgoing) {
                trace!("SocketServer: processor choose to drop connection with {} (peer id {}): '{:?}'", peer.peer_address, peer.peer_id, outgoing);
                if !LocalMessages::is_no_answer_message(&outgoing) {
                    let _sent = peer.sender.try_send(|slot| unsafe { std::ptr::write(slot,  outgoing) } );
                }
                peer.sender.cancel_all_streams();
            } else if !LocalMessages::is_no_answer_message(&outgoing) {
                // send the answer
                trace!("Sending Answer `{:?}` to {:?} (peer id {})", outgoing, peer.peer_address, peer.peer_id);
                let sent = peer.sender.try_send(|slot|  unsafe { std::ptr::write(slot,  outgoing) } );
                if !sent {
                    warn!("Slow reader detected -- {:?} (peer id {}). Closing the connection...", peer.peer_address, peer.peer_id);
                    // peer is slow-reading (and, possibly, fast sending?) -- this is a protocol offence and the connection will be closed (as soon as all messages are sent?? names should be reviewed: gracefully_end_all_streams() & immediately_cancel_all_streams())
                    peer.sender.cancel_all_streams();
                }
            }


        })
}


static PEER_COUNTER: AtomicU32 = AtomicU32::new(0);
pub type PeerId = u32;
/// Represents a channel to a peer able to send out `MessageType` kinds of messages from "here" to "there"
pub struct Peer<MessagesType: 'static + Send + Sync + PartialEq + Debug + SocketServerSerializer<MessagesType>> {
    pub peer_id:      PeerId,
    pub sender:       Arc<SenderChannel<MessagesType, SENDER_BUFFER_SIZE>>,
    pub peer_address: SocketAddr,
}

impl<MessagesType: 'static + Send + Sync + PartialEq + Debug + SocketServerSerializer<MessagesType>>
Peer<MessagesType> {

    pub fn new(sender: Arc<SenderChannel<MessagesType, SENDER_BUFFER_SIZE>>, peer_address: SocketAddr) -> Self {
        Self {
            peer_id: PEER_COUNTER.fetch_add(1, Relaxed),
            sender,
            peer_address,
        }
    }

}

impl<MessagesType: 'static + Send + Sync + PartialEq + Debug + SocketServerSerializer<MessagesType>>
Debug for
Peer<MessagesType> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Peer {{peer_id: {}, peer_address: '{}', sender: {} pending messages}}",
                  self.peer_id, self.peer_address, self.sender.pending_items_count())
    }
}


/// Unit tests the [tokio_message_io](self) module
#[cfg(any(test,doc))]
mod tests {
    use std::time::SystemTime;
    use futures::future::BoxFuture;
    use super::*;

    #[cfg(debug_assertions)]
    const DEBUG: bool = true;
    #[cfg(not(debug_assertions))]
    const DEBUG: bool = false;


    #[ctor::ctor]
    fn suite_setup() {
        simple_logger::SimpleLogger::new().with_utc_timestamps().init().unwrap_or_else(|_| eprintln!("--> LOGGER WAS ALREADY STARTED"));
    }

    /// assures connection & dialogs work for either the server and client, using the "unresponsive" flavours of the `Stream` processors
    #[cfg_attr(not(doc),tokio::test)]
    async fn unresponsive_dialogs() {
        const LISTENING_INTERFACE: &str = "127.0.0.1";
        const PORT               : u16  = 8570;
        let client_secret = format!("open, sesame");
        let server_secret = format!("now the 40 of you may enter");
        let observed_secret = Arc::new(Mutex::new(None));

        let (server_shutdown_sender, server_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (_client_shutdown_sender, client_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();

        // server
        let client_secret_ref = client_secret.clone();
        let server_secret_ref = server_secret.clone();
        tokio::spawn(server_loop_for_unresponsive_text_protocol(LISTENING_INTERFACE.to_string(), PORT, server_shutdown_receiver,
                                                                |connection_event| {
               match connection_event {
                   ConnectionEvent::PeerConnected { peer } => {
                       assert!(peer.sender.try_send_movable(format!("Welcome! State your business!")), "couldn't send");
                   },
                   ConnectionEvent::PeerDisconnected { peer } => {},
                   ConnectionEvent::ApplicationShutdown { timeout_ms } => {
                       println!("Test Server: shutdown was requested ({timeout_ms}ms timeout)... No connection will receive the drop message (nor will be even closed) because I, the lib caller, intentionally didn't keep track of the connected peers for this test!");
                   }
               }
               future::ready(())
           },
                                                                move |client_addr, client_port, peer, client_messages_stream| {
               let client_secret_ref = client_secret_ref.clone();
               let server_secret_ref = server_secret_ref.clone();
               client_messages_stream.inspect(move |client_message| {
                   assert!(peer.sender.try_send_movable(format!("Client just sent '{}'", client_message)), "couldn't send");
                   if &*client_message == &client_secret_ref {
                       assert!(peer.sender.try_send_movable(format!("{}", server_secret_ref)), "couldn't send");
                   } else {
                       panic!("Client sent the wrong secret: '{}' -- I was expecting '{}'", client_message, client_secret_ref);
                   }
               })
            }));

        println!("### Waiting a little for the server to start...");
        tokio::time::sleep(Duration::from_millis(10)).await;

        // client
        let client_secret_ref1 = client_secret.clone();
        let server_secret_ref = server_secret.clone();
        let observed_secret_ref = Arc::clone(&observed_secret);
        client_for_unresponsive_text_protocol(LISTENING_INTERFACE.to_string(), PORT, client_shutdown_receiver,
                                              move |connection_event| {
                match connection_event {
                    ConnectionEvent::PeerConnected { peer } => {
                        assert!(peer.sender.try_send_movable(format!("{}", client_secret_ref1)), "couldn't send");
                    },
                    ConnectionEvent::PeerDisconnected { peer } => {
                        println!("Test Client: connection with {} (peer_id #{}) was dropped -- should not happen in this test", peer.peer_address, peer.peer_id);
                    },
                    ConnectionEvent::ApplicationShutdown { timeout_ms } => {}
                }
                future::ready(())
            },
                                              move |client_addr, client_port, peer, server_messages_stream: ProcessorRemoteStreamType<String>| {
                let observed_secret_ref = Arc::clone(&observed_secret_ref);
                server_messages_stream.then(move |server_message| {
                    let observed_secret_ref = Arc::clone(&observed_secret_ref);
                    async move {
                        println!("Server said: '{}'", server_message);
                        let _ = observed_secret_ref.lock().await.insert(server_message);
                    }
                })
            }).await.expect("Starting the client");
        println!("### Started a client -- which is running concurrently, in the background... it has 100ms to do its thing!");

        tokio::time::sleep(Duration::from_millis(100)).await;
        server_shutdown_sender.send(500).expect("sending server shutdown signal");

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
    async fn latency_measurements() {
        const TEST_DURATION_MS:    u64  = 2000;
        const TEST_DURATION_NS:    u64  = TEST_DURATION_MS * 1e6 as u64;
        const LISTENING_INTERFACE: &str = "127.0.0.1";
        const PORT               : u16  = 8571;
        let (server_shutdown_sender, server_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (client_shutdown_sender, client_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();

        // server
        tokio::spawn(server_loop_for_unresponsive_text_protocol(LISTENING_INTERFACE.to_string(), PORT, server_shutdown_receiver,
                                                                |_connection_event| async {},
                                                                |listening_interface, listening_port, peer, client_messages: ProcessorRemoteStreamType<String>| {
                client_messages.inspect(move |client_message| {
                    // Receives: "Ping(n)"
                    let n_str = &client_message[5..(client_message.len() - 1)];
                    let n = str::parse::<u32>(n_str).unwrap_or_else(|err| panic!("could not convert '{}' to number. Original client message: '{}'. Parsing error: {:?}", n_str, client_message, err));
                    // Answers:  "Pong(n)"
                    assert!(peer.sender.try_send_movable(format!("Pong({})", n)), "couldn't send");
                })
            }));

        println!("### Waiting a little for the server to start...");
        tokio::time::sleep(Duration::from_millis(10)).await;

        let counter = Arc::new(AtomicU32::new(0));
        let counter_ref = Arc::clone(&counter);
        // client
        client_for_unresponsive_text_protocol(LISTENING_INTERFACE.to_string(), PORT, client_shutdown_receiver,
                                              |connection_event| {
                match connection_event {
                    ConnectionEvent::PeerConnected { peer } => {
                        // conversation starter
                        assert!(peer.sender.try_send_movable(format!("Ping(0)")), "couldn't send");
                    },
                    ConnectionEvent::PeerDisconnected { .. } => {},
                    ConnectionEvent::ApplicationShutdown { .. } => {},
                }
                future::ready(())
            },
                                              move |listening_interface, listening_port, peer, server_messages: ProcessorRemoteStreamType<String>| {
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
            }).await.expect("Starting the client");

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
    async fn message_flooding_throughput() {
        const TEST_DURATION_MS:    u64  = 2000;
        const TEST_DURATION_NS:    u64  = TEST_DURATION_MS * 1e6 as u64;
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
        tokio::spawn(server_loop_for_unresponsive_text_protocol(LISTENING_INTERFACE.to_string(), PORT, server_shutdown_receiver,
                                                                |_connection_event: ConnectionEvent<String>| async {},
                                                                move |listening_interface, listening_port, peer, client_messages: ProcessorRemoteStreamType<String>| {
                let received_messages_count = Arc::clone(&received_messages_count_ref);
                let unordered = Arc::clone(&unordered_ref);
                client_messages.inspect(move |client_message| {
                    // Message format: DoNotAnswer(n)
                    let n_str = &client_message[12..(client_message.len()-1)];
                    let n = str::parse::<u32>(n_str).expect(&format!("could not convert '{}' to number. Original message: '{}'", n_str, client_message));
                    let count = received_messages_count.fetch_add(1, Relaxed);
                    if count != n {
                        if unordered.compare_exchange(0, count, Relaxed, Relaxed).is_ok() {
                            println!("Server: ERROR: received order of messages broke at message #{}", count);
                        };
                    }
                })
        }));

        println!("### Waiting a little for the server to start...");
        tokio::time::sleep(Duration::from_millis(10)).await;

        // client
        let sent_messages_count = Arc::new(AtomicU32::new(0));
        let sent_messages_count_ref = Arc::clone(&sent_messages_count);
        client_for_unresponsive_text_protocol(LISTENING_INTERFACE.to_string(), PORT,
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
                                              move |listening_interface, listening_port, peer, server_messages: ProcessorRemoteStreamType<String>| {
                server_messages
            }).await.expect("Starting the client");
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
        println!("    {} received messages {}", received_messages_count, if unordered == 0 {format!("in order")} else {format!("unordered -- ordering broke at message #{}", unordered)});
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
        let client_secret = format!("open, sesame");
        let server_secret = format!("now the 40 of you may enter");
        let observed_secret = Arc::new(Mutex::new(None));

        let (server_shutdown_sender, server_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (_client_shutdown_sender, client_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();

        // server
        let client_secret_ref = client_secret.clone();
        let server_secret_ref = server_secret.clone();
        tokio::spawn(server_loop_for_responsive_text_protocol(LISTENING_INTERFACE.to_string(), PORT, server_shutdown_receiver,
            |connection_event| future::ready(()),
            move |client_addr, client_port, peer, client_messages_stream| {
               let client_secret_ref = client_secret_ref.clone();
               let server_secret_ref = server_secret_ref.clone();
                assert!(peer.sender.try_send_movable(format!("Welcome! State your business!")), "couldn't send");
                client_messages_stream.flat_map(move |client_message: SocketProcessorDerivedType<String>| {
                   stream::iter([
                       format!("Client just sent '{}'", client_message),
                       if &*client_message == &client_secret_ref {
                           format!("{}", server_secret_ref)
                       } else {
                           panic!("Client sent the wrong secret: '{}' -- I was expecting '{}'", client_message, client_secret_ref);
                       }])
               })
            }));

        println!("### Waiting a little for the server to start...");
        tokio::time::sleep(Duration::from_millis(10)).await;

        // client
        let client_secret_ref1 = client_secret.clone();
        let server_secret_ref = server_secret.clone();
        let observed_secret_ref = Arc::clone(&observed_secret);
        client_for_responsive_text_protocol(LISTENING_INTERFACE.to_string(), PORT, client_shutdown_receiver,
            move |connection_event| future::ready(()),
            move |client_addr, client_port, peer, server_messages_stream: ProcessorRemoteStreamType<String>| {
                let observed_secret_ref = Arc::clone(&observed_secret_ref);
                assert!(peer.sender.try_send_movable(format!("{}", client_secret_ref1)), "couldn't send");
                server_messages_stream
                    .then(move |server_message| {
                        let observed_secret_ref = Arc::clone(&observed_secret_ref);
                        async move {
                            println!("Server said: '{}'", server_message);
                            let _ = observed_secret_ref.lock().await.insert(server_message);
                        }
                    })
                    .map(|server_message| ".".to_string())
            }).await.expect("Starting the client");
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

        tokio::spawn(server_loop_for_unresponsive_text_protocol(LISTENING_INTERFACE.to_string(), PORT, server_shutdown_receiver,
            move |connection_event| {
                let server_disconnected = Arc::clone(&server_disconnected_ref);
                async move {
                    match connection_event {
                        ConnectionEvent::PeerDisconnected { .. } => server_disconnected.store(true, Relaxed),
                        _ => {}
                    }
                }
            },
            move |client_addr, client_port, peer: Arc<Peer<String>>, client_messages_stream: ProcessorRemoteStreamType<String>| client_messages_stream));

        // wait a bit for the server to start
        tokio::time::sleep(Duration::from_millis(10)).await;

        client_for_unresponsive_text_protocol(LISTENING_INTERFACE.to_string(), PORT, client_shutdown_receiver,
            move |connection_event| {
                let client_disconnected = Arc::clone(&client_disconnected_ref);
                async move {
                    match connection_event {
                        ConnectionEvent::PeerDisconnected { .. } => client_disconnected.store(true, Relaxed),
                        _ => {}
                    }
                }
            },
            move |client_addr, client_port, peer: Arc<Peer<String>>, server_messages_stream: ProcessorRemoteStreamType<String>| server_messages_stream).await.expect("Starting the client");

        // wait a bit for the connection, shutdown the client, wait another bit
        tokio::time::sleep(Duration::from_millis(100)).await;
        client_shutdown_sender.send(50).expect("sending client shutdown signal");
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(client_disconnected.load(Relaxed), "Client didn't drop the connection with the server");
        assert!(server_disconnected.load(Relaxed), "Server didn't notice the drop in the connection made by the client");

        // unneeded, but avoids error logs on the oneshot channel being dropped
        server_shutdown_sender.send(50).expect("sending server shutdown signal");
    }


        /// Test implementation for our text-only protocol
    impl SocketServerSerializer<String> for String {
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
    impl SocketServerDeserializer<String> for String {
        #[inline(always)]
        fn deserialize(message: &[u8]) -> Result<String, Box<dyn std::error::Error + Sync + Send + 'static>> {
            Ok(String::from_utf8_lossy(message).to_string())
        }
    }

}