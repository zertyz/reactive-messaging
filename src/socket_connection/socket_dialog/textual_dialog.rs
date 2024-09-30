use std::error::Error;
use std::fmt::Debug;
use std::future::Future;
use std::marker::PhantomData;
use std::ops::RangeInclusive;
use std::sync::Arc;
use futures::StreamExt;
use reactive_mutiny::prelude::{FullDuplexUniChannel, GenericUni};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use crate::prelude::{Peer, SocketConnection};
use crate::serde::{ReactiveMessagingDeserializer, ReactiveMessagingSerializer};
use crate::socket_connection::common::ReactiveMessagingUniSender;
use crate::socket_connection::socket_dialog::dialog_types::SocketDialog;
use log::{debug, error, trace, warn};
use tokio::io;

pub struct TextualDialog<const CONFIG:       u64,
                         RemoteMessagesType: ReactiveMessagingDeserializer<RemoteMessagesType>                                   + Send + Sync + PartialEq + Debug + 'static,
                         LocalMessagesType:  ReactiveMessagingSerializer<LocalMessagesType>                                      + Send + Sync + PartialEq + Debug + 'static,
                         ProcessorUniType:   GenericUni<ItemType=RemoteMessagesType>                                             + Send + Sync                     + 'static,
                         SenderChannelType:  FullDuplexUniChannel<ItemType=LocalMessagesType, DerivedItemType=LocalMessagesType> + Send + Sync                     + 'static,
                         StateType:                                                                                                Send + Sync + Clone     + Debug + 'static = ()
                        > {
    _phantom_data: PhantomData<(RemoteMessagesType, LocalMessagesType, ProcessorUniType, SenderChannelType, StateType)>,
}

impl<const CONFIG: u64,
     RemoteMessagesType: ReactiveMessagingDeserializer<RemoteMessagesType>                                   + Send + Sync + PartialEq + Debug + 'static,
     LocalMessagesType:  ReactiveMessagingSerializer<LocalMessagesType>                                      + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:   GenericUni<ItemType=RemoteMessagesType>                                             + Send + Sync                     + 'static,
     SenderChannelType:  FullDuplexUniChannel<ItemType=LocalMessagesType, DerivedItemType=LocalMessagesType> + Send + Sync                     + 'static,
     StateType:                                                                                                Send + Sync + Clone     + Debug + 'static,
    >
Default
for TextualDialog<CONFIG, RemoteMessagesType, LocalMessagesType, ProcessorUniType, SenderChannelType, StateType> {
    fn default() -> Self {
        Self {
            _phantom_data: PhantomData,
        }
    }
}

impl<const CONFIG: u64,
     RemoteMessagesType: ReactiveMessagingDeserializer<RemoteMessagesType>                                   + Send + Sync + PartialEq + Debug + 'static,
     LocalMessagesType:  ReactiveMessagingSerializer<LocalMessagesType>                                      + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:   GenericUni<ItemType=RemoteMessagesType>                                             + Send + Sync                     + 'static,
     SenderChannelType:  FullDuplexUniChannel<ItemType=LocalMessagesType, DerivedItemType=LocalMessagesType> + Send + Sync                     + 'static,
     StateType:                                                                                                Send + Sync + Clone     + Debug + 'static,
    >
