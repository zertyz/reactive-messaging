//! Common & low-level code for reactive clients and servers


use crate::{
    config::*,
    types::*,
    socket_connection::{
        common::{ReactiveMessagingSender, ReactiveMessagingUniSender, upgrade_processor_uni_retrying_logic},
        peer::Peer,
    },
};
use std::{error::Error, fmt::Debug, future::Future, net::SocketAddr, sync::Arc, time::Duration};
use reactive_mutiny::prelude::advanced::GenericUni;
use futures::Stream;
use tokio::io::AsyncWriteExt;
use log::{trace, debug, warn, error};
use crate::socket_connection::connection::SocketConnection;
use crate::socket_connection::socket_dialog::dialog_types::SocketDialog;

/// Contains abstractions, useful for clients and servers, for dealing with socket connections handled by Stream Processors:\
///   - handles all combinations of the Stream's output types returned by `dialog_processor_builder_fn()`: futures/non-future & fallible/non-fallible;
///   - handles unresponsive / responsive output types (also issued by the Streams created by `dialog_processor_builder_fn()`).
pub struct SocketConnectionHandler<const CONFIG:        u64,
                                   SocketDialogType:    SocketDialog<CONFIG> + Send + Sync + 'static> {
    _socket_dialog: SocketDialogType,
}

impl<const CONFIG:        u64,
     SocketDialogType:    SocketDialog<CONFIG> + Send + Sync + 'static>
 SocketConnectionHandler<CONFIG, SocketDialogType> {

    const CONST_CONFIG: ConstConfig = ConstConfig::from(CONFIG);


    pub fn new(_socket_dialog: SocketDialogType) -> Self {
        Self {
            _socket_dialog,
        }
    }

    /// Serves textual protocols whose events/commands/sentences are separated by '\n':
    ///   - `dialog_processor_builder_fn()` builds streams that will receive `ClientMessages` but will generate no answers;
    ///   - sending messages back to the client will be done explicitly by `peer`;
    ///   - `shutdown_signaler` comes from a `tokio::sync::oneshot::channel`, which receives the maximum time, in
    ///     milliseconds: it will be passed to `connection_events_callback` and cause the server loop to exit.
    #[inline(always)]
    pub async fn server_loop<PipelineOutputType:                                                                                                                                                                                                                                                                                                                 Send + Sync + Debug + 'static,
                             OutputStreamType:               Stream<Item=PipelineOutputType>                                                                                                                                                                                                                                                                   + Send                + 'static,
                             ConnectionEventsCallbackFuture: Future<Output=()>                                                                                                                                                                                                                                                                                 + Send,
                             ConnectionEventsCallback:       Fn(/*server_event: */ProtocolEvent<CONFIG, SocketDialogType::LocalMessages, SocketDialogType::SenderChannel, SocketDialogType::State>)                                                                                                                          -> ConnectionEventsCallbackFuture + Send + Sync         + 'static,
                             ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<CONFIG, SocketDialogType::LocalMessages, SocketDialogType::SenderChannel, SocketDialogType::State>>, /*client_messages_stream: */MessagingMutinyStream<SocketDialogType::ProcessorUni>) -> OutputStreamType               + Send + Sync         + 'static>

                            (self,
                             listening_interface:         &str,
                             listening_port:              u16,
                             mut connection_source:       tokio::sync::mpsc::Receiver<SocketConnection<SocketDialogType::State>>,
                             connection_sink:             tokio::sync::mpsc::Sender<SocketConnection<SocketDialogType::State>>,
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
                let sender = ReactiveMessagingSender::<CONFIG, SocketDialogType::LocalMessages, SocketDialogType::SenderChannel>::new(format!("Sender for client {addr}"));
                let peer = Arc::new(Peer::new(sender, addr, &socket_connection));//       THIS LINE IS HANGING!!
                let peer_ref1 = Arc::clone(&peer);
                let peer_ref2 = Arc::clone(&peer);
                let peer_ref3 = Arc::clone(&peer);

                // issue the connection event
                connection_events_callback(ProtocolEvent::PeerArrived {peer: peer.clone()}).await;

                let connection_events_callback_ref = Arc::clone(&connection_events_callback);
                let processor_sender = SocketDialogType::ProcessorUni::new(format!("Server processor for remote client {addr} @ {listening_interface_and_port}"))
                    .spawn_non_futures_non_fallibles_executors(1,
                                                               |in_stream| dialog_processor_builder_fn(client_ip.clone(), client_port, peer_ref1.clone(), in_stream),
                                                               move |executor| async move {
                                                                   let execution_duration = Duration::from_nanos(executor.execution_finish_delta_nanos() - executor.execution_start_delta_nanos());
                                                                   let executor_name = executor.executor_name().clone();
                                                                   // issue the async disconnect event when the `Stream` ends
                                                                   connection_events_callback_ref(ProtocolEvent::PeerLeft { peer: peer_ref2, stream_stats: executor }).await;
                                                                   // ensure the related peer is also closed
                                                                   let flush_timeout_millis = Self::CONST_CONFIG.flush_timeout_millis as u64;
                                                                   let unsent_messages = peer_ref3.flush_and_close(Duration::from_millis(flush_timeout_millis)).await;
                                                                   if unsent_messages > 0 {
                                                                       warn!("{executor_name} (in execution for {execution_duration:?}) left with {unsent_messages} unsent messages to {peer_ref3:?}, even after flushing with a timeout of {flush_timeout_millis}ms after, probably, its `Stream` being dropped. Consider increasing `CONST_CONFIG.flush_timeout_millis` or revisiting the processor logic.");
                                                                   }
                                                               });
                let processor_sender = upgrade_processor_uni_retrying_logic::<CONFIG, SocketDialogType::DeserializedRemoteMessages, <<SocketDialogType as SocketDialog<CONFIG>>::ProcessorUni as GenericUni>::DerivedItemType, SocketDialogType::ProcessorUni>
                                                                                                        (processor_sender);
                // spawn a task to handle communications with that client
                let cloned_self = Arc::clone(&arc_self);
                let listening_interface_and_port = listening_interface_and_port.clone();
                let cloned_connection_sink = Arc::clone(&connection_sink);
                tokio::spawn(tokio::task::unconstrained(async move {
                    let socket_connection = match cloned_self.dialog_loop(socket_connection, peer.clone(), processor_sender).await {
                        Ok(socket_connection) => socket_connection,
                        Err(err) => {
                            error!("`reactive-messaging::SocketServer`: ERROR in server @ {listening_interface_and_port} while starting the dialog with client {client_ip}:{client_port}: {err}");
                            return
                        }
                    };
                    if let Err(err) = cloned_connection_sink.send(socket_connection).await {
                        error!("`reactive-messaging::SocketServer`: ERROR in server @ {listening_interface_and_port} while returning the connection with {client_ip}:{client_port} to the caller. It will now be forcibly closed: {err}");
                    };
                    // if let Err(err) = connection.shutdown().await {
                    //     error!("`reactive-messaging::SocketServer`: ERROR in server @ {listening_interface_and_port} while shutting down the socket with client {client_ip}:{client_port}: {err}");
                    // }
                }));
            }
            debug!("`reactive-messaging::SocketServer`: bailing out of network loop -- we should be undergoing a shutdown...");
            // issue the shutdown event
            connection_events_callback(ProtocolEvent::LocalServiceTermination).await;
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
    pub async fn client<PipelineOutputType:                                                                                                                                                                                                                                                                                                                 Send + Sync + Debug + 'static,
                        OutputStreamType:               Stream<Item=PipelineOutputType>                                                                                                                                                                                                                                                                   + Send                + 'static,
                        ConnectionEventsCallbackFuture: Future<Output=()>                                                                                                                                                                                                                                                                                 + Send,
                        ConnectionEventsCallback:       Fn(/*server_event: */ProtocolEvent<CONFIG, SocketDialogType::LocalMessages, SocketDialogType::SenderChannel, SocketDialogType::State>)                                                                                                                          -> ConnectionEventsCallbackFuture + Send + Sync         + 'static,
                        ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<CONFIG, SocketDialogType::LocalMessages, SocketDialogType::SenderChannel, SocketDialogType::State>>, /*server_messages_stream: */MessagingMutinyStream<SocketDialogType::ProcessorUni>) -> OutputStreamType>

                       (self,
                        socket_connection:           SocketConnection<SocketDialogType::State>,
                        mut shutdown_signaler:       tokio::sync::broadcast::Receiver<()>,
                        connection_events_callback:  ConnectionEventsCallback,
                        dialog_processor_builder_fn: ProcessorBuilderFn)

                       -> Result<SocketConnection<SocketDialogType::State>, Box<dyn Error + Sync + Send>> {

        let addr = socket_connection.connection().peer_addr()?;
        let sender = ReactiveMessagingSender::<CONFIG, SocketDialogType::LocalMessages, SocketDialogType::SenderChannel>::new(format!("Sender for client {addr}"));
        let peer = Arc::new(Peer::new(sender, addr, &socket_connection));
        let peer_ref1 = Arc::clone(&peer);
        let peer_ref2 = Arc::clone(&peer);
        let peer_ref3 = Arc::clone(&peer);
        let peer_ref4 = Arc::clone(&peer);

        // issue the connection event
        connection_events_callback(ProtocolEvent::PeerArrived {peer: peer.clone()}).await;

        let connection_events_callback_ref1 = Arc::new(connection_events_callback);
        let connection_events_callback_ref2 = Arc::clone(&connection_events_callback_ref1);
        let processor_sender = SocketDialogType::ProcessorUni::new(format!("Client Processor for remote server @ {addr}"))
            .spawn_non_futures_non_fallibles_executors(1,
                                                    |in_stream| dialog_processor_builder_fn(addr.ip().to_string(), addr.port(), peer_ref1.clone(), in_stream),
                                                    move |executor| async move {
                                                        let execution_duration = Duration::from_nanos(executor.execution_finish_delta_nanos() - executor.execution_start_delta_nanos());
                                                        let executor_name = executor.executor_name().clone();
                                                        // issue the async disconnect event when the `Stream` ends
                                                        connection_events_callback_ref1(ProtocolEvent::PeerLeft { peer: peer_ref2, stream_stats: executor }).await;
                                                        // ensure the related peer is also closed
                                                        let flush_timeout_millis = Self::CONST_CONFIG.flush_timeout_millis as u64;
                                                        let unsent_messages = peer_ref3.flush_and_close(Duration::from_millis(flush_timeout_millis)).await;
                                                        if unsent_messages > 0 {
                                                            warn!("{executor_name} (in execution for {execution_duration:?}) left with {unsent_messages} unsent messages to {peer_ref3:?}, even after flushing with a timeout of {flush_timeout_millis}ms after, probably, its `Stream` being dropped. Consider increasing `CONST_CONFIG.flush_timeout_millis` or revisiting the processor logic.");
                                                        }
                                                    });
        let processor_sender = upgrade_processor_uni_retrying_logic::<CONFIG, SocketDialogType::DeserializedRemoteMessages, <<SocketDialogType as SocketDialog<CONFIG>>::ProcessorUni as GenericUni>::DerivedItemType, SocketDialogType::ProcessorUni>
                                                                                                (processor_sender);

        // spawn the shutdown listener
        let addr = addr.to_string();
        tokio::spawn(async move {
            match shutdown_signaler.recv().await {
                Ok(())             => trace!("reactive-messaging: SHUTDOWN requested for client connected to server @ {addr} -- notifying & dropping the connection"),
                Err(err) => error!("reactive-messaging: PROBLEM in the `shutdown signaler` client connected to server @ {addr} (a client shutdown will be commanded now due to this occurrence): {err:?}"),
            };
            // issue the shutdown event
            connection_events_callback_ref2(ProtocolEvent::LocalServiceTermination).await;
            // close the connection
            peer_ref4.flush_and_close(Duration::from_millis(Self::CONST_CONFIG.flush_timeout_millis as u64)).await;
        });

        // the processor
        let arc_self = Arc::new(self);
        arc_self.dialog_loop(socket_connection, peer.clone(), processor_sender).await
    }

    /// Handles the "local" side of the peers dialog that is to take place once the connection is ready.\
    /// The protocol is determined by `LocalMessages` & `RemoteMessages`, where and each one may either be the client messages or server messages, depending on who is the "local peer":
    ///   - `peer` represents the remote end of the connection;
    ///   - `processor_sender` is a [reactive_mutiny::Uni], to which incoming messages will be sent;
    ///   - conversely, `peer.sender` is the [reactive_mutiny::Uni] to receive outgoing messages.
    ///
    /// After the processor is done with the `socket_connection`, the method returns it back to the caller if no errors had happen.
    #[inline(always)]
    async fn dialog_loop(self: Arc<Self>,
                         mut socket_connection: SocketConnection<SocketDialogType::State>,
                         peer:                  Arc<Peer<CONFIG, SocketDialogType::LocalMessages, SocketDialogType::SenderChannel, SocketDialogType::State>>,
                         processor_sender:      ReactiveMessagingUniSender<CONFIG, SocketDialogType::DeserializedRemoteMessages, <<SocketDialogType as SocketDialog<CONFIG>>::ProcessorUni as GenericUni>::DerivedItemType, SocketDialogType::ProcessorUni>)

                        -> Result<SocketConnection<SocketDialogType::State>, Box<dyn Error + Sync + Send>> {

        // socket configs
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

        // delegate the dialog loop to the specialized message form specialist
        let result = SocketDialogType::default().dialog_loop(&mut socket_connection, &peer, &processor_sender).await;

        // processor closing procedures
        if let Ok(()) = result {
            _ = processor_sender.close(Duration::from_millis(Self::CONST_CONFIG.flush_timeout_millis as u64)).await;
            peer.cancel_and_close();
            socket_connection.connection_mut()
                .flush()
                .await
                .map_err(|err| format!("error flushing the socket connected to {}:{}: {}", peer.peer_address, peer.peer_id, err))?;
            if let Some(state) = peer.take_state().await {
                socket_connection.set_state(state);                
            }
        }

        // return the connection
        result.map(|_| socket_connection)
    }

//     #[inline(always)]
//     async fn dialog_loop_for_variable_binary_form(self: Arc<Self>,
//                                                   socket_connection:     &mut SocketConnection<StateType>,
//                                                   peer:                  &Arc<Peer<CONFIG, LocalMessagesType, SenderChannel, StateType>>,
//                                                   _processor_sender:     &ReactiveMessagingUniSender<CONFIG, RemoteMessagesType, ProcessorUniType::DerivedItemType, ProcessorUniType>,
//                                                   max_payload_size:      u32)
//
//                                                  -> Result<(), Box<dyn Error + Sync + Send>>
//
//                                                  /* TODO: 2024-02-26: the blocker on this method is that we need to use a particular channel type that allows us to allocate the elements in this function -- via Box or Arc,  */
//                                                  /*       in opposition to publishing the raw `RemoteMessagesType` as done in the textual or fixed binary versions. To circumvent this, our macros might not only receive the Channel */
//                                                  /*       type, but the message form as well (which would be moved out of ConstConfig) and, here, we may publish the element as `processor_sender.send_derived(arc_version)` */
//                                                  /*       provided that the caller of this method, if using RKYV, will use `ArchivedMyRemoteMessages` for the `RemoteMessagesType`. */
//                                                  /*       Anyway, we may have to rework `reactive-mutiny` for that, as .send_derived() seems not to be, currently, available for `Uni`s */
//                                                  /*where Box<RemoteMessagesType>: Deref -- provided the caller needs to use the RKYV's `ArchiveRemoteMessages` on all invocations */  {
//         const SIZE_OF_MAX_LEN: usize = 4; // use Self::CONST_CONFIG.message_form.size_of_max_len() everywhere
//
//         const SERIALIZATION_BUFFER_SIZE_HINT: u32 = 1024;
//         let mut serialization_buffer = Vec::<u8>::with_capacity(max_payload_size.min(SERIALIZATION_BUFFER_SIZE_HINT) as usize);
//
//         /// Controls the read state machine -- a full read message consists of two reads: the payload size (u32) followed by the payload itself.\
//         /// Using a state machine allows we to prioritise writes without having to duplicate the `select!()` expression
//         enum ReadState {
//             ExpectingSize,
//             ExpectingPayload,
//         }
//         let _read_state = ReadState::ExpectingSize;
//         let mut payload_size_buff: [u8; SIZE_OF_MAX_LEN] = [0; SIZE_OF_MAX_LEN];
//         let mut payload_size_buff_len = 0usize;
//
//         let (mut sender_stream, _) = peer.create_stream();
//
//         // for sending -- returns `false` if it was not possible to send (an unrecoverable error which should cause the upper method to return)
//         async fn send_message<LocalMessagesType: ReactiveMessagingSerializer<LocalMessagesType> + Send + Sync + PartialEq + Debug + 'static,
//                               StateType:                                                          Send + Sync + Clone     + Debug + 'static>
//                              (to_send: Option<LocalMessagesType>,
//                               serialization_buffer: &mut Vec<u8>,
//                               socket_connection: &mut SocketConnection<StateType>,
//                               peer_descriptor: impl FnOnce() -> String)
//                              -> bool {
//             match to_send {
//                 Some(to_send) => {
//                     // serialize
//                     LocalMessagesType::serialize_binary(&to_send, serialization_buffer);
//                     let payload_len = serialization_buffer.len() - SIZE_OF_MAX_LEN;
//                     let payload_len_bytes = payload_len.to_le_bytes();
//                     for i in 0..SIZE_OF_MAX_LEN {
//                         serialization_buffer[i] = payload_len_bytes[i];
//                     }
//                     // send
//                     if let Err(err) = socket_connection.connection_mut().write_all(&serialization_buffer).await {
//                         warn!("`dialog_loop_for_variable_binary_form()`: PROBLEM in the connection with {} while WRITING '{to_send:?}': {err:?}", peer_descriptor());
//                         socket_connection.report_closed();
//                         return false
//                     }
//                 },
//                 None => {
//                     debug!("`dialog_loop_for_variable_binary_form()`: Sender for {} ended (most likely, either `peer.flush_and_close()` or `peer.cancel_and_close()` was called)", peer_descriptor());
//                     return false
//                 }
//             }
//             true
//         }
//
//         'connection: loop {
//
//             let mut read_slice: &mut [u8] = &mut payload_size_buff[payload_size_buff_len..];    // for some reason, Rust 1.76 doesn't allow inlining this expression where it is used (resulting in it not being recognized as &mut [u8]) -- a compiler bug?
//
//             // 1) Read the payload size -- prioritizing writes back to the peer, in case any answer is ready
//             let expected_payload_size = 'reading_payload_size: loop {
//                 tokio::select!(
//
//                     biased;     // sending has priority over receiving
//
//                     // send?
//                     to_send = sender_stream.next() => if !send_message(to_send, &mut serialization_buffer, socket_connection, || format!("{peer:?}")).await {
//                         break 'connection
//                     },
//
//                     // receive?
//                     read = socket_connection.connection_mut().read_buf(&mut read_slice) => {
//                         match read {
//                             Ok(n) if n > 0 => {
//                                 payload_size_buff_len += n;
//                                 // completed reading the payload size?
//                                 if payload_size_buff_len == SIZE_OF_MAX_LEN {
//                                     let payload_size = u32::from_le_bytes(payload_size_buff);
//                                     break 'reading_payload_size payload_size as usize
//                                 }
//                             },
//                             Ok(_) /* zero bytes received -- the other end probably closed the connection */ => {
//                                 trace!("`dialog_loop_for_variable_binary_form()`: EOF while reading from {:?} (peer id {}) -- it is out of bytes! Dropping the connection", peer.peer_address, peer.peer_id);
//                                 socket_connection.report_closed();
//                                 break 'connection
//                             },
//                             Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {},
//                             Err(err) => {
//                                 error!("`dialog_loop_for_variable_binary_form()`: ERROR in the connection with {:?} (peer id {}) while READING: '{:?}' -- dropping it", peer.peer_address, peer.peer_id, err);
//                                 socket_connection.report_closed();
//                                 break 'connection
//                             },
//                         }
//                     },
//                 );
//             };
//
//             let mut payload_slot: Box<[u8]> = vec![0; expected_payload_size as usize].into_boxed_slice();
//             let mut payload_slot_len = 0usize;
//
//             // 2) Read the payload & deliver it to the local processor -- prioritizing writes back to the peer, in case any answer is ready
//             let _payload_slot = 'reading_payload: loop {
//
//                 let mut read_slice: &mut [u8] = &mut payload_slot[payload_slot_len..];  // for some reason, Rust 1.76 doesn't allow inlining this expression where it is used (resulting in it not being recognized as &mut [u8]) -- a compiler bug?
//
//                 tokio::select!(
//
//                     biased;     // sending has priority over receiving
//
//                     // send?
//                     to_send = sender_stream.next() => if !send_message(to_send, &mut serialization_buffer, socket_connection, || format!("{peer:?}")).await {
//                         break 'connection
//                     },
//
//                     // receive?
//                     read = socket_connection.connection_mut().read_buf(&mut read_slice) => {
//                         match read {
//                             Ok(n) if n > 0 => {
//                                 payload_slot_len += n;
//                                 // completed reading the payload?
//                                 if payload_slot_len == expected_payload_size {
//                                     // deserialize
//                                     let rkyv_archive = ();  // make, somehow, RKYV to use `payload_slot` as our `Box<archive_model>`
//                                     // TODO: RKYV obviously needs a variable size... do we need to check that?
//                                     // let ptr = slot.as_mut_ptr();
//                                     break 'reading_payload rkyv_archive
//
//                                 }
//                             },
//                             Ok(_) /* zero bytes received -- the other end probably closed the connection */ => {
//                                 trace!("`dialog_loop_for_variable_binary_form()`: EOF with reading from {:?} (peer id {}) -- it is out of bytes! Dropping the connection", peer.peer_address, peer.peer_id);
//                                 socket_connection.report_closed();
//                                 break 'connection
//                             },
//                             Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {},
//                             Err(err) => {
//                                 error!("`dialog_loop_for_variable_binary_form()`: ERROR in the connection with {:?} (peer id {}) while READING: '{:?}' -- dropping it", peer.peer_address, peer.peer_id, err);
//                                 socket_connection.report_closed();
//                                 break 'connection
//                             },
//                         }
//                     },
//                 );
//             };
//
//             // 3) deliver the payload for processing
//             // processor_sender(payload);
//
//             // TESTS to add to our crate:
//             // 1) serialize_binary(PayloadType) -> Vec<u8> -- containing the data of a ArchivePayloadType
//             // 2) deserialize_binary? No, not needed!!! We are zero-copy, so NO DESERIALIZATION!
//             // 3) This dialog's `processor_sender` should receive items like  Arc<ArchiveRemoteMessagesType>
//
//             // TESTS to add to `reactive-mutiny`:
//             // 1) Being able to send_derived an `Arc<ArchivePayloadType>` to a `Uni<PayloadType>`.
//             //    (and the method here must require that the processor sender uses an -- yet to be introduced -- Arc allocator?)
//
//         }
//         Ok(())
//     }

}


/// Unit tests the [tokio_message_io](self) module
#[cfg(any(test,doc))]
pub mod tests {
    use super::*;
    use crate::unit_test_utils::{next_server_port, StringDeserializer, StringSerializer};
    use crate::socket_connection::{
        socket_dialog::textual_dialog::TextualDialog,
        connection_provider::ServerConnectionHandler,
    };
    use std::{
        future,
        sync::atomic::{
            AtomicBool,
            AtomicU32,
            Ordering::Relaxed,
        },
        net::ToSocketAddrs,
    };
    use std::time::Instant;
    use reactive_mutiny::{prelude::advanced::{UniZeroCopyAtomic, ChannelUniMoveAtomic, ChannelUniZeroCopyAtomic}, types::{ChannelCommon, ChannelUni, ChannelProducer}};
    use futures::stream::{self, StreamExt};
    use tokio::net::TcpStream;
    use tokio::sync::Mutex;
    use crate::serde::{ReactiveMessagingRonDeserializer, ReactiveMessagingRonSerializer};

    #[cfg(debug_assertions)]
    const DEBUG: bool = true;
    #[cfg(not(debug_assertions))]
    const DEBUG: bool = false;


    /// Checks if the test configs make sense & the libraries are still working under the same contract,
    /// popping up any errors, as soon as possible, that would be difficult to debug otherwise
    #[cfg_attr(not(doc),tokio::test)]
    async fn sanity_check() {

        const DEFAULT_TEST_CONFIG: ConstConfig         = ConstConfig {
            retrying_strategy: RetryingStrategies::RetryYieldingForUpToMillis(30),
            ..ConstConfig::default()
        };
        const DEFAULT_TEST_CONFIG_U64:  u64            = DEFAULT_TEST_CONFIG.into();
        const DEFAULT_TEST_UNI_INSTRUMENTS: usize      = DEFAULT_TEST_CONFIG.executor_instruments.into();
        type DefaultTestUni<PayloadType = String>      = UniZeroCopyAtomic<PayloadType, {DEFAULT_TEST_CONFIG.receiver_channel_size as usize}, 1, DEFAULT_TEST_UNI_INSTRUMENTS>;
        type SenderChannel<PayloadType = String>       = ChannelUniMoveAtomic<PayloadType, {DEFAULT_TEST_CONFIG.sender_channel_size as usize}, 1>;

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
        let sender = ChannelUniZeroCopyAtomic::<String, {DEFAULT_TEST_CONFIG.sender_channel_size as usize}, 1>::new("Please work also...");
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
        let mut connection_provider = ServerConnectionHandler::new("127.0.0.1", next_server_port(), ()).await
            .expect("couldn't start the Connection Provider server event loop");
        let new_connections_source = connection_provider.connection_receiver()
            .expect("couldn't move the Connection Receiver out of the Connection Provider");
        let (returned_connections_sink, _returned_connections_source) = tokio::sync::mpsc::channel::<SocketConnection<()>>(2);
        let socket_communications_handler = SocketConnectionHandler::<DEFAULT_TEST_CONFIG_U64, TextualDialog<DEFAULT_TEST_CONFIG_U64, String, String, StringSerializer, StringDeserializer, DefaultTestUni, SenderChannel, ()>>::new(TextualDialog::default());
        socket_communications_handler.server_loop(
            "127.0.0.1", 8579, new_connections_source, returned_connections_sink,
            |connection_event| async move {
                match connection_event {
                    ProtocolEvent::PeerArrived { .. }       => tokio::time::sleep(Duration::from_millis(100)).await,
                    ProtocolEvent::PeerLeft { .. }    => tokio::time::sleep(Duration::from_millis(100)).await,
                    ProtocolEvent::LocalServiceTermination { .. } => tokio::time::sleep(Duration::from_millis(100)).await,
                }
            },
            move |_client_addr, _client_port, _peer, client_messages_stream|
                client_messages_stream
        ).await.expect("Starting the server");

    }
    
    // REUSABLE TEST METHODS
    ////////////////////////
    // Used in the submodules


    /// Assures connection & dialogs work for either the server and client, using the "unresponsive" flavours of the `Stream` processors
    pub async fn unresponsive_dialogs<const CONFIG:     u64,
                                      SocketDialogType: SocketDialog<CONFIG, RemoteMessages = String, LocalMessages = String, State = ()> + 'static>
                                     ()
                                     where <<SocketDialogType as SocketDialog<CONFIG>>::ProcessorUni as GenericUni>::DerivedItemType: PartialEq<String> {
        const LISTENING_INTERFACE: &str = "127.0.0.1";
        let port = next_server_port();
        let client_secret = String::from("open, sesame");
        let server_secret = String::from("now the 40 of you may enter");
        let observed_secret = Arc::new(Mutex::new(None));

        let (_client_shutdown_sender, client_shutdown_receiver) = tokio::sync::broadcast::channel(1);

        // server
        let client_secret_clone = client_secret.clone();
        let server_secret_clone = server_secret.clone();
        let mut connection_provider = ServerConnectionHandler::new(LISTENING_INTERFACE, port, ()).await
            .expect("Sanity Check: couldn't start the Connection Provider server event loop");
        let new_connections_source = connection_provider.connection_receiver()
            .expect("Sanity Check: couldn't move the Connection Receiver out of the Connection Provider");
        let socket_communications_handler = SocketConnectionHandler::<CONFIG, SocketDialogType>::new(SocketDialogType::default());
        let (returned_connections_sink, mut server_connections_source) = tokio::sync::mpsc::channel::<SocketConnection<()>>(2);
        socket_communications_handler.server_loop(
            LISTENING_INTERFACE, port, new_connections_source, returned_connections_sink,
            |connection_event| {
                 match connection_event {
                     ProtocolEvent::PeerArrived { peer } => {
                         assert!(peer.send(String::from("Welcome! State your business!")).is_ok(), "couldn't send");
                     },
                     ProtocolEvent::PeerLeft { peer: _, stream_stats: _ } => {},
                     ProtocolEvent::LocalServiceTermination => {
                         println!("Test Server: shutdown was requested... No connection will receive the drop message (nor will be even closed) because I, the lib caller, intentionally didn't keep track of the connected peers for this test!");
                     }
                 }
                 future::ready(())
            },
            move |_client_addr, _client_port, peer, client_messages_stream| {
                let client_secret_clone = client_secret_clone.clone();
                let server_secret_clone = server_secret_clone.clone();
                client_messages_stream.inspect(move |client_message| {
                    assert!(peer.send(format!("Client just sent {:?}", client_message)).is_ok(), "couldn't send");
                    if *client_message == client_secret_clone {
                        assert!(peer.send(server_secret_clone.clone()).is_ok(), "couldn't send");
                    } else {
                        panic!("Client sent the wrong secret: {:?} -- I was expecting '{}'", client_message, client_secret_clone);
                    }
                })
            }
        ).await.expect("Starting the server");

        println!("### Waiting a little for the server to start...");
        tokio::time::sleep(Duration::from_millis(10)).await;

        // client
        let client_secret = client_secret.clone();
        let observed_secret_clone = Arc::clone(&observed_secret);
        let tokio_connection = TcpStream::connect(format!("{}:{}", LISTENING_INTERFACE, port).to_socket_addrs().expect("Error resolving address").into_iter().next().unwrap()).await.expect("Error connecting");
        let socket_connection = SocketConnection::new(tokio_connection, ());
        let client_communications_handler = SocketConnectionHandler::<CONFIG, SocketDialogType>::new(SocketDialogType::default());
        tokio::spawn(
            client_communications_handler.client(
                socket_connection, client_shutdown_receiver,
                move |connection_event| {
                    match connection_event {
                        ProtocolEvent::PeerArrived { peer } => {
                            assert!(peer.send(client_secret.clone()).is_ok(), "couldn't send");
                        },
                        ProtocolEvent::PeerLeft { peer, stream_stats: _ } => {
                            println!("Test Client: connection with {} (peer_id #{}) was dropped -- should not happen in this test", peer.peer_address, peer.peer_id);
                        },
                        ProtocolEvent::LocalServiceTermination => {}
                    }
                    future::ready(())
                },
                move |_client_addr, _client_port, _peer, server_messages_stream| {
                    let observed_secret_clone = Arc::clone(&observed_secret_clone);
                    server_messages_stream.then(move |server_message| {
                        let observed_secret_clone = Arc::clone(&observed_secret_clone);
                        async move {
                            println!("Server said: {:?}", server_message);
                            let _ = observed_secret_clone.lock().await.insert(server_message);
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
    
    /// Assures connection & dialogs work for either the server and client, using the "responsive" flavours of the `Stream` processors
    pub async fn responsive_dialogs<const CONFIG:     u64,
                                    SocketDialogType: SocketDialog<CONFIG, RemoteMessages = String, LocalMessages = String, State = ()> + 'static>
                                   ()
                                   where <<SocketDialogType as SocketDialog<CONFIG>>::ProcessorUni as GenericUni>::DerivedItemType: PartialEq<String> {
        
        const LISTENING_INTERFACE: &str = "127.0.0.1";
        let port = next_server_port();
        let client_secret = String::from("open, sesame");
        let server_secret = String::from("now the 40 of you may enter");
        let observed_secret = Arc::new(Mutex::new(None));
    
        let (_client_shutdown_sender, client_shutdown_receiver) = tokio::sync::broadcast::channel(1);
    
        // server
        let client_secret_clone = client_secret.clone();
        let server_secret_clone = server_secret.clone();
        let mut connection_provider = ServerConnectionHandler::new(LISTENING_INTERFACE, port, ()).await
            .expect("couldn't start the Connection Provider server event loop");
        let new_connections_source = connection_provider.connection_receiver()
            .expect("couldn't move the Connection Receiver out of the Connection Provider");
        let (returned_connections_sink, mut server_connections_source) = tokio::sync::mpsc::channel::<SocketConnection<()>>(2);
        let socket_communications_handler = SocketConnectionHandler::<CONFIG, SocketDialogType>::new(SocketDialogType::default());
        socket_communications_handler.server_loop(LISTENING_INTERFACE, port, new_connections_source, returned_connections_sink,
                                                  |_connection_event| future::ready(()),
                                                  move |_client_addr, _client_port, peer, client_messages_stream| {
               let client_secret_clone = client_secret_clone.clone();
               let server_secret_clone = server_secret_clone.clone();
                assert!(peer.send(String::from("Welcome! State your business!")).is_ok(), "couldn't send");
                client_messages_stream
                    .flat_map(move |client_message| {
                       stream::iter([
                           format!("Client just sent {:?}", client_message),
                           if client_message == client_secret_clone {
                               server_secret_clone.clone()
                           } else {
                               panic!("Client sent the wrong secret: {:?} -- I was expecting '{}'", client_message, client_secret_clone);
                           }])
                    })
                    .to_responsive_stream(peer, |_, _| ())
            }
        ).await.expect("ERROR starting the server");
    
        println!("### Waiting a little for the server to start...");
        tokio::time::sleep(Duration::from_millis(10)).await;
    
        // client
        let client_secret = client_secret.clone();
        let observed_secret_clone = Arc::clone(&observed_secret);
        let tokio_connection = TcpStream::connect(format!("{}:{}", LISTENING_INTERFACE, port).to_socket_addrs().expect("Error resolving address").into_iter().next().unwrap()).await.expect("Error connecting");
        let socket_connection = SocketConnection::new(tokio_connection, ());
        let socket_communications_handler = SocketConnectionHandler::<CONFIG, SocketDialogType>::new(SocketDialogType::default());
        tokio::spawn(
            socket_communications_handler.client(socket_connection, client_shutdown_receiver,
                                                 move |_connection_event| future::ready(()),
                                                 move |_client_addr, _client_port, peer, server_messages_stream| {
                    let observed_secret_clone = Arc::clone(&observed_secret_clone);
                    assert!(peer.send(client_secret.clone()).is_ok(), "couldn't send");
                    server_messages_stream
                        .then(move |server_message| {
                            let observed_secret_clone = Arc::clone(&observed_secret_clone);
                            async move {
                                println!("Server said: {:?}", server_message);
                                let _ = observed_secret_clone.lock().await.insert(server_message);
                            }
                        })
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
    pub async fn client_termination<const CONFIG:     u64,
                                    SocketDialogType: SocketDialog<CONFIG, RemoteMessages = String, LocalMessages = String, State = ()> + 'static>
                                   ()
                                   where <<SocketDialogType as SocketDialog<CONFIG>>::ProcessorUni as GenericUni>::DerivedItemType: std::ops::Deref<Target=String> {
        const LISTENING_INTERFACE: &str = "127.0.0.1";
        let port = next_server_port();
        let server_disconnected = Arc::new(AtomicBool::new(false));
        let server_disconnected_ref = Arc::clone(&server_disconnected);
        let client_disconnected = Arc::new(AtomicBool::new(false));
        let client_disconnected_ref = Arc::clone(&client_disconnected);
    
        let (client_shutdown_sender, client_shutdown_receiver) = tokio::sync::broadcast::channel(2);
    
        let mut connection_provider = ServerConnectionHandler::new(LISTENING_INTERFACE, port, ()).await
            .expect("couldn't start the Connection Provider server event loop");
        let new_connections_source = connection_provider.connection_receiver()
            .expect("couldn't move the Connection Receiver out of the Connection Provider");
        let (returned_connections_sink, mut server_connections_source) = tokio::sync::mpsc::channel::<SocketConnection<()>>(2);
        let socket_communications_handler = SocketConnectionHandler::<CONFIG, SocketDialogType>::new(SocketDialogType::default());
        socket_communications_handler.server_loop(
            LISTENING_INTERFACE, port, new_connections_source, returned_connections_sink,
            move |connection_event| {
                let server_disconnected = Arc::clone(&server_disconnected_ref);
                async move {
                    if let ProtocolEvent::PeerLeft { .. } = connection_event {
                        server_disconnected.store(true, Relaxed);
                    }
                }
            },
            move |_client_addr, _client_port, _peer, client_messages_stream| client_messages_stream
        ).await.expect("ERROR starting the server");
    
        // wait a bit for the server to start
        tokio::time::sleep(Duration::from_millis(10)).await;
    
        let tokio_connection = TcpStream::connect(format!("{}:{}", LISTENING_INTERFACE, port).to_socket_addrs().expect("Error resolving address").into_iter().next().unwrap()).await.expect("Error connecting");
        let socket_connection = SocketConnection::new(tokio_connection, ());
        let socket_communications_handler = SocketConnectionHandler::<CONFIG, SocketDialogType>::new(SocketDialogType::default());
    
        tokio::spawn(
            socket_communications_handler.client(
                socket_connection, client_shutdown_receiver,
                move |connection_event| {
                    let client_disconnected = Arc::clone(&client_disconnected_ref);
                    async move {
                        if let ProtocolEvent::PeerLeft { .. } = connection_event {
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

    /// Assures the minimum acceptable latency values -- for either Debug & Release modes.\
    /// One sends Ping(n); the other receives it and send Pong(n); the first receives it and sends Ping(n+1) and so on...\
    /// Latency is computed dividing the number of seconds per n*2 (we care about the server leg of the latency, while here we measure the round trip client<-->server).
    /// 
    /// NOTE for tests using these assertions: annotate your test with `#[ignore]`, following the convention that ignored tests are to be run by a single thread
    /// 
    /// Run it (on an idle machine) with:
    /// ```nocompile
    /// # Testing the release latencies
    /// sudo sync; sleep 1; RUSTFLAGS="-C target-cpu=native" cargo test --release -- --ignored --test-threads=1 --nocapture latency_measurements
    /// # Testing the debug latencies
    /// sudo sync; sleep 1; cargo test -- --ignored --test-threads=1 --nocapture latency_measurements
    pub async fn latency_measurements<const CONFIG:     u64,
                                      SocketDialogType: SocketDialog<CONFIG, RemoteMessages = String, LocalMessages = String, State = ()> + 'static>
    
                                     (tolerance: f64, debug_expected_count: u32, release_expected_count: u32)
    
                                     where <<SocketDialogType as SocketDialog<CONFIG>>::ProcessorUni as GenericUni>::DerivedItemType: std::ops::Deref<Target=String> {
        const TEST_DURATION_MS:    u64  = 2000;
        const TEST_DURATION_NS:    u64  = TEST_DURATION_MS * 1e6 as u64;
        const LISTENING_INTERFACE: &str = "127.0.0.1";
        let port = next_server_port();
    
        let (client_shutdown_sender, client_shutdown_receiver) = tokio::sync::broadcast::channel(1);
    
        // server
        let mut connection_provider = ServerConnectionHandler::new(LISTENING_INTERFACE, port, ()).await
            .expect("couldn't start the Connection Provider server event loop");
        let new_connections_source = connection_provider.connection_receiver()
            .expect("couldn't move the Connection Receiver out of the Connection Provider");
        let (returned_connections_sink, mut server_connections_source) = tokio::sync::mpsc::channel::<SocketConnection<()>>(2);
        let socket_communications_handler = SocketConnectionHandler::<CONFIG, SocketDialogType>::new(SocketDialogType::default());
        socket_communications_handler.server_loop(
            LISTENING_INTERFACE, port, new_connections_source, returned_connections_sink,
            |_connection_event| async {},
            |_listening_interface, _listening_port, peer, client_messages| {
                 client_messages.inspect(move |client_message| {
                     // Receives: "Ping(n)"
                     let n_str = &client_message[5..(client_message.len() - 1)];
                     let n = str::parse::<u32>(n_str).unwrap_or_else(|err| panic!("could not convert '{}' to number. Original client message: {:?}. Parsing error: {:?}", n_str, client_message, err));
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
        let tokio_connection = TcpStream::connect(format!("{}:{}", LISTENING_INTERFACE, port).to_socket_addrs().expect("Error resolving address").into_iter().next().unwrap()).await.expect("Error connecting");
        let socket_connection = SocketConnection::new(tokio_connection, ());
        let socket_communications_handler = SocketConnectionHandler::<CONFIG, SocketDialogType>::new(SocketDialogType::default());
        tokio::spawn(
            socket_communications_handler.client(
                socket_connection, client_shutdown_receiver,
                |connection_event| {
                    match connection_event {
                        ProtocolEvent::PeerArrived { peer } => {
                            // conversation starter
                            assert!(peer.send(String::from("Ping(0)")).is_ok(), "couldn't send");
                        },
                        ProtocolEvent::PeerLeft { .. } => {},
                        ProtocolEvent::LocalServiceTermination { .. } => {},
                    }
                    future::ready(())
                },
                move |_listening_interface, _listening_port, peer, server_messages| {
                    let counter_ref = Arc::clone(&counter_ref);
                    server_messages.inspect(move |server_message| {
                        // Receives: "Pong(n)"
                        let n_str = &server_message[5..(server_message.len()-1)];
                        let n = str::parse::<u32>(n_str).unwrap_or_else(|err| panic!("could not convert '{}' to number. Original server message: {:?}. Parsing error: {:?}", n_str, server_message, err));
                        let current_count = counter_ref.fetch_add(1, Relaxed);
                        if n != current_count {
                            panic!("Received {:?}, where Client was expecting 'Pong({})'", server_message, current_count);
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
            assert!(counter > ((1.0 - tolerance) * debug_expected_count as f64) as u32, "Latency regression detected: we used to make {debug_expected_count} round trips in 2 seconds (Debug mode) -- now only {counter} were made");
        } else {
            assert!(counter > ((1.0 - tolerance) * release_expected_count as f64) as u32, "Latency regression detected: we used to make {release_expected_count} round trips in 2 seconds (Release mode) -- now only {counter} were made");
    
        }
    }
    
    /// When a client floods the server with messages, it should, at most, screw just that client up... or, maybe, not even that!\
    /// This test works like the latency test, but we don't wait for the answer to come to send another one -- we just keep sending like crazy\
    ///
    /// NOTE for tests using these assertions: annotate your test with `#[ignore]`, following the convention that ignored tests are to be run by a single thread
    ///
    /// Run it (on an idle machine) with:
    /// ```nocompile
    /// # Testing the release latencies
    /// sudo sync; sleep 1; RUSTFLAGS="-C target-cpu=native" cargo test --release -- --ignored --test-threads=1 --nocapture message_flooding_throughput
    /// # Testing the debug latencies
    /// sudo sync; sleep 1; cargo test -- --ignored --test-threads=1 --nocapture message_flooding_throughput
    pub async fn message_flooding_throughput<const CONFIG:     u64,
                                             SocketDialogType: SocketDialog<CONFIG, RemoteMessages = String, LocalMessages = String, State = ()> + 'static>

                                            (tolerance: f64, debug_expected_count: u32, release_expected_count: u32)

                                            where <<SocketDialogType as SocketDialog<CONFIG>>::ProcessorUni as GenericUni>::DerivedItemType: std::ops::Deref<Target=String> {
        const TEST_DURATION_MS:    u64  = 2000;
        const LISTENING_INTERFACE: &str = "127.0.0.1";
        let port = next_server_port();
        let (client_shutdown_sender, client_shutdown_receiver) = tokio::sync::broadcast::channel(1);
    
        // server -- do not answer to (flood) messages (just parses & counts them, making sure they are received in the right order)
        // message format is "DoNotAnswer(n)", where n should be sent by the client in natural order, starting from 0
        let received_messages_count = Arc::new(AtomicU32::new(0));
        let unordered = Arc::new(AtomicU32::new(0));    // if non-zero, contains the last message received before the ordering went kaput
        let received_messages_count_ref = Arc::clone(&received_messages_count);
        let unordered_ref = Arc::clone(&unordered);
        let mut connection_provider = ServerConnectionHandler::new(LISTENING_INTERFACE, port, ()).await
            .expect("couldn't start the Connection Provider server event loop");
        let new_connections_source = connection_provider.connection_receiver()
            .expect("couldn't move the Connection Receiver out of the Connection Provider");
        let (returned_connections_sink, mut server_connections_source) = tokio::sync::mpsc::channel::<SocketConnection<()>>(2);
        let socket_communications_handler = SocketConnectionHandler::<CONFIG, SocketDialogType>::new(SocketDialogType::default());
        socket_communications_handler.server_loop(
            LISTENING_INTERFACE, port, new_connections_source, returned_connections_sink,
            |_connection_event| async {},
            move |_listening_interface, _listening_port, _peer, client_messages| {
               let received_messages_count = Arc::clone(&received_messages_count_ref);
               let unordered = Arc::clone(&unordered_ref);
               client_messages.inspect(move |client_message| {
                   // Message format: DoNotAnswer(n)
                   let n_str = &client_message[12..(client_message.len()-1)];
                   let n = str::parse::<u32>(n_str).unwrap_or_else(|_| panic!("could not convert '{}' to number. Original message: {:?}", n_str, client_message));
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
        let tokio_connection = TcpStream::connect(format!("{}:{}", LISTENING_INTERFACE, port).to_socket_addrs().expect("Error resolving address").into_iter().next().unwrap()).await.expect("Error connecting");
        let socket_connection = SocketConnection::new(tokio_connection, ());
        let socket_connection_handler = SocketConnectionHandler::<CONFIG, SocketDialogType>::new(SocketDialogType::default());
        tokio::spawn(
            socket_connection_handler.client(
                socket_connection,
                client_shutdown_receiver,
                move |connection_event| {
                    let sent_messages_count = Arc::clone(&sent_messages_count_ref);
                    match connection_event {
                        ProtocolEvent::PeerArrived { peer } => {
                            tokio::spawn(async move {
                                let start = Instant::now();
                                let mut n = 0;
                                loop {
                                    let send_result = peer.send_async(format!("DoNotAnswer({})", n)).await;
                                    assert!(send_result.is_ok(), "couldn't send: {:?}", send_result.unwrap_err());
                                    n += 1;
                                    // flush & bailout check for timeout every 128k messages
                                    if n % (1<<15) == 0 && start.elapsed().as_millis() as u64 >= TEST_DURATION_MS {
                                        println!("Client sent {} messages before bailing out", n);
                                        sent_messages_count.store(n, Relaxed);
                                        assert_eq!(peer.flush_and_close(Duration::from_secs(1)).await, 0, "couldn't flush!");
                                        break;
                                    }
                                }
                            });
                        },
                        ProtocolEvent::PeerLeft { .. } => {},
                        ProtocolEvent::LocalServiceTermination { .. } => {},
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
        tokio::time::sleep(Duration::from_millis(100)).await;
    
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
            assert!(received_messages_count > ((1.0 - tolerance) * debug_expected_count as f64) as u32, "Client flooding throughput regression detected: we used to send/receive {debug_expected_count} flood messages in this test (Debug mode) -- now only {received_messages_count} were made");
        } else {
            assert!(received_messages_count > ((1.0 - tolerance) * release_expected_count as f64) as u32, "Client flooding throughput regression detected: we used to send/receive {release_expected_count} flood messages in this test (Release mode) -- now only {received_messages_count} were made");
    
        }
    
    }

}
