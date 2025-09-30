use std::fmt::Debug;
use std::marker::PhantomData;
use std::sync::Arc;
use futures::StreamExt;
use reactive_mutiny::prelude::{FullDuplexUniChannel, GenericUni};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use crate::prelude::{Peer, SocketConnection};
use crate::serde::{ReactiveMessagingConfig, ReactiveMessagingDeserializer, ReactiveMessagingSerializer};
use crate::socket_connection::common::ReactiveMessagingUniSender;
use crate::socket_connection::socket_dialog::dialog_types::SocketDialog;
use log::{debug, error, trace, warn};
use tokio::io;
use crate::config::ConstConfig;

pub struct TextualDialog<const CONFIG:       u64,
                         RemoteMessagesType:                                                                                       Send + Sync + PartialEq + Debug + 'static,
                         LocalMessagesType:  ReactiveMessagingConfig<LocalMessagesType>                                          + Send + Sync + PartialEq + Debug + 'static,
                         Serializer:         ReactiveMessagingSerializer<LocalMessagesType>,
                         Deserializer:       ReactiveMessagingDeserializer<RemoteMessagesType>,
                         ProcessorUniType:   GenericUni<ItemType=Deserializer::DeserializedRemoteMessages>                       + Send + Sync                     + 'static,
                         SenderChannelType:  FullDuplexUniChannel<ItemType=LocalMessagesType, DerivedItemType=LocalMessagesType> + Send + Sync                     + 'static,
                         StateType:                                                                                                Send + Sync + Clone     + Debug + 'static = ()
                        > {
    _phantom_data: PhantomData<(RemoteMessagesType, LocalMessagesType, Serializer, Deserializer, ProcessorUniType, SenderChannelType, StateType)>,
}

impl<const CONFIG: u64,
     RemoteMessagesType:                                                                                       Send + Sync + PartialEq + Debug + 'static,
     LocalMessagesType:  ReactiveMessagingConfig<LocalMessagesType>                                          + Send + Sync + PartialEq + Debug + 'static,
     Serializer:         ReactiveMessagingSerializer<LocalMessagesType>,
     Deserializer:       ReactiveMessagingDeserializer<RemoteMessagesType>,
     ProcessorUniType:   GenericUni<ItemType=Deserializer::DeserializedRemoteMessages>                       + Send + Sync                     + 'static,
     SenderChannelType:  FullDuplexUniChannel<ItemType=LocalMessagesType, DerivedItemType=LocalMessagesType> + Send + Sync                     + 'static,
     StateType:                                                                                                Send + Sync + Clone     + Debug + 'static,
>
TextualDialog<CONFIG, RemoteMessagesType, LocalMessagesType, Serializer, Deserializer, ProcessorUniType, SenderChannelType, StateType> {
    const CONST_CONFIG: ConstConfig = ConstConfig::from(CONFIG);
}

impl<const CONFIG: u64,
     RemoteMessagesType:                                                                                       Send + Sync + PartialEq + Debug + 'static,
     LocalMessagesType:  ReactiveMessagingConfig<LocalMessagesType>                                          + Send + Sync + PartialEq + Debug + 'static,
     Serializer:         ReactiveMessagingSerializer<LocalMessagesType>,
     Deserializer:       ReactiveMessagingDeserializer<RemoteMessagesType>,
     ProcessorUniType:   GenericUni<ItemType=Deserializer::DeserializedRemoteMessages>                       + Send + Sync                     + 'static,
     SenderChannelType:  FullDuplexUniChannel<ItemType=LocalMessagesType, DerivedItemType=LocalMessagesType> + Send + Sync                     + 'static,
     StateType:                                                                                                Send + Sync + Clone     + Debug + 'static,
    >
Default
for TextualDialog<CONFIG, RemoteMessagesType, LocalMessagesType, Serializer, Deserializer, ProcessorUniType, SenderChannelType, StateType> {
    fn default() -> Self {
        Self {
            _phantom_data: PhantomData,
        }
    }
}

impl<const CONFIG: u64,
     RemoteMessagesType:                                                                                       Send + Sync + PartialEq + Debug + 'static,
     LocalMessagesType:  ReactiveMessagingConfig<LocalMessagesType>                                          + Send + Sync + PartialEq + Debug + 'static,
     Serializer:         ReactiveMessagingSerializer<LocalMessagesType>,
     Deserializer:       ReactiveMessagingDeserializer<RemoteMessagesType>,
     ProcessorUniType:   GenericUni<ItemType=Deserializer::DeserializedRemoteMessages>                       + Send + Sync                     + 'static,
     SenderChannelType:  FullDuplexUniChannel<ItemType=LocalMessagesType, DerivedItemType=LocalMessagesType> + Send + Sync                     + 'static,
     StateType:                                                                                                Send + Sync + Clone     + Debug + 'static,
    >
SocketDialog<CONFIG>
for TextualDialog<CONFIG, RemoteMessagesType, LocalMessagesType, Serializer, Deserializer, ProcessorUniType, SenderChannelType, StateType> {
    type RemoteMessages = RemoteMessagesType;
    type DeserializedRemoteMessages = Deserializer::DeserializedRemoteMessages;
    type LocalMessages  = LocalMessagesType;
    type ProcessorUni   = ProcessorUniType;
    type SenderChannel  = SenderChannelType;
    type State          = StateType;

    /// Dialog loop specialized in text-based message forms, where each in & out event/command/sentence ends in '\n'.
    #[inline(always)]
    async fn dialog_loop(self,
                         socket_connection:     &mut SocketConnection<StateType>,
                         peer:                  &Arc<Peer<CONFIG, Self::LocalMessages, Self::SenderChannel, StateType>>,
                         processor_sender:      &ReactiveMessagingUniSender<CONFIG, Self::DeserializedRemoteMessages, <<Self as SocketDialog<CONFIG>>::ProcessorUni as GenericUni>::DerivedItemType, Self::ProcessorUni>)

                        -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

        let mut read_buffer = Vec::with_capacity(Self::CONST_CONFIG.receiver_max_msg_size as usize);
        let mut serialization_buffer = Vec::with_capacity(Self::CONST_CONFIG.sender_max_msg_size as usize);

        // sanity checks on payload sizes
        debug_assert!(read_buffer.capacity() >= 4,          "Textual Dialog Loop: the given `CONST_CONFIG.receiver_max_msg_size` for the payload is too small (only {} bytes) and this is probably a BUG in your program", read_buffer.len());
        debug_assert!(serialization_buffer.capacity() >= 4, "Textual Dialog Loop: the given `CONST_CONFIG.sender_max_msg_size` for the payload is too small (only {} bytes) and this is probably a BUG in your program", serialization_buffer.len());

        let (mut sender_stream, _) = peer.create_stream();

        'connection: loop {
            // wait for the socket to be readable or until we have something to write
            tokio::select!(

                biased;     // sending has priority over receiving

                // send?
                to_send = sender_stream.next() => {
                    match to_send {
                        Some(to_send) => {
                            // serialize
                            Serializer::serialize(&to_send, &mut serialization_buffer);
                            serialization_buffer.push(b'\n');
                            // send
                            if let Err(err) = socket_connection.connection_mut().write_all(&serialization_buffer).await {
                                warn!("`dialog_loop() for textual protocol: PROBLEM in the connection with {peer:#?} while WRITING '{to_send:?}': {err:?}");
                                socket_connection.report_closed();
                                break 'connection
                            }
                        },
                        None => {
                            debug!("`dialog_loop() for textual protocol: Sender for {peer:#?} ended (most likely, either `peer.flush_and_close()` or `peer.cancel_and_close()` was called on the `peer`)");
                            break 'connection
                        }
                    }
                },

                // receive?
                read = socket_connection.connection_mut().read_buf(&mut read_buffer) => {
                    match read {
                        Ok(n) if n > 0 => {
                            // deserialize
                            let mut next_line_index = 0;
                            let mut this_line_search_start = read_buffer.len() - n;
                            loop {
                                if let Some(mut eol_pos) = read_buffer[next_line_index+this_line_search_start..].iter().position(|&b| b == b'\n') {
                                    eol_pos += next_line_index+this_line_search_start;
                                    let line_bytes = &read_buffer[next_line_index..eol_pos];
                                    match Deserializer::deserialize(line_bytes) {
                                        Ok(remote_message) => {
                                            if let Err((abort_processor, processor_error_message)) = processor_sender.send(remote_message).await {
                                                // log & send the error message to the remote peer, if desired
                                                error!("`dialog_loop_for_textual_form()`: {} -- `dialog_processor` is full of unprocessed messages ({}/{})", processor_error_message, processor_sender.pending_items_count(), processor_sender.buffer_size());
                                                // inform the peer?
                                                if let Some(error_message_to_send) = LocalMessagesType::processor_error_message(processor_error_message) {
                                                    if let Err((abort_sender, error_msg_sender)) = peer.send_async(error_message_to_send).await {
                                                            warn!("dialog_loop_for_textual_form(): {error_msg_sender} -- Slow reader {:?}", peer);
                                                        if abort_sender {
                                                            socket_connection.report_closed();
                                                            break 'connection
                                                        }
                                                    }
                                                }
                                                if abort_processor {
                                                    socket_connection.report_closed();
                                                    break 'connection
                                                }
                                            }
                                        },
                                        Err(err) => {
                                            let stripped_line = String::from_utf8_lossy(line_bytes);
                                            let error_message = format!("Unknown command received from {:?} (peer id {}): '{}': {}",
                                                                            peer.peer_address, peer.peer_id, stripped_line, err);
                                            // log & send the error message to the remote peer
                                            warn!("`dialog_loop_for_textual_form()`:  {error_message}");
                                            if let Some(outgoing_error) = LocalMessagesType::processor_error_message(error_message) {
                                                if let Err((abort, error_msg)) = peer.send_async(outgoing_error).await {
                                                    if abort {
                                                        warn!("`dialog_loop_for_textual_form()`:  {error_msg} -- Slow reader {:?}", peer);
                                                        socket_connection.report_closed();
                                                        break 'connection
                                                    }
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
                                    // TODO: can we break the server (or client) if the message is too big / don't have a '\n'? TEST IT!
                                    break
                                }
                            }
                        },
                        Ok(_) /* zero bytes received -- the other end probably closed the connection */ => {
                            trace!("`dialog_loop_for_textual_form()`: EOF while reading from {:?} (peer id {}) -- it is out of bytes! Dropping the connection", peer.peer_address, peer.peer_id);
                            socket_connection.report_closed();
                            break 'connection
                        },
                        Err(err) if err.kind() == io::ErrorKind::WouldBlock => {},
                        Err(err) => {
                            error!("`dialog_loop_for_textual_form()`: ERROR in the connection with {:?} (peer id {}) while READING: '{:?}' -- dropping it", peer.peer_address, peer.peer_id, err);
                            socket_connection.report_closed();
                            break 'connection
                        },
                    }
                },
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::super::socket_connection_handler;
    use reactive_mutiny::prelude::advanced::{ChannelUniMoveAtomic, ChannelUniMoveFullSync, UniZeroCopyAtomic, UniZeroCopyFullSync};
    use crate::config::{ConstConfig, RetryingStrategies};
    use crate::serde::{ReactiveMessagingRonDeserializer, ReactiveMessagingRonSerializer};

    const DEFAULT_TEST_CONFIG: ConstConfig = ConstConfig {
        //retrying_strategy: RetryingStrategies::DoNotRetry,    // uncomment to see `message_flooding_throughput()` fail due to unsent messages
        retrying_strategy: RetryingStrategies::RetryYieldingForUpToMillis(30),
        ..ConstConfig::default()
    };
    const DEFAULT_TEST_CONFIG_U64:  u64              = DEFAULT_TEST_CONFIG.into();
    const DEFAULT_TEST_UNI_INSTRUMENTS: usize        = DEFAULT_TEST_CONFIG.executor_instruments.into();
    type AtomicTestUni<PayloadType = String>         = UniZeroCopyAtomic<PayloadType, {DEFAULT_TEST_CONFIG.receiver_channel_size as usize}, 1, DEFAULT_TEST_UNI_INSTRUMENTS>;
    type AtomicSenderChannel<PayloadType = String>   = ChannelUniMoveAtomic<PayloadType, {DEFAULT_TEST_CONFIG.sender_channel_size as usize}, 1>;
    type FullSyncTestUni<PayloadType = String>       = UniZeroCopyFullSync<PayloadType, {DEFAULT_TEST_CONFIG.receiver_channel_size as usize}, 1, DEFAULT_TEST_UNI_INSTRUMENTS>;
    type FullSyncSenderChannel<PayloadType = String> = ChannelUniMoveFullSync<PayloadType, {DEFAULT_TEST_CONFIG.sender_channel_size as usize}, 1>;


    // "unresponsive dialogs" tests
    ///////////////////////////////

    /// Performs the test [socket_connection_handler::tests::unresponsive_dialogs()] with the Atomic Uni channel
    #[cfg_attr(not(doc),tokio::test)]
    async fn unresponsive_dialogs_atomic_channel() {
        socket_connection_handler::tests::unresponsive_dialogs::<DEFAULT_TEST_CONFIG_U64, TextualDialog<DEFAULT_TEST_CONFIG_U64, String, String, ReactiveMessagingRonSerializer, ReactiveMessagingRonDeserializer, AtomicTestUni, AtomicSenderChannel, ()>>().await
    }

    /// Performs the test [socket_connection_handler::tests::unresponsive_dialogs()] with the FullSync Uni channel
    #[cfg_attr(not(doc),tokio::test)]
    async fn unresponsive_dialogs_fullsync_channel() {
        socket_connection_handler::tests::unresponsive_dialogs::<DEFAULT_TEST_CONFIG_U64, TextualDialog<DEFAULT_TEST_CONFIG_U64, String, String, ReactiveMessagingRonSerializer, ReactiveMessagingRonDeserializer, FullSyncTestUni, FullSyncSenderChannel, ()>>().await
    }


    // "responsive dialogs" tests
    /////////////////////////////

    /// Performs the test [socket_connection_handler::tests::responsive_dialogs()] with the Atomic Uni channel
    #[cfg_attr(not(doc),tokio::test)]
    async fn responsive_dialogs_atomic_channel() {
        socket_connection_handler::tests::responsive_dialogs::<DEFAULT_TEST_CONFIG_U64, TextualDialog<DEFAULT_TEST_CONFIG_U64, String, String, ReactiveMessagingRonSerializer, ReactiveMessagingRonDeserializer, AtomicTestUni, AtomicSenderChannel, ()>>().await
    }

    /// Performs the test [socket_connection_handler::tests::responsive_dialogs()] with the FullSync Uni channel
    #[cfg_attr(not(doc),tokio::test)]
    async fn responsive_dialogs_fullsync_channel() {
        socket_connection_handler::tests::responsive_dialogs::<DEFAULT_TEST_CONFIG_U64, TextualDialog<DEFAULT_TEST_CONFIG_U64, String, String, ReactiveMessagingRonSerializer, ReactiveMessagingRonDeserializer, FullSyncTestUni, FullSyncSenderChannel, ()>>().await
    }


    // "client termination" tests
    /////////////////////////////

    /// Performs the test [socket_connection_handler::tests::client_termination()] with the Atomic Uni channel
    #[cfg_attr(not(doc),tokio::test)]
    async fn client_termination_atomic_channel() {
        socket_connection_handler::tests::client_termination::<DEFAULT_TEST_CONFIG_U64, TextualDialog<DEFAULT_TEST_CONFIG_U64, String, String, ReactiveMessagingRonSerializer, ReactiveMessagingRonDeserializer, AtomicTestUni, AtomicSenderChannel, ()>>().await
    }

    /// Performs the test [socket_connection_handler::tests::client_termination()] with the FullSync Uni channel
    #[cfg_attr(not(doc),tokio::test)]
    async fn client_termination_fullsync_channel() {
        socket_connection_handler::tests::client_termination::<DEFAULT_TEST_CONFIG_U64, TextualDialog<DEFAULT_TEST_CONFIG_U64, String, String, ReactiveMessagingRonSerializer, ReactiveMessagingRonDeserializer, FullSyncTestUni, AtomicSenderChannel, ()>>().await
    }


    // "latency measurements" assertions
    ////////////////////////////////////

    /// Performs the measured test [socket_connection_handler::tests::latency_measurements()] with the Atomic Uni channel.\
    /// The values here are for a `12th Gen Intel(R) Core(TM) i7-1265U (12) @ 4,80 GHz` machine
    #[cfg_attr(not(doc),tokio::test(flavor = "multi_thread"))]
    #[ignore]   // convention for this project: ignored tests are to be run by a single thread
    async fn latency_measurements_atomic_channel() {
        const DEBUG_EXPECTED_COUNT: u32 = 77780;
        const RELEASE_EXPECTED_COUNT: u32 = 386504;
        const TOLERANCE: f64 = 0.10;

        socket_connection_handler::tests::latency_measurements::
            <DEFAULT_TEST_CONFIG_U64,
             TextualDialog<DEFAULT_TEST_CONFIG_U64, String, String, ReactiveMessagingRonSerializer, ReactiveMessagingRonDeserializer, AtomicTestUni, AtomicSenderChannel, ()>
            > (TOLERANCE, DEBUG_EXPECTED_COUNT, RELEASE_EXPECTED_COUNT).await
    }

    /// Performs the measured test [socket_connection_handler::tests::latency_measurements()] with the FullSync Uni channel.\
    /// The values here are for a `12th Gen Intel(R) Core(TM) i7-1265U (12) @ 4,80 GHz` machine
    #[cfg_attr(not(doc),tokio::test(flavor = "multi_thread"))]
    #[ignore]   // convention for this project: ignored tests are to be run by a single thread
    async fn latency_measurements_fullsync_channel() {
        const DEBUG_EXPECTED_COUNT: u32 = 77406;
        const RELEASE_EXPECTED_COUNT: u32 = 394358;
        const TOLERANCE: f64 = 0.10;

        socket_connection_handler::tests::latency_measurements::
            <DEFAULT_TEST_CONFIG_U64,
             TextualDialog<DEFAULT_TEST_CONFIG_U64, String, String, ReactiveMessagingRonSerializer, ReactiveMessagingRonDeserializer, FullSyncTestUni, FullSyncSenderChannel, ()>
            > (TOLERANCE, DEBUG_EXPECTED_COUNT, RELEASE_EXPECTED_COUNT).await
    }
    
    
    // "message_flooding_throughput" assertions
    ///////////////////////////////////////////

    /// Performs the measured test [socket_connection_handler::tests::message_flooding_throughput()] with the Atomic Uni channel.\
    /// The values here are for a `12th Gen Intel(R) Core(TM) i7-1265U (12) @ 4,80 GHz` machine
    #[cfg_attr(not(doc),tokio::test(flavor = "multi_thread"))]
    #[ignore]   // convention for this project: ignored tests are to be run by a single thread
    async fn message_flooding_throughput_atomic_channel() {
        const DEBUG_EXPECTED_COUNT: u32 = 1081344;
        const RELEASE_EXPECTED_COUNT: u32 = 1146880;
        const TOLERANCE: f64 = 0.10;

        socket_connection_handler::tests::message_flooding_throughput::
            <DEFAULT_TEST_CONFIG_U64,
             TextualDialog<DEFAULT_TEST_CONFIG_U64, String, String, ReactiveMessagingRonSerializer, ReactiveMessagingRonDeserializer, AtomicTestUni, AtomicSenderChannel, ()>
            > (TOLERANCE, DEBUG_EXPECTED_COUNT, RELEASE_EXPECTED_COUNT).await
    }

    /// Performs the measured test [socket_connection_handler::tests::message_flooding_throughput()] with the FullSync Uni channel.\
    /// The values here are for a `12th Gen Intel(R) Core(TM) i7-1265U (12) @ 4,80 GHz` machine
    #[cfg_attr(not(doc),tokio::test(flavor = "multi_thread"))]
    #[ignore]   // convention for this project: ignored tests are to be run by a single thread
    async fn message_flooding_throughput_fullsync_channel() {
        const DEBUG_EXPECTED_COUNT: u32 = 1081344;
        const RELEASE_EXPECTED_COUNT: u32 = 1146880;
        const TOLERANCE: f64 = 0.10;

        socket_connection_handler::tests::message_flooding_throughput::
            <DEFAULT_TEST_CONFIG_U64,
             TextualDialog<DEFAULT_TEST_CONFIG_U64, String, String, ReactiveMessagingRonSerializer, ReactiveMessagingRonDeserializer, FullSyncTestUni, FullSyncSenderChannel, ()>
            > (TOLERANCE, DEBUG_EXPECTED_COUNT, RELEASE_EXPECTED_COUNT).await
    }

}