SocketDialog<CONFIG>
for TextualDialog<CONFIG, RemoteMessagesType, LocalMessagesType, ProcessorUniType, SenderChannelType, StateType> {
    type RemoteMessages = RemoteMessagesType;
    type LocalMessages = LocalMessagesType;
    type ProcessorUni  = ProcessorUniType;
    type SenderChannel = SenderChannelType;
    type State         = StateType;

    /// Dialog loop specialist for text-based message forms, where each in & out event/command/sentence ends in '\n'.\
    /// `max_line_size` is the limit length of the lines that can be parsed (including the '\n' delimiter): if bigger
    /// lines come in, the dialog will end in error.
    #[inline(always)]
    async fn dialog_loop(self,
                         socket_connection:     &mut SocketConnection<StateType>,
                         peer:                  &Arc<Peer<CONFIG, Self::LocalMessages, Self::SenderChannel, StateType>>,
                         processor_sender:      &ReactiveMessagingUniSender<CONFIG, Self::RemoteMessages, <<Self as SocketDialog<CONFIG>>::ProcessorUni as GenericUni>::DerivedItemType, Self::ProcessorUni>,
                         payload_size_range:    RangeInclusive<u32>)

                         -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

        // sanity checks on payload sizes
        let max_line_size = *payload_size_range.end();
        if max_line_size < 4 {
            return Err(Box::from(format!("Textual DIalog Loop: the given `max_line_size` for the payload is too small (only {max_line_size} bytes) and this is probably a BUG in your program")))
        }

        let mut read_buffer = Vec::with_capacity(max_line_size as usize);
        let mut serialization_buffer = Vec::with_capacity(max_line_size as usize);

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
                            LocalMessagesType::serialize_textual(&to_send, &mut serialization_buffer);
                            serialization_buffer.push(b'\n');
                            // send
                            if let Err(err) = socket_connection.connection_mut().write_all(&serialization_buffer).await {
                                warn!("`dialog_loop_for_textual_form()`: PROBLEM in the connection with {peer:#?} while WRITING '{to_send:?}': {err:?}");
                                socket_connection.report_closed();
                                break 'connection
                            }
                        },
                        None => {
                            debug!("`dialog_loop_for_textual_form()`: Sender for {peer:#?} ended (most likely, either `peer.flush_and_close()` or `peer.cancel_and_close()` was called on the `peer`)");
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
                                    match RemoteMessagesType::deserialize_textual(line_bytes) {
                                        Ok(remote_message) => {
                                            if let Err((abort_processor, error_msg_processor)) = processor_sender.send(remote_message).await {
                                                // log & send the error message to the remote peer
                                                error!("`dialog_loop_for_textual_form()`: {} -- `dialog_processor` is full of unprocessed messages ({}/{})", error_msg_processor, processor_sender.pending_items_count(), processor_sender.buffer_size());
                                                if let Err((abort_sender, error_msg_sender)) = peer.send_async(LocalMessagesType::processor_error_message(error_msg_processor)).await {
                                                        warn!("dialog_loop_for_textual_form(): {error_msg_sender} -- Slow reader {:?}", peer);
                                                    if abort_sender {
                                                        socket_connection.report_closed();
                                                        break 'connection
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
                                            let outgoing_error = LocalMessagesType::processor_error_message(error_message);
                                            if let Err((abort, error_msg)) = peer.send_async(outgoing_error).await {
                                                if abort {
                                                    warn!("`dialog_loop_for_textual_form()`:  {error_msg} -- Slow reader {:?}", peer);
                                                    socket_connection.report_closed();
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
                        Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {},
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
    use crate::config::{ConstConfig, MessageForms, RetryingStrategies};

    const DEFAULT_TEST_CONFIG: ConstConfig = ConstConfig {
        //retrying_strategy: RetryingStrategies::DoNotRetry,    // uncomment to see `message_flooding_throughput()` fail due to unsent messages
        retrying_strategy: RetryingStrategies::RetryYieldingForUpToMillis(30),
        message_form: MessageForms::Textual { max_size: 1024 }, // IMPORTANT: this is not enforced in this module!!
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
        socket_connection_handler::tests::unresponsive_dialogs::<DEFAULT_TEST_CONFIG_U64, TextualDialog<DEFAULT_TEST_CONFIG_U64, String, String, AtomicTestUni, AtomicSenderChannel, ()>>().await
    }

    /// Performs the test [socket_connection_handler::tests::unresponsive_dialogs()] with the FullSync Uni channel
    #[cfg_attr(not(doc),tokio::test)]
    async fn unresponsive_dialogs_fullsync_channel() {
        socket_connection_handler::tests::unresponsive_dialogs::<DEFAULT_TEST_CONFIG_U64, TextualDialog<DEFAULT_TEST_CONFIG_U64, String, String, FullSyncTestUni, FullSyncSenderChannel, ()>>().await
    }


    // "responsive dialogs" tests
    /////////////////////////////

    /// Performs the test [socket_connection_handler::tests::responsive_dialogs()] with the Atomic Uni channel
    #[cfg_attr(not(doc),tokio::test)]
    async fn responsive_dialogs_atomic_channel() {
        socket_connection_handler::tests::responsive_dialogs::<DEFAULT_TEST_CONFIG_U64, TextualDialog<DEFAULT_TEST_CONFIG_U64, String, String, AtomicTestUni, AtomicSenderChannel, ()>>().await
    }

    /// Performs the test [socket_connection_handler::tests::responsive_dialogs()] with the FullSync Uni channel
    #[cfg_attr(not(doc),tokio::test)]
    async fn responsive_dialogs_fullsync_channel() {
        socket_connection_handler::tests::responsive_dialogs::<DEFAULT_TEST_CONFIG_U64, TextualDialog<DEFAULT_TEST_CONFIG_U64, String, String, FullSyncTestUni, FullSyncSenderChannel, ()>>().await
    }


    // "client termination" tests
    /////////////////////////////

    /// Performs the test [socket_connection_handler::tests::client_termination()] with the Atomic Uni channel
    #[cfg_attr(not(doc),tokio::test)]
    async fn client_termination_atomic_channel() {
        socket_connection_handler::tests::client_termination::<DEFAULT_TEST_CONFIG_U64, TextualDialog<DEFAULT_TEST_CONFIG_U64, String, String, AtomicTestUni, AtomicSenderChannel, ()>>().await
    }

    /// Performs the test [socket_connection_handler::tests::client_termination()] with the FullSync Uni channel
    #[cfg_attr(not(doc),tokio::test)]
    async fn client_termination_fullsync_channel() {
        socket_connection_handler::tests::client_termination::<DEFAULT_TEST_CONFIG_U64, TextualDialog<DEFAULT_TEST_CONFIG_U64, String, String, FullSyncTestUni, AtomicSenderChannel, ()>>().await
    }


    // "latency measurements" assertions
    ////////////////////////////////////

    /// Performs the measured test [socket_connection_handler::tests::latency_measurements()] with the Atomic Uni channel.\
    /// The values here are for a `Intel(R) Core(TM) i5-4200U CPU @ 1.60GHz` machine
    #[cfg_attr(not(doc),tokio::test(flavor = "multi_thread"))]
    #[ignore]   // convention for this project: ignored tests are to be run by a single thread
    async fn latency_measurements_atomic_channel() {
        const DEBUG_EXPECTED_COUNT: u32 = 81815;
        const RELEASE_EXPECTED_COUNT: u32 = 397491;
        const TOLERANCE: f64 = 0.10;

        socket_connection_handler::tests::latency_measurements::
            <DEFAULT_TEST_CONFIG_U64,
             TextualDialog<DEFAULT_TEST_CONFIG_U64, String, String, AtomicTestUni, AtomicSenderChannel, ()>
            > (TOLERANCE, DEBUG_EXPECTED_COUNT, RELEASE_EXPECTED_COUNT).await
    }

    /// Performs the measured test [socket_connection_handler::tests::latency_measurements()] with the FullSync Uni channel.\
    /// The values here are for a `Intel(R) Core(TM) i5-4200U CPU @ 1.60GHz` machine
    #[cfg_attr(not(doc),tokio::test(flavor = "multi_thread"))]
    #[ignore]   // convention for this project: ignored tests are to be run by a single thread
    async fn latency_measurements_fullsync_channel() {
        const DEBUG_EXPECTED_COUNT: u32 = 81815;
        const RELEASE_EXPECTED_COUNT: u32 = 398196;
        const TOLERANCE: f64 = 0.10;

        socket_connection_handler::tests::latency_measurements::
            <DEFAULT_TEST_CONFIG_U64,
             TextualDialog<DEFAULT_TEST_CONFIG_U64, String, String, FullSyncTestUni, FullSyncSenderChannel, ()>
            > (TOLERANCE, DEBUG_EXPECTED_COUNT, RELEASE_EXPECTED_COUNT).await
    }
    
    
    // "message_flooding_throughput" assertions
    ///////////////////////////////////////////

    /// Performs the measured test [socket_connection_handler::tests::message_flooding_throughput()] with the Atomic Uni channel.\
    /// The values here are for a `Intel(R) Core(TM) i5-4200U CPU @ 1.60GHz` machine
    #[cfg_attr(not(doc),tokio::test(flavor = "multi_thread"))]
    #[ignore]   // convention for this project: ignored tests are to be run by a single thread
    async fn message_flooding_throughput_atomic_channel() {
        const DEBUG_EXPECTED_COUNT: u32 = 1146880;
        const RELEASE_EXPECTED_COUNT: u32 = 1245184;
        const TOLERANCE: f64 = 0.10;

        socket_connection_handler::tests::message_flooding_throughput::
            <DEFAULT_TEST_CONFIG_U64,
             TextualDialog<DEFAULT_TEST_CONFIG_U64, String, String, AtomicTestUni, AtomicSenderChannel, ()>
            > (TOLERANCE, DEBUG_EXPECTED_COUNT, RELEASE_EXPECTED_COUNT).await
    }

    /// Performs the measured test [socket_connection_handler::tests::message_flooding_throughput()] with the FullSync Uni channel.\
    /// The values here are for a `Intel(R) Core(TM) i5-4200U CPU @ 1.60GHz` machine
    #[cfg_attr(not(doc),tokio::test(flavor = "multi_thread"))]
    #[ignore]   // convention for this project: ignored tests are to be run by a single thread
    async fn message_flooding_throughput_fullsync_channel() {
        const DEBUG_EXPECTED_COUNT: u32 = 1146880;
        const RELEASE_EXPECTED_COUNT: u32 = 1245184;
        const TOLERANCE: f64 = 0.10;

        socket_connection_handler::tests::message_flooding_throughput::
            <DEFAULT_TEST_CONFIG_U64,
             TextualDialog<DEFAULT_TEST_CONFIG_U64, String, String, FullSyncTestUni, FullSyncSenderChannel, ()>
            > (TOLERANCE, DEBUG_EXPECTED_COUNT, RELEASE_EXPECTED_COUNT).await
    }

}