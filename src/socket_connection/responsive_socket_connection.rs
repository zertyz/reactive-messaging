//! Common code for responsive reactive clients and servers

use std::fmt::Debug;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;
use futures::{Stream,StreamExt};
use log::{error, trace, warn};
use reactive_mutiny::prelude::GenericUni;
use crate::{ReactiveMessagingDeserializer, ReactiveMessagingSerializer};
use crate::socket_connection::common::RetryableSender;
use crate::socket_connection::{Peer, UnresponsiveSocketConnectionHandler};
use crate::types::{ConnectionEvent, MessagingMutinyStream, ResponsiveMessages};

// Contains abstractions, useful for unresponsive clients and servers, for dealing with socket connections handled by Stream Processors:\
//   - handles all combinations of the Stream's output types returned by `dialog_processor_builder_fn()`: futures/non-future & fallible/non-fallible;
//   - handles unresponsive / responsive output types (also issued by the Streams created by `dialog_processor_builder_fn()`).
pub struct ResponsiveSocketConnectionHandler<const CONFIG: usize,
                                             RemoteMessagesType:  ReactiveMessagingDeserializer<RemoteMessagesType>  + Send + Sync + PartialEq + Debug + 'static,
                                             LocalMessagesType:   ResponsiveMessages<LocalMessagesType>              +
                                                                  ReactiveMessagingSerializer<LocalMessagesType>     + Send + Sync + PartialEq + Debug + 'static,
                                             ProcessorUniType:    GenericUni<ItemType=RemoteMessagesType>            + Send + Sync                     + 'static,
                                             RetryableSenderImpl: RetryableSender<LocalMessages = LocalMessagesType> + Send + Sync                     + 'static> {
    _phantom: PhantomData<(RemoteMessagesType, LocalMessagesType, ProcessorUniType, RetryableSenderImpl)>,
}

impl<const CONFIG: usize,
     RemoteMessagesType:  ReactiveMessagingDeserializer<RemoteMessagesType>  + Send + Sync + PartialEq + Debug + 'static,
     LocalMessagesType:   ResponsiveMessages<LocalMessagesType>              +
                          ReactiveMessagingSerializer<LocalMessagesType>     + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:    GenericUni<ItemType=RemoteMessagesType>            + Send + Sync                     + 'static,
     RetryableSenderImpl: RetryableSender<LocalMessages = LocalMessagesType> + Send + Sync                     + 'static>
ResponsiveSocketConnectionHandler<CONFIG, RemoteMessagesType, LocalMessagesType, ProcessorUniType, RetryableSenderImpl> {
    pub fn new() -> Self {
        Self {
            _phantom: PhantomData,
        }
    }

    /// Similar to [UnresponsiveSocketConnectionHandle::server_loop_for_unresponsive_text_protocol()], but for when the provided `dialog_processor_builder_fn()` produces
    /// answers of type `ServerMessages`, which will be automatically routed to the clients.
    #[inline(always)]
    pub async fn server_loop_for_responsive_text_protocol<OutputStreamType:               Stream<Item=LocalMessagesType>                                                                                                                                                                        + Send        + 'static,
                                                          ConnectionEventsCallbackFuture: Future<Output=()>                                                                                                                                                                                     + Send,
                                                          ConnectionEventsCallback:       Fn(/*server_event: */ConnectionEvent<RetryableSenderImpl>)                                                                                                          -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                                          ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<RetryableSenderImpl>>, /*client_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> OutputStreamType               + Send + Sync + 'static>

                                                         (self,
                                                          listening_interface:         String,
                                                          listening_port:              u16,
                                                          shutdown_signaler:           tokio::sync::oneshot::Receiver<u32>,
                                                          connection_events_callback:  ConnectionEventsCallback,
                                                          dialog_processor_builder_fn: ProcessorBuilderFn)

                                                         -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

        // upgrade the "unresponsive" streams from `dialog_processor_builder_fn()` to "responsive" streams that will send the outcome to clients
       let dialog_processor_builder_fn = move |client_addr, connected_port, peer, client_messages_stream| {
           let dialog_processor_stream = dialog_processor_builder_fn(client_addr, connected_port, Arc::clone(&peer), client_messages_stream);
           self.to_responsive_stream(peer, dialog_processor_stream)
       };
        let unresponsive_socket_connection_handler = UnresponsiveSocketConnectionHandler::<CONFIG, RemoteMessagesType, LocalMessagesType, ProcessorUniType, RetryableSenderImpl>::new();
        unresponsive_socket_connection_handler.server_loop_for_unresponsive_text_protocol(listening_interface.clone(), listening_port, shutdown_signaler, connection_events_callback, dialog_processor_builder_fn).await
            .map_err(|err| Box::from(format!("error when starting server @ {listening_interface}:{listening_port} with `server_loop_for_unresponsive_text_protocol()`: {err}")))
    }

    /// Similar to [client_for_unresponsive_text_protocol()], but for when the provided `dialog_processor_builder_fn()` produces
    /// answers of type `ClientMessages`, which will be automatically routed to the server.
    #[inline(always)]
    pub async fn client_for_responsive_text_protocol<OutputStreamType:               Stream<Item=LocalMessagesType>                                                                                                                                                                        + Send +        'static,
                                                     ConnectionEventsCallbackFuture: Future<Output=()>                                                                                                                                                                                     + Send,
                                                     ConnectionEventsCallback:       Fn(/*server_event: */ConnectionEvent<RetryableSenderImpl>)                                                                                                          -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                                     ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<RetryableSenderImpl>>, /*server_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> OutputStreamType>

                                                    (self,
                                                     server_ipv4_addr:            String,
                                                     port:                        u16,
                                                     shutdown_signaler:           tokio::sync::oneshot::Receiver<u32>,
                                                     connection_events_callback:  ConnectionEventsCallback,
                                                     dialog_processor_builder_fn: ProcessorBuilderFn)

                                                    -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

       // upgrade the "unresponsive" streams from `dialog_processor_builder_fn()` to "responsive" streams that will send the outcome to clients
       let dialog_processor_builder_fn = |server_addr, port, peer, server_messages_stream| {
           let dialog_processor_stream = dialog_processor_builder_fn(server_addr, port, Arc::clone(&peer), server_messages_stream);
           self.to_responsive_stream(peer, dialog_processor_stream)
       };
        let unresponsive_socket_connection_handler = UnresponsiveSocketConnectionHandler::<CONFIG, RemoteMessagesType, LocalMessagesType, ProcessorUniType, RetryableSenderImpl>::new();
        unresponsive_socket_connection_handler.client_for_unresponsive_text_protocol(server_ipv4_addr.clone(), port, shutdown_signaler, connection_events_callback, dialog_processor_builder_fn).await
            .map_err(|err| Box::from(format!("error when starting client for server @ {server_ipv4_addr}:{port} with `client_for_unresponsive_text_protocol()`: {err}")))
    }

    /// upgrades the `request_processor_stream` (of non-fallible & non-future items) to a `Stream` which is also able send answers back to the `peer`
    #[inline(always)]
    fn to_responsive_stream(&self,
                            peer:                     Arc<Peer<RetryableSenderImpl>>,
                            request_processor_stream: impl Stream<Item=LocalMessagesType>)

                           -> impl Stream<Item = ()> {

        request_processor_stream
            .map(move |outgoing| {
                // wired `if`s ahead to try to get some branch prediction optimizations -- assuming Rust will see the first `if` branches as the "usual case"
                let is_disconnect = LocalMessagesType::is_disconnect_message(&outgoing);
                let is_no_answer = LocalMessagesType::is_no_answer_message(&outgoing);
                // send the message, skipping messages that are programmed not to generate any response
                if !is_disconnect && !is_no_answer {
                    // send the answer
                    trace!("reactive-messaging: Sending Answer `{:?}` to {:?} (peer id {})", outgoing, peer.peer_address, peer.peer_id);
                    if let Err((abort, error_msg)) = peer.send(outgoing) {
                        // peer is slow-reading -- and, possibly, fast sending
                        warn!("reactive-messaging: Slow reader detected while sending to {:?}: {error_msg}", peer);
                        if abort {
                            peer.cancel_and_close();
                        }
                    }
                } else if is_disconnect {
                    trace!("reactive-messaging: SocketServer: processor choose to drop connection with {} (peer id {}): '{:?}'", peer.peer_address, peer.peer_id, outgoing);
                    if !is_no_answer {
                        if let Err((abort, error_msg)) = peer.send(outgoing) {
                            warn!("reactive-messaging: Slow reader detected while sending the closing message to {:?}: {error_msg}", peer);
                        }
                    }
                    peer.cancel_and_close();
                }
            })
    }

    /// upgrades the `request_processor_stream` (of fallible & non-future items) to a `Stream` which is also able send answers back to the `peer`
    #[inline(always)]
    fn _to_responsive_stream_of_fallibles(&self,
                                         peer:                     Arc<Peer<RetryableSenderImpl>>,
                                         request_processor_stream: impl Stream<Item = Result<LocalMessagesType, Box<dyn std::error::Error + Sync + Send>>>)

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
    use futures::stream::{self,StreamExt};
    use tokio::sync::Mutex;

    use super::*;
    use std::{time::SystemTime, sync::atomic::AtomicBool, future};
    use std::time::Duration;
    use reactive_mutiny::prelude::advanced::{UniZeroCopyAtomic, ChannelUniMoveAtomic, ChannelUniZeroCopyAtomic};
    use reactive_mutiny::prelude::Instruments;
    use crate::config::ConstConfig;
    use crate::socket_connection::common::ReactiveMessagingSender;

    #[cfg(debug_assertions)]
    const DEBUG: bool = true;
    #[cfg(not(debug_assertions))]
    const DEBUG: bool = false;

    const DEFAULT_TEST_CONFIG: ConstConfig           = ConstConfig::default();
    const DEFAULT_TEST_CONFIG_USIZE: usize           = DEFAULT_TEST_CONFIG.into();
    const DEFAULT_TEST_UNI_INSTRUMENTS: usize        = DEFAULT_TEST_CONFIG.executor_instruments.into();
    type DefaultTestUni<PayloadType = String>        = UniZeroCopyAtomic<PayloadType, {DEFAULT_TEST_CONFIG.receiver_buffer as usize}, 1, DEFAULT_TEST_UNI_INSTRUMENTS>;
    type RetryableSenderImpl<PayloadType = String> = ReactiveMessagingSender<DEFAULT_TEST_CONFIG_USIZE, PayloadType, ChannelUniMoveAtomic<PayloadType, {DEFAULT_TEST_CONFIG.sender_buffer as usize}, 1>>;


    #[ctor::ctor]
    fn suite_setup() {
        simple_logger::SimpleLogger::new().with_utc_timestamps().init().unwrap_or_else(|_| eprintln!("--> LOGGER WAS ALREADY STARTED"));
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
        let socket_connection_handler = ResponsiveSocketConnectionHandler::<DEFAULT_TEST_CONFIG_USIZE, String, String, DefaultTestUni, RetryableSenderImpl>::new();
        socket_connection_handler.server_loop_for_responsive_text_protocol(LISTENING_INTERFACE.to_string(), PORT, server_shutdown_receiver,
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
        ).await.expect("Starting the server");

        println!("### Waiting a little for the server to start...");
        tokio::time::sleep(Duration::from_millis(10)).await;

        // client
        let client_secret = client_secret.clone();
        let observed_secret_ref = Arc::clone(&observed_secret);
        let socket_connection_handler = ResponsiveSocketConnectionHandler::<DEFAULT_TEST_CONFIG_USIZE, String, String, DefaultTestUni, RetryableSenderImpl>::new();
        socket_connection_handler.client_for_responsive_text_protocol(LISTENING_INTERFACE.to_string(), PORT, client_shutdown_receiver,
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
}