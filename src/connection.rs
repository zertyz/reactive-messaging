//! Tokio version of the inspiring `message-io` crate, but improving it on the following (as of 2022-08):
//! 1) Tokio is used for async IO -- instead of something else `message-io` uses, which is out of Rust's async execution context;
//! 2) `message-io` has poor/no support for streams/textual protocols: it, eventually, breaks messages in overload scenarios,
//!    transforming 1 good message into 2 invalid ones -- its `FramedTCP` comm model is not subjected to that, for the length is prepended to the message;
//! 3) `message-io` is prone to DoS attacks: during flood tests, be it in Release or Debug mode, `message-io` answers to a single peer only -- the flooder.
//! 4) `message-io` uses ~3x more CPU and is single threaded -- throughput couldn't be measured for the network speed was the bottleneck; latency was not measured at all


use super::{
    types::AtomicChannel,
    serde::{SocketServerDeserializer, SocketServerSerializer},
};
use std::{cell::Cell, collections::VecDeque, fmt::Debug, future, future::Future, marker::PhantomData, net::SocketAddr, ops::Deref, sync::{Arc, atomic::{AtomicBool, AtomicU32, AtomicU8}, atomic::Ordering::Relaxed}, task::{Context, Poll, Waker}, time::Duration};
use std::fmt::Formatter;
use std::net::{IpAddr, Ipv4Addr};
use std::str::FromStr;
use futures::{StreamExt, Stream, stream, SinkExt};
use tokio::{
    io::{self,AsyncWriteExt, AsyncBufReadExt, BufReader, BufWriter, Interest},
    net::{TcpListener, TcpStream, tcp::WriteHalf},
    sync::Mutex,
};
use log::{trace, debug, warn, error};
use reactive_mutiny::prelude::advanced::{UniZeroCopyAtomic, ChannelCommon, ChannelUni, ChannelProducer, Instruments, ChannelUniZeroCopyAtomic, OgreUnique, AllocatorAtomicArray};
use reactive_mutiny::prelude::MutinyStream;
use crate::{ConnectionEvent, SenderUniType, SocketServer};


const CHAT_MSG_SIZE_HINT: usize = 2048;
/// How many messages may be produced ahead of sending, for each socket client
const SENDER_BUFFER_SIZE: usize = 1024;
/// How many messages may be received ahead of processing, for each socket
const RECEIVER_BUFFER_SIZE: usize = 1024;
/// Default executor instruments for processing the server-side logic due to each client message
const SOCKET_PROCESSOR_INSTRUMENTS: usize = Instruments::NoInstruments.into();
/// Timeout to wait for any last messages to be sent to the peer when a disconnection was commanded
const GRACEFUL_STREAM_ENDING_TIMEOUT_DURATION: Duration = Duration::from_millis(100);

// Uni types
type SocketProcessorUniType<MessagesType>     = UniZeroCopyAtomic<MessagesType, RECEIVER_BUFFER_SIZE, 1, SOCKET_PROCESSOR_INSTRUMENTS>;
type SocketProcessorChannelType<MessagesType> = ChannelUniZeroCopyAtomic<MessagesType, RECEIVER_BUFFER_SIZE, 1>;
type SocketProcessorDerivedType<MessagesType> = OgreUnique<MessagesType, AllocatorAtomicArray<MessagesType, RECEIVER_BUFFER_SIZE>>;

pub type ProcessorRemoteStreamType<MessagesType> = MutinyStream<'static, MessagesType, SocketProcessorChannelType<MessagesType>, SocketProcessorDerivedType<MessagesType>>;

/// Our `message-io` special version for '\n' separated textual protocols,
/// substituting the original crate with functional and performance improvements (see [self]).\
/// `shutdown_signaler` comes from a `tokio::sync::oneshot::channel`, which receives the maximum time, in
/// milliseconds, to wait for messages to flush before closing & to send the shutdown message to connected clients
pub async fn server_network_loop_for_text_protocol<ClientMessages:           SocketServerDeserializer<ClientMessages>              + Send + Sync + PartialEq + Debug + 'static,
                                                   ServerMessages:           SocketServerSerializer<ServerMessages>                + Send + Sync + PartialEq + Debug + 'static,
                                                   LocalStreamType:          Stream<Item=ServerMessages>                           + Send + 'static,
                                                   ConnectionEventsCallback: Fn(/*server_event: */ConnectionEvent<ServerMessages>) + Send + Sync + 'static,
                                                   ProcessorBuilderFn:       Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<ServerMessages>>, /*client_messages_stream: */ProcessorRemoteStreamType<ClientMessages>) -> LocalStreamType>

                                                  (listening_interface:         String,
                                                   listening_port:              u16,
                                                   mut shutdown_signaler:       tokio::sync::oneshot::Receiver<u32>,
                                                   connection_events_callback:  ConnectionEventsCallback,
                                                   dialog_processor_builder_fn: ProcessorBuilderFn)

                                                  -> Result<(), Box<dyn std::error::Error>> {

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
                        trace!("SocketServer: SHUTDOWN requested -- with timeout {}ms -- bailing out from the network loop", timeout_millis);
                        timeout_millis
                    },
                    Err(err) => {
                        error!("SocketServer: PROBLEM waiting for shutdown signal: {:?}", err);
                        error!("SocketServer: Shutting down anyway...");
                        5000    // see this as the "problematic shutdown timeout (millis) constant"
                    },
                };
                // issue the shutdown event
                connection_events_callback(ConnectionEvent::ApplicationShutdown);
                break
            }
        } {
            accepted_connection
        } else {
            // error accepting -- not fatal: try again
            continue
        };

        // prepares for the dialog to come with the (just accepted) connection
        let sender = AtomicChannel::<ServerMessages, SENDER_BUFFER_SIZE>::new(format!("Sender for client {addr}"));
        let (mut sender_stream, _) = Arc::clone(&sender).create_stream();
        let peer = Arc::new(Peer::new(sender, addr));
        let peer_ref1 = Arc::clone(&peer);
        let peer_ref2 = Arc::clone(&peer);

        // issue the connection event
        connection_events_callback(ConnectionEvent::PeerConnected {peer: peer.clone()});

        let (client_ip, client_port) = match addr {
            SocketAddr::V4(v4) => (v4.ip().to_string(), v4.port()),
            SocketAddr::V6(v6) => (v6.ip().to_string(), v6.port()),
        };

        let connection_events_callback_ref = Arc::clone(&connection_events_callback);
        let processor_sender = SocketProcessorUniType::<ClientMessages>::new(format!("Server processor for remote client {addr} @ {listening_interface}:{listening_port}"))
            .spawn_non_futures_non_fallibles_executors(1,
                                                       |in_stream| dialog_processor_builder_fn(client_ip.clone(), client_port, peer_ref1.clone(), in_stream),
                                                       move |executor| {
                                                           // issue the disconnect event
                                                           connection_events_callback_ref(ConnectionEvent::PeerDisconnected { peer: peer_ref2 });
                                                           future::ready(())
                                                       });

        // spawn a task to handle communications with that client
        tokio::spawn(connection_loop_for_textual_protocol(socket, peer, processor_sender));

    }

    debug!("SocketServer: bailing out of network loop -- we should be undergoing a shutdown...");

    Ok(())
}

pub async fn client_for_text_protocol<ClientMessages:           SocketServerSerializer<ClientMessages>                + Send + Sync + PartialEq + Debug + 'static,
                                      ServerMessages:           SocketServerDeserializer<ServerMessages>              + Send + Sync + PartialEq + Debug + 'static,
                                      RemoteStreamType:         Stream<Item=ServerMessages>                           + Send + 'static,
                                      LocalStreamType:          Stream<Item=ClientMessages>                           + Send + 'static,
                                      ConnectionEventsCallback: Fn(/*server_event: */ConnectionEvent<ClientMessages>) + Send + Sync + 'static,
                                      ProcessorBuilderFn:       Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<ClientMessages>>, /*server_messages_stream: */ProcessorRemoteStreamType<ServerMessages>) -> LocalStreamType>

                                     (server_ipv4_addr:            String,
                                      port:                        u16,
                                      mut shutdown_signaler:       tokio::sync::oneshot::Receiver<u32>,
                                      connection_events_callback:  ConnectionEventsCallback,
                                      dialog_processor_builder_fn: ProcessorBuilderFn)

                                      -> Result<(), Box<dyn std::error::Error>> {

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::from_str(server_ipv4_addr.as_str())?), port);
    let socket = TcpStream::connect(addr).await?;

    // prepares for the dialog to come with the (just accepted) connection
    let sender = AtomicChannel::<ClientMessages, SENDER_BUFFER_SIZE>::new(format!("Sender for client {addr}"));
    let peer = Arc::new(Peer::new(sender, addr));
    let peer_ref1 = Arc::clone(&peer);
    let peer_ref2 = Arc::clone(&peer);

    // issue the connection event
    connection_events_callback(ConnectionEvent::PeerConnected {peer: peer.clone()});

    let processor_sender = SocketProcessorUniType::<ServerMessages>::new(format!("Client Processor for remote server @ {addr}"))
        .spawn_non_futures_non_fallibles_executors(1,
                                                   |in_stream| dialog_processor_builder_fn(server_ipv4_addr.clone(), port, peer_ref1.clone(), in_stream),
                                                   move |executor| {
                                                       // issue the disconnect event
                                                       connection_events_callback(ConnectionEvent::PeerDisconnected { peer: peer_ref2 });
                                                       future::ready(())
                                                   });

    tokio::spawn(connection_loop_for_textual_protocol(socket, peer, processor_sender));

    Ok(())
}

/// connection (chat) loop (after the connection is set) usable either by clients & servers of the same protocol
/// TODO: a better name should be employed here, emphasizing this handles a single client and is the starting point where any connection state is set -- where `processor_builder_fn()` will be called.
async fn connection_loop_for_textual_protocol<RemoteMessages:    SocketServerDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug + 'static,
                                              LocalMessages:     SocketServerSerializer<LocalMessages>    + Send + Sync + PartialEq + Debug + 'static>
                                             (mut socket:        TcpStream,
                                              peer:              Arc<Peer<LocalMessages>>,
                                              processor_sender:  Arc<SocketProcessorUniType<RemoteMessages>>) {

    let mut read_buffer = Vec::with_capacity(CHAT_MSG_SIZE_HINT);
    socket.set_nodelay(true).expect("setting nodelay() for the socket");
    socket.set_ttl(30).expect("setting ttl(30) for the socket");

    let (mut sender_stream, _) = peer.sender.create_stream();

    'connection: loop {
        // wait for the socket to be readable or until we have something to write
        tokio::select!(

            biased;     // sending has priority over receiving

            // send?
            result = sender_stream.next() => {
                match result {
                    Some(to_send_message) => {
                        let to_send_text = LocalMessages::ss_serialize(&to_send_message);
                        if let Err(err) = socket.write_all(to_send_text.as_bytes()).await {
                            warn!("SocketServer::TokioMessageIO: PROBLEM in the connection with {:#?} while WRITING: '{:?}' -- dropping it", peer, err);
                            peer.sender.cancel_all_streams();
                            break 'connection
                        }
                        // sending was complete. Was it a "disconnect" message?
                        if LocalMessages::is_disconnect_message(&to_send_message) {
                            peer.sender.gracefully_end_all_streams(GRACEFUL_STREAM_ENDING_TIMEOUT_DURATION);
                            break 'connection
                        }
                    },
                    None => {
                        warn!("SocketServer::TokioMessageIO: Sender for {:#?} ended (most likely, .cancel_all_streams() was called on the `peer` by the processor.", peer);
                        break 'connection
                    }
                }
            },
            // read?
            _ = socket.readable() => {
                match socket.try_read_buf(&mut read_buffer) {
                    Ok(0) => {
                        warn!("SocketServer::TokioMessageIO: PROBLEM with reading from {:?} (peer id {}) -- it is out of bytes! Dropping the connection", peer.peer_address, peer.peer_id);
                        peer.sender.cancel_all_streams();
                        break 'connection
                    },
                    Ok(n) => {
                        let mut next_line_index = 0;
                        let mut search_start = read_buffer.len() - n;
                        loop {
                            if let Some(mut eol_pos) = read_buffer[next_line_index+search_start..].iter().position(|&b| b == '\n' as u8) {
                                eol_pos += next_line_index+search_start;
                                let line_bytes = &read_buffer[next_line_index..eol_pos];
                                match RemoteMessages::ss_deserialize(&line_bytes) {
                                    Ok(client_message) => {
                                        if !processor_sender.try_send(|slot| *slot = client_message) {
                                            // breach in protocol: we cannot process further messages (they were sent too fast)
                                            let msg = format!("Client message ignored: server is too busy -- `dialog_processor` is full of unprocessed messages ({}/{}) while attempting to enqueue another message",
                                                                     processor_sender.channel.pending_items_count(), processor_sender.channel.buffer_size());
                                            peer.sender.send(|slot| *slot = LocalMessages::processor_error_message(msg.clone()));
                                            error!("SocketServer: {}", msg);
                                        }
                                    },
                                    Err(err) => {
                                        let stripped_line = String::from_utf8_lossy(line_bytes);
                                        let error_message = format!("Unknown command received from {:?} (peer id {}): '{}'",
                                                                           peer.peer_address, peer.peer_id, stripped_line);
                                        debug!("SocketServer: {error_message}");
                                        let outgoing_error = LocalMessages::processor_error_message(error_message);
                                        let sent = peer.sender.try_send(|slot| *slot = outgoing_error);
                                        if !sent {
                                            // prevent bad clients from depleting our resources: if the out buffer is full due to bad input, we'll wait 1 sec to process the next message
                                            tokio::time::sleep(Duration::from_millis(1000)).await;
                                        }
                                    }
                                }
                                next_line_index = eol_pos + 1;
                                if next_line_index >= read_buffer.len() {
                                    next_line_index = read_buffer.len();
                                    break
                                }
                                search_start = 0;
                            } else {
                                break
                            }
                        }
                        if next_line_index > 0 {
                            read_buffer.drain(0..next_line_index);
                        }
                    },
                    Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {},
                    Err(err) => {
                        error!("SocketServer::TokioMessageIO: ERROR in the connection with {:?} (peer id {}) while READING: '{:?}' -- dropping it", peer.peer_address, peer.peer_id, err);
                        break 'connection
                    },
                }
            },
        );

    }
}

static PEER_COUNTER: AtomicU32 = AtomicU32::new(0);
pub type PeerId = u32;
/// Represents a channel to a peer able to send out `MessageType` kinds of messages from "here" to "there"
pub struct Peer<MessagesType: 'static + Send + Sync + PartialEq + Debug + SocketServerSerializer<MessagesType>> {
    pub peer_id:      PeerId,
    pub sender:       Arc<AtomicChannel<MessagesType, SENDER_BUFFER_SIZE>>,
    pub peer_address: SocketAddr,
}

impl<MessagesType: 'static + Send + Sync + PartialEq + Debug + SocketServerSerializer<MessagesType>>
Peer<MessagesType> {

    pub fn new(sender: Arc<AtomicChannel<MessagesType, SENDER_BUFFER_SIZE>>, peer_address: SocketAddr) -> Self {
        Self {
            peer_id: PEER_COUNTER.fetch_add(1, Relaxed),
            sender,
            peer_address,
        }
    }

}

impl<MessagesType: 'static + Send + Sync + PartialEq + Debug + SocketServerSerializer<MessagesType>>
Debug
for Peer<MessagesType> {
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


    /// assures connection & dialogs work
    #[cfg_attr(not(doc),tokio::test)]
    async fn connect_and_disconnect() {
        let client_secret = format!("open, sesame");
        let server_secret = format!("now the 40 of you may enter");
        let observed_secret = Arc::new(Mutex::new(None));

        let (server_shutdown_sender, server_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (_client_shutdown_sender, client_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();

        // server
        tokio::spawn(server_network_loop_for_text_protocol("127.0.0.1".to_string(), 8570, server_shutdown_receiver,
           |connection_event| {
               match connection_event {
                   ConnectionEvent::PeerConnected { peer } => {
                       peer.sender.try_send_movable(format!("Welcome! State your business!\n"));
                   },
                   ConnectionEvent::PeerDisconnected { peer } => {},
                   ConnectionEvent::ApplicationShutdown => {
                       println!("Server: shutdown was requested... No connection will receive the drop message (nor even will be dropped) because I didn't keep track of the connected peers!");
                   }
               }
           },
           |client_addr, client_port, peer, client_messages_stream: ProcessorRemoteStreamType<String>| {
               client_messages_stream.map(|client_message| {
                   peer.sender.try_send_movable(format!("Client just sent '{}'\n", client_message));
                   if client_message == client_secret {
                       peer.sender.try_send_movable(format!("{}\n", server_secret));
                   } else {
                       panic!("Client sent the wrong secret: '{}' -- I was expecting '{}'", client_message, client_secret);
                   }
               })
            }));

        println!("### Waiting a little for the server to start...");
        tokio::time::sleep(Duration::from_millis(10)).await;

        // client
        let observed_secret_ref = Arc::clone(&observed_secret);
        client_for_text_protocol(format!("127.0.0.1"), 8570, client_shutdown_receiver,
            |connection_event| {
                match connection_event {
                    ConnectionEvent::PeerConnected { peer } => {
                        peer.sender.try_send_movable(format!("{}\n", client_secret));
                    },
                    ConnectionEvent::PeerDisconnected { peer } => {
                        println!("Client: connection with {} (peer_id #{}) was dropped -- should not happen in this test", peer.peer_address, peer.peer_id);
                    },
                    ConnectionEvent::ApplicationShutdown => {}
                }
            },
            move |client_addr, client_port, peer, server_messages_stream| {
                let observed_secret_ref = Arc::clone(&observed_secret_ref);
                server_messages_stream.inspect(|server_message| async {
                    println!("Server said: '{}'", server_message);
                    let _ = observed_secret_ref.lock().await.insert(server_message);
                })
            }).await.expect("Starting the client");
        println!("### Client is running concurrently, in the background...");

        tokio::time::sleep(Duration::from_millis(500)).await;
        server_shutdown_sender.send(500).expect("sending shutdown signal");

        println!("### Waiting a little for the shutdown signal to reach the server...");
        tokio::time::sleep(Duration::from_millis(10)).await;

        assert_eq!(*observed_secret.lock().await, Some(server_secret.to_string()), "Communications didn't go according the plan");
    }
/*
    /// Assures the minimum acceptable latency values -- either for Debug & Release modes.\
    /// One sends Ping(n); the other receives it and send Pong(n); the first receives it and sends Ping(n+1) and so on...\
    /// Latency is computed dividing the number of seconds per n*2 (we care about the server leg of the latency, while here we measure the round trip client<-->server)
    #[cfg_attr(not(doc),tokio::test)]
    async fn latency_measurements() {
        const TEST_DURATION_MS: u64 = 2000;
        const TEST_DURATION_NS: u64 = TEST_DURATION_MS * 1e6 as u64;
        let (server_shutdown_sender, server_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (_client_shutdown_sender, client_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();

        // server
        tokio::spawn(server_network_loop_for_text_protocol("127.0.0.1".to_string(), 8571, server_shutdown_receiver, |event: SocketEvent<String, String>| async {
            match event {
                SocketEvent::Incoming { peer, message: client_message } => {
                    // Message received: Ping(n)
                    // Answer: Pong(n)
                    let n_str = &client_message[5..(client_message.len()-1)];
                    let n = str::parse::<u32>(n_str).expect(&format!("could not convert '{}' to number. Original message: '{}'", n_str, client_message));
                    peer.sender.try_send_movable(format!("Pong({})\n", n));
                },
                SocketEvent::Connected { .. } => {},
                SocketEvent::Disconnected { .. } => {},
                SocketEvent::Shutdown { .. } => {},
            }
        }));

        println!("### Waiting a little for the server to start...");
        tokio::time::sleep(Duration::from_millis(10)).await;

        let counter = Arc::new(AtomicU32::new(0));
        let counter_ref = Arc::clone(&counter);
        // client
        client_for_text_protocol("127.0.0.1", 8571, client_shutdown_receiver, move |event: SocketEvent<String, String>| {
            let counter_ref = Arc::clone(&counter_ref);
            async move {
                match event {
                    SocketEvent::Incoming { peer, message: server_message } => {
                        // Message received: Pong(n)
                        // Answer: Ping(n+1)
                        let n_str = &server_message[5..(server_message.len()-1)];
                        let n = str::parse::<u32>(n_str).expect(&format!("could not convert '{}' to number. Original message: '{}'", n_str, server_message));
                        let current_count = counter_ref.fetch_add(1, Relaxed);
                        if n != current_count {
                            panic!("Received '{}', where Client was expecting 'Pong({})'", server_message, current_count);
                        }
                        peer.sender.try_send_movable(format!("Ping({})\n", current_count+1));
                    },
                    SocketEvent::Connected { peer } => {
                        peer.sender.try_send_movable(format!("Ping(0)\n"));
                    },
                    SocketEvent::Disconnected { .. } => {},
                    SocketEvent::Shutdown { .. } => {},
                }
            }
        }).await.expect("Starting the client");
        println!("### Measuring latency for 2 seconds...");

        tokio::time::sleep(Duration::from_millis(TEST_DURATION_MS)).await;
        server_shutdown_sender.send(500).expect("sending shutdown signal");

        println!("### Waiting a little for the shutdown signal to reach the server...");
        tokio::time::sleep(Duration::from_millis(10)).await;

        let counter = counter.load(Relaxed);
        let latency = Duration::from_nanos((TEST_DURATION_NS as f64 / (2.0 * counter as f64)) as u64);
        println!("Round trips counter: {}", counter);
        println!("Measured latency: {:?} ({})", latency, if DEBUG {"Debug mode"} else {"Release mode"});
        if DEBUG {
            assert!(counter > 13000, "Latency regression detected: we used to make 23273 round trips in 2 seconds (Debug mode) -- now only {} were made", counter);
        } else {
            assert!(counter > 290000, "Latency regression detected: we used to make 308955 round trips in 2 seconds (Release mode) -- now only {} were made", counter);

        }
    }

    /// When a client floods the server with messages, it should, at most, screw just that client up... or, maybe, not even that!\
    /// This test works like the latency test, but we don't wait for the answer to come to send another one -- we just do it like crazy\
    /// (currently, as of 2022-10-29, a flooding client won't have all its messages processed, for some reason that should be still investigated and improved upon)
    #[cfg_attr(not(doc),tokio::test(flavor = "multi_thread"))]
    async fn message_flooding_throughput_test() {
        const TEST_DURATION_MS: u64 = 2000;
        const TEST_DURATION_NS: u64 = TEST_DURATION_MS * 1e6 as u64;
        let (server_shutdown_sender, server_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (_client_shutdown_sender, client_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();

        // server -- do not answer to (flood) messages (just parses & counts them, making sure they are received in the right order)
        // message format is "DoNotAnswer(n)", where n should be sent by the client in natural order, starting from 0
        let received_messages_count = Arc::new(AtomicU32::new(0));
        let unordered = Arc::new(AtomicU32::new(0));    // if non-zero, will contain the last message received before the ordering went kaputt
        let received_messages_count_ref = Arc::clone(&received_messages_count);
        let unordered_ref = Arc::clone(&unordered);
        tokio::spawn(server_network_loop_for_text_protocol("127.0.0.1".to_string(), 8572, server_shutdown_receiver, move |event: SocketEvent<String, String>| {
            let received_messages_count = Arc::clone(&received_messages_count_ref);
            let unordered = Arc::clone(&unordered_ref);
            async move {
                match event {
                    SocketEvent::Incoming { peer, message: client_message } => {
                        // Message format: DoNotAnswer(n)
                        let n_str = &client_message[12..(client_message.len()-1)];
                        let n = str::parse::<u32>(n_str).expect(&format!("could not convert '{}' to number. Original message: '{}'", n_str, client_message));
                        let count = received_messages_count.fetch_add(1, Relaxed);
                        if count != n {
                            if unordered.compare_exchange(0, count, Relaxed, Relaxed).is_ok() {
                                println!("Server: ERROR: received order of messages broke at message #{}", count);
                            };
                        }
                    },
                    SocketEvent::Connected { .. } => {},
                    SocketEvent::Disconnected { .. } => {},
                    SocketEvent::Shutdown { .. } => {},
                }
            }
        }));

        println!("### Waiting a little for the server to start...");
        tokio::time::sleep(Duration::from_millis(10)).await;

        // client
        let sent_messages_count = Arc::new(AtomicU32::new(0));
        let sent_messages_count_ref = Arc::clone(&sent_messages_count);
        client_for_text_protocol("127.0.0.1", 8572, client_shutdown_receiver, move |event: SocketEvent<String, String>| {
            let sent_messages_count = Arc::clone(&sent_messages_count_ref);
            async move {
                let sent_messages_count = Arc::clone(&sent_messages_count);
                match event {
                    SocketEvent::Incoming { peer, message: server_message } => {},
                    SocketEvent::Connected { peer } => {
                        tokio::spawn(async move {
                            let start = SystemTime::now();
                            let mut n = 0;
                            loop {
                                peer.sender.try_send_movable(format!("DoNotAnswer({})\n", n));
                                n += 1;
                                // flush & bailout check for timeout every 1024 messages
                                if n % (1<<10) == 0 {
                                    peer.sender.flush(Duration::from_millis(100)).await;
                                    if start.elapsed().unwrap().as_millis() as u64 >= TEST_DURATION_MS  {
                                        println!("Client sent {} messages before bailing out", n);
                                        sent_messages_count.store(n, Relaxed);
                                        break;
                                    }
                                }
                            }
                        });
                    },
                    SocketEvent::Disconnected { .. } => {},
                    SocketEvent::Shutdown { .. } => {},
                }
            }
        }).await.expect("Starting the client");
        println!("### Measuring latency for 2 seconds...");

        tokio::time::sleep(Duration::from_millis(TEST_DURATION_MS)).await;
        server_shutdown_sender.send(500).expect("sending shutdown signal");

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
            assert!(received_messages_count > 100000, "Client flooding throughput regression detected: we used to send/receive 217088 flood messages in this test (Debug mode) -- now only {} were made", received_messages_count);
        } else {
            assert!(received_messages_count > 300000, "Client flooding throughput regression detected: we used to send/receive 477184 flood messages in this test (Release mode) -- now only {} were made", received_messages_count);

        }

    }
*/

    /// Test implementation for our text-only protocol
    impl SocketServerSerializer<String> for String {

        fn ss_serialize(message: &String) -> String {
            message.clone()
        }
        fn processor_error_message(err: String) -> String {
            let msg = format!("ServerBug! Please, fix! Error: {}", err);
            panic!("SocketServerSerializer<String>::processor_error_message(): {}", msg);
            // msg
        }
        fn is_disconnect_message(processor_answer: &String) -> bool {
            // for String communications, an empty line sent by the messages processor signals that the connection should be closed
            processor_answer.is_empty()
        }

        fn is_no_answer_message(processor_answer: &String) -> bool {
            processor_answer == "."
        }
    }

    /// Testable implementation for our text-only protocol
    impl SocketServerDeserializer<String> for String {
        fn ss_deserialize(message: &[u8]) -> Result<String, Box<dyn std::error::Error + Sync + Send>> {
            // TODO consider using the generic Cow<'a, str> instead of String, for increased performance allowing conversions not to take place...
            Ok(String::from_utf8_lossy(message).to_string())
        }
    }

}