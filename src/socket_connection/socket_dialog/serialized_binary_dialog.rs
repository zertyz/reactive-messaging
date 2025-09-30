use std::fmt::{Debug, Formatter};
use std::io::IoSlice;
use std::marker::PhantomData;
use std::ops::Deref;
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

pub struct SerializedBinaryDialog<const CONFIG:       u64,
                                  RemoteMessagesType:                                                                                       Send + Sync + PartialEq + Debug + 'static,
                                  LocalMessagesType:  ReactiveMessagingConfig<LocalMessagesType>                                          + Send + Sync + PartialEq + Debug + 'static,
                                  Serializer:         ReactiveMessagingSerializer<LocalMessagesType>,
                                  Deserializer:       ReactiveMessagingDeserializer<RemoteMessagesType>,
                                  ProcessorUniType:   GenericUni<ItemType=SerializedWrapperType<RemoteMessagesType, Deserializer>>        + Send + Sync                     + 'static,
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
    ProcessorUniType:   GenericUni<ItemType=SerializedWrapperType<RemoteMessagesType, Deserializer>>        + Send + Sync                     + 'static,
    SenderChannelType:  FullDuplexUniChannel<ItemType=LocalMessagesType, DerivedItemType=LocalMessagesType> + Send + Sync                     + 'static,
    StateType:                                                                                                Send + Sync + Clone     + Debug + 'static,
>
SerializedBinaryDialog<CONFIG, RemoteMessagesType, LocalMessagesType, Serializer, Deserializer, ProcessorUniType, SenderChannelType, StateType> {
    const CONST_CONFIG: ConstConfig = ConstConfig::from(CONFIG);
}

impl<const CONFIG: u64,
     RemoteMessagesType:                                                                                       Send + Sync + PartialEq + Debug + 'static,
     LocalMessagesType:  ReactiveMessagingConfig<LocalMessagesType>                                          + Send + Sync + PartialEq + Debug + 'static,
     Serializer:         ReactiveMessagingSerializer<LocalMessagesType>,
     Deserializer:       ReactiveMessagingDeserializer<RemoteMessagesType>,
     ProcessorUniType:   GenericUni<ItemType=SerializedWrapperType<RemoteMessagesType, Deserializer>>        + Send + Sync                     + 'static,
     SenderChannelType:  FullDuplexUniChannel<ItemType=LocalMessagesType, DerivedItemType=LocalMessagesType> + Send + Sync                     + 'static,
     StateType:                                                                                                Send + Sync + Clone     + Debug + 'static,
    >
Default
for SerializedBinaryDialog<CONFIG, RemoteMessagesType, LocalMessagesType, Serializer, Deserializer, ProcessorUniType, SenderChannelType, StateType> {
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
     ProcessorUniType:   GenericUni<ItemType=SerializedWrapperType<RemoteMessagesType, Deserializer>>        + Send + Sync                     + 'static,
     SenderChannelType:  FullDuplexUniChannel<ItemType=LocalMessagesType, DerivedItemType=LocalMessagesType> + Send + Sync                     + 'static,
     StateType:                                                                                                Send + Sync + Clone     + Debug + 'static,
    >
SocketDialog<CONFIG>
for SerializedBinaryDialog<CONFIG, RemoteMessagesType, LocalMessagesType, Serializer, Deserializer, ProcessorUniType, SenderChannelType, StateType> {
    type RemoteMessages = RemoteMessagesType;
    type DeserializedRemoteMessages = SerializedWrapperType<RemoteMessagesType, Deserializer>;
    type LocalMessages = LocalMessagesType;
    type ProcessorUni  = ProcessorUniType;
    type SenderChannel = SenderChannelType;
    type State         = StateType;

    /// Dialog loop specialized in variable size binary message forms, where each in or out event/command/parcel is preceded by the payload size.
    #[inline(always)]
    async fn dialog_loop(self,
                         socket_connection:     &mut SocketConnection<StateType>,
                         peer:                  &Arc<Peer<CONFIG, Self::LocalMessages, Self::SenderChannel, StateType>>,
                         processor_sender:      &ReactiveMessagingUniSender<CONFIG, Self::DeserializedRemoteMessages, <<Self as SocketDialog<CONFIG>>::ProcessorUni as GenericUni>::DerivedItemType, Self::ProcessorUni>)

                        -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

        let payload_size_len = min_bytes_for_usize(Self::CONST_CONFIG.receiver_max_msg_size as usize);

        let mut payload_size_buffer  = Vec::with_capacity(payload_size_len);
        // let mut serialization_buffer = Vec::with_capacity(Self::CONST_CONFIG.sender_max_msg_size as usize);
        let mut serialization_buffer = Vec::new();  // grow as needed instead of limiting it to `Self::CONST_CONFIG.sender_max_msg_size`

        // sanity checks on payload sizes
        debug_assert!(payload_size_len > 0,                        "Serialized Binary Dialog Loop: the given `CONST_CONFIG.receiver_max_msg_size` lead to a zero-sized `payload_size_buffer`. This is likely a BUG in the `reactive-messaging` crate :(");
        debug_assert!(Self::CONST_CONFIG.sender_max_msg_size >= 4, "Serialized Binary Dialog Loop: the given `CONST_CONFIG.sender_max_msg_size` for the payload is too small (only {} bytes) and this is probably a BUG in your program", serialization_buffer.len());

        let (mut sender_stream, _) = peer.create_stream();

        // reading happens in 2 steps: first we read the payload size (which, in its own right, is a small fixed-size read)
        // then we read the binary payload itself.
        enum ReadState<'a> {
            WaitingForPayloadSize { payload_size_buffer: &'a mut Vec<u8> },     // this is always a reference to `payload_size_buffer` to avoid unnecessary allocations
            ReadingPayload        { payload_read_buffer: Vec<u8> },             // this is always a freshly allocated `Vec`
        }
        let mut read_state = ReadState::WaitingForPayloadSize { payload_size_buffer: &mut payload_size_buffer};

        'connection: loop {

            // determine the `read_buffer` -- into which we will read data until it is totally filled up
            let read_buffer = match &mut read_state {
                ReadState::WaitingForPayloadSize { payload_size_buffer } => {
                    payload_size_buffer.clear();
                    payload_size_buffer
                },
                ReadState::ReadingPayload { payload_read_buffer } => payload_read_buffer,
            };

            // wait for the socket to be readable or until we have something to write
            tokio::select!(

                biased;     // sending has priority over receiving

                // send?
                to_send = sender_stream.next() => {
                    match to_send {
                        Some(to_send) => {
                            // serialize
                            Serializer::serialize(&to_send, &mut serialization_buffer);
                            debug_assert!(serialization_buffer.len() < Self::CONST_CONFIG.sender_max_msg_size as usize, "Serialized Binary Dialog Loop: While semdomg a message, the `serialization_buffer` (now, len = {}) just exceeded the specified maximum `Self::CONST_CONFIG.sender_max_msg_size` of {}",
                                                                                                                        serialization_buffer.len(), Self::CONST_CONFIG.sender_max_msg_size);
                            // send
                            let serialized_payload_len_buffer = &serialization_buffer.len().to_le_bytes()[..payload_size_len];
                            let to_write = &[
                                IoSlice::new(serialized_payload_len_buffer),     // the payload size
                                IoSlice::new(&serialization_buffer),             // the payload
                            ];
                            if let Err(err) = socket_connection.connection_mut().write_vectored(to_write).await {
                                warn!("`dialog_loop()` for serialized binary: PROBLEM in the connection with {peer:#?} while WRITING '{to_send:?}': {err:?}");
                                socket_connection.report_closed();
                                break 'connection
                            }
                        },
                        None => {
                            debug!("`dialog_loop()` for serialized binary: Sender for {peer:#?} ended (most likely, either `peer.flush_and_close()` or `peer.cancel_and_close()` was called on the `peer`)");
                            break 'connection
                        }
                    }
                },

                // receive?

                read = socket_connection.connection_mut().read_buf(read_buffer) => {
                    match read {
                        Ok(n) if n > 0 => {
                            // data arrived
                            let expected_len = read_buffer.capacity();
                            let received_len = read_buffer.len();
                            debug_assert!(received_len <= expected_len, "Serialized Binary Dialog Loop: BUG! Our understanding of the Tokio async IO had changed -- Tokio is now extending the buffer upon reading data! Please, fix the logic here: received_len ({received_len}) <= expected_len ({expected_len})");
                            // did we finish reading whatever we were reading? either the payload size or the payload itself...
                            if received_len == expected_len {
                                // consume and determine the next `read_state`
                                read_state = match read_state {
                                    ReadState::WaitingForPayloadSize { payload_size_buffer } => {
                                        // we finished reading the next payload size: allocate the buffer to read the payload
                                        let expected_payload_len = decode_usize_min(payload_size_buffer);
                                        debug_assert!(expected_payload_len > 0, "Serialized Binary Dialog Loop: the client informed that the next message's len is ZERO. This is certainly a BUG there.");
                                        let payload_read_buffer = Vec::with_capacity(expected_payload_len);     // allocate the next `read_buffer`
                                        ReadState::ReadingPayload { payload_read_buffer }
                                    },
                                    ReadState::ReadingPayload { payload_read_buffer } => {
                                        // we finished reading the payload: emit the event and prepare to read a next payload size
                                        let remote_message = SerializedWrapperType::<RemoteMessagesType, Deserializer>::from_raw_unchecked(payload_read_buffer);
                                        if let Err((abort_processor, processor_error_message)) = processor_sender.send(remote_message).await {
                                            // log & send the error message to the remote peer, if desired
                                            error!("`dialog_loop()` for serialized binary: {} -- `dialog_processor` is full of unprocessed messages ({}/{})", processor_error_message, processor_sender.pending_items_count(), processor_sender.buffer_size());
                                            // inform the peer?
                                            if let Some(error_message_to_send) = LocalMessagesType::processor_error_message(processor_error_message) {
                                                if let Err((abort_sender, error_msg_sender)) = peer.send_async(error_message_to_send).await {
                                                    warn!("`dialog_loop()` for serialized binary: {error_msg_sender} -- Slow reader {:?}", peer);
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
                                        ReadState::WaitingForPayloadSize { payload_size_buffer: &mut payload_size_buffer}
                                    },
                                }
                            }

                        },
                        Ok(_) /* zero bytes received -- the other end probably closed the connection */ => {
                            trace!("`dialog_loop()` for serialized binary: EOF while reading the payload size from {:?} (peer id {}) -- it is out of bytes! Dropping the connection", peer.peer_address, peer.peer_id);
                            socket_connection.report_closed();
                            break 'connection
                        },
                        Err(err) if err.kind() == io::ErrorKind::WouldBlock => {},
                        Err(err) => {
                            error!("`dialog_loop()` for serialized binary: ERROR in the connection with {:?} (peer id {}) while READING the payload size: '{:?}' -- dropping it", peer.peer_address, peer.peer_id, err);
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

// utils
////////

/// Tells the minimum number of bytes you need to use to represent the given usize `value`.\
/// In other words, how many significant bytes would be different from 0 when representing that usize `value`.
const fn min_bytes_for_usize(mut value: usize) -> usize {
    let mut count = 0;
    while value != 0 {
        count += 1;
        value >>= 8;
    }
    count
}

/// Reverse operation of `usize::to_le_bytes()` whose most significant bytes -- containing the value 0 -- where omitted.\
/// Meaning `bytes` can be of any length from `[0..8]`.
fn decode_usize_min(bytes: &[u8]) -> usize {
    let mut value = 0usize;
    for (i, &byte) in bytes.iter().enumerate() {
        value |= (byte as usize) << (i * 8);
    }
    value
}

// Our variable binary SERDE wrapper -- RKYV
////////////////////////////////////////////

/// NOTE: This simple wrapper type might be ready for use in production
#[derive(Default)]
#[repr(align(16))]  // keep our struct SIMD-friendly (same strategy as RKYV's `AlignedVec` uses
pub struct SerializedWrapperType<MessagesType: PartialEq + Debug,
                                 Deserializer: ReactiveMessagingDeserializer<MessagesType>> {
    raw: Vec<u8>,
    _phantom: PhantomData<(MessagesType, Deserializer)>,
}
impl<MessagesType: PartialEq + Debug,
     Deserializer: ReactiveMessagingDeserializer<MessagesType>>
SerializedWrapperType<MessagesType, Deserializer> {
    /// Intended for use while receiving
    /// -- when you are sure the source is trusty (or else the program may crash)
    #[inline(always)]
    pub fn from_raw_unchecked(raw: Vec<u8>) -> Self {
        Self {
            raw,
            _phantom: PhantomData,
        }
    }
    /// Intended for use while receiving
    /// -- when the source might send junk binary data: you'll pay the price for the validation
    #[inline(always)]
    pub fn try_from_raw(raw: Vec<u8>)
                       -> Result<Self, crate::prelude::Error> {
        Deserializer::validate(&raw)
            .map(|()| Self::from_raw_unchecked(raw))
    }

    /// Intended for use while sending
    /// -- allocates a `Vec` that will contain the serialized binary for `local_message`
    #[inline(always)]
    pub fn from_value<Serializer: ReactiveMessagingSerializer<MessagesType>>
                     (local_message: MessagesType) -> Self {
        let mut buffer = Vec::new();
        Serializer::serialize(&local_message, &mut buffer);
        Self {
            raw: buffer,
            _phantom: PhantomData,
        }
    }
}
impl<MessagesType: PartialEq + Debug,
     Deserializer: ReactiveMessagingDeserializer<MessagesType>>
Deref
for SerializedWrapperType<MessagesType, Deserializer> {
    // type Target = <MessagesType as ReactiveMessagingDeserializer<MessagesType>>::DeserializedRemoteMessages;
    type Target = Deserializer::DeserializedRemoteMessages;
    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        Deserializer::deserialize_as_ref(&self.raw)
            .unwrap_or_else(|err| panic!("`reactive_messaging::serialized_binary_dialog::SerializedWrapperType::deref()` BUG! The Variable Binary Deserializer should never return an error, but it returned: {}", err))
    }
}
impl<MessagesType: PartialEq + Debug,
     Deserializer: ReactiveMessagingDeserializer<MessagesType>>
AsRef<Deserializer::DeserializedRemoteMessages>
for SerializedWrapperType<MessagesType, Deserializer> {
    #[inline(always)]
    fn as_ref(&self) -> &Deserializer::DeserializedRemoteMessages {
        self.deref()
    }
}
impl<MessagesType: PartialEq + Debug,
     Deserializer: ReactiveMessagingDeserializer<MessagesType>>
Debug
for SerializedWrapperType<MessagesType, Deserializer> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.deref().fmt(f)
    }
}
impl<MessagesType: PartialEq + Debug,
     Deserializer: ReactiveMessagingDeserializer<MessagesType>>
PartialEq
for SerializedWrapperType<MessagesType, Deserializer> {
    fn eq(&self, other: &Self) -> bool {
        self.raw.eq(other.raw.as_slice())
    }
}


#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::future;
    use std::net::ToSocketAddrs;
    use std::sync::atomic::AtomicU32;
    use std::sync::atomic::Ordering::Relaxed;
    use std::time::Duration;
    use super::*;
    use crate::prelude::{ConstConfig, ReactiveMessagingRkyvFastDeserializer, ReactiveMessagingRkyvSerializer, ServerConnectionHandler};
    use crate::config::RetryingStrategies;
    use reactive_mutiny::prelude::advanced::{ChannelUniMoveAtomic, ChannelUniMoveFullSync, UniZeroCopyAtomic, UniZeroCopyFullSync};
    use tokio::net::TcpStream;
    use crate::socket_connection::socket_connection_handler::SocketConnectionHandler;
    use crate::types::ProtocolEvent;
    use crate::unit_test_utils::next_server_port;

    const DEFAULT_TEST_CONFIG: ConstConfig = ConstConfig {
        //retrying_strategy: RetryingStrategies::DoNotRetry,    // uncomment to see `message_flooding_throughput()` fail due to unsent messages
        retrying_strategy: RetryingStrategies::RetryYieldingForUpToMillis(30),
        ..ConstConfig::default()
    };
    const DEFAULT_TEST_CONFIG_U64:  u64       = DEFAULT_TEST_CONFIG.into();
    const DEFAULT_TEST_UNI_INSTRUMENTS: usize = DEFAULT_TEST_CONFIG.executor_instruments.into();
    type AtomicTestUni<PayloadType>           = UniZeroCopyAtomic<PayloadType, {DEFAULT_TEST_CONFIG.receiver_channel_size as usize}, 1, DEFAULT_TEST_UNI_INSTRUMENTS>;
    type AtomicSenderChannel<PayloadType>     = ChannelUniMoveAtomic<PayloadType, {DEFAULT_TEST_CONFIG.sender_channel_size as usize}, 1>;
    type FullSyncTestUni<PayloadType>         = UniZeroCopyFullSync<PayloadType, {DEFAULT_TEST_CONFIG.receiver_channel_size as usize}, 1, DEFAULT_TEST_UNI_INSTRUMENTS>;
    type FullSyncSenderChannel<PayloadType>   = ChannelUniMoveFullSync<PayloadType, {DEFAULT_TEST_CONFIG.sender_channel_size as usize}, 1>;


    #[cfg_attr(not(doc),tokio::test)]
    async fn serialized_binary_messages() {

        #[derive(Debug, PartialEq, rkyv::Serialize, rkyv::Deserialize, rkyv::Archive)]
        #[archive_attr(derive(Debug))]
        #[archive_attr(derive(PartialEq))]
        enum VariableBinary {
            ElementCounts(BTreeMap<String, u32>),
            Error(String),
        }
        impl Default for VariableBinary {
            fn default() -> Self {
                VariableBinary::Error(String::from("Channel slot not Initialized"))
            }
        }
        impl ReactiveMessagingConfig<VariableBinary> for VariableBinary {
            fn processor_error_message(err: String) -> Option<VariableBinary> {
                Some(VariableBinary::Error(err))
            }
        }

        const TIMEOUT: Duration = Duration::from_millis(1000);
        const LISTENING_INTERFACE: &str = "127.0.0.1";
        const COUNT_LIMIT        : u32 = 100;
        const EXPECTED_SUM       : u32 = (COUNT_LIMIT / 2) * (COUNT_LIMIT + 1);
        const MY_ELEMENT_NAME: &str = "MyElement";
        
        let port = next_server_port();
        let observed_sum = Arc::new(AtomicU32::new(0));
        let observed_sum_clone = observed_sum.clone();
        let (_client_shutdown_sender, client_shutdown_receiver) = tokio::sync::broadcast::channel(1);

        // server
        let mut connection_provider = ServerConnectionHandler::new(LISTENING_INTERFACE, port, ()).await
            .expect("Sanity Check: couldn't start the Connection Provider server event loop");
        let new_connections_source = connection_provider.connection_receiver()
            .expect("Sanity Check: couldn't move the Connection Receiver out of the Connection Provider");
        let socket_communications_handler = SocketConnectionHandler::<DEFAULT_TEST_CONFIG_U64, SerializedBinaryDialog<DEFAULT_TEST_CONFIG_U64, VariableBinary, VariableBinary, ReactiveMessagingRkyvSerializer, ReactiveMessagingRkyvFastDeserializer, AtomicTestUni<SerializedWrapperType<VariableBinary, ReactiveMessagingRkyvFastDeserializer>>, AtomicSenderChannel<VariableBinary>, ()>>::new(SerializedBinaryDialog::default());
        let (returned_connections_sink, mut _server_connections_source) = tokio::sync::mpsc::channel::<SocketConnection<()>>(2);
        socket_communications_handler.server_loop(
            LISTENING_INTERFACE, port, new_connections_source, returned_connections_sink,
            |connection_event| {
                match connection_event {
                    ProtocolEvent::PeerArrived { peer } => {
                        assert!(peer.send(VariableBinary::ElementCounts(BTreeMap::new())).is_ok(), "couldn't send");
                    },
                    ProtocolEvent::PeerLeft { peer: _, stream_stats: _ } => {},
                    ProtocolEvent::LocalServiceTermination => {
                        println!("Test Server: shutdown was requested... No connection will receive the drop message (nor will be even closed) because I, the lib caller, intentionally didn't keep track of the connected peers for this test!");
                    }
                }
                future::ready(())
            },
            move |_client_addr, _client_port, peer, client_messages_stream| {
                client_messages_stream.then(move |client_message| {
                    let peer = peer.clone();
                    async move {
                        match client_message.deref().deref() {
                            ArchivedVariableBinary::ElementCounts(client_element_counts) => {
                                let mut new_element_counts = BTreeMap::<String, u32>::from_iter(client_element_counts.into_iter().map(|(k, v)| (k.to_string(), *v)));
                                new_element_counts.entry(MY_ELEMENT_NAME.to_string())
                                    .and_modify(|count| *count += 1)
                                    .or_insert(1);
                                assert!(peer.send(VariableBinary::ElementCounts(new_element_counts)).is_ok(), "server couldn't send");
                            }
                            ArchivedVariableBinary::Error(err) => panic!("Client sent an error message: {err}"),
                        }
                    }
                })
            }
        ).await.expect("Starting the server");

        println!("### Waiting a little for the server to start...");
        tokio::time::sleep(Duration::from_millis(10)).await;

        // client
        let tokio_connection = TcpStream::connect(format!("{}:{}", LISTENING_INTERFACE, port).to_socket_addrs().expect("Error resolving address").into_iter().next().unwrap()).await.expect("Error connecting");
        let socket_connection = SocketConnection::new(tokio_connection, ());
        let client_communications_handler = SocketConnectionHandler::<DEFAULT_TEST_CONFIG_U64, SerializedBinaryDialog<DEFAULT_TEST_CONFIG_U64, VariableBinary, VariableBinary, ReactiveMessagingRkyvSerializer, ReactiveMessagingRkyvFastDeserializer, FullSyncTestUni<SerializedWrapperType<VariableBinary, ReactiveMessagingRkyvFastDeserializer>>, FullSyncSenderChannel<VariableBinary>, ()>>::new(SerializedBinaryDialog::default());
        let client_task = tokio::spawn(
            client_communications_handler.client(
                socket_connection, client_shutdown_receiver,
                move |_connection_event| future::ready(()),
                move |_client_addr, _client_port, peer, server_messages_stream| {
                    let observed_sum = observed_sum_clone.clone();
                    server_messages_stream.inspect(move |server_message| {
                        println!("Server said: {:?}", server_message.deref());
                        match server_message.deref().deref() {
                            ArchivedVariableBinary::ElementCounts(server_element_counts) => {
                                let count = *server_element_counts.get(MY_ELEMENT_NAME).unwrap_or(&0);
                                observed_sum.fetch_add(count, Relaxed);
                                if count >= COUNT_LIMIT {
                                    peer.cancel_and_close();
                                } else {
                                    let element_counts = BTreeMap::<String, u32>::from_iter(server_element_counts.into_iter().map(|(k, v)| (k.to_string(), *v)));
                                    assert!(peer.send(VariableBinary::ElementCounts(element_counts)).is_ok(), "client couldn't send");
                                }
                            },
                            ArchivedVariableBinary::Error(err) => panic!("Server sent an error message: {err}"),
                        }
                    })
                }
            )
        );
        println!("### Started a client -- which is running concurrently, in the background... it has {TIMEOUT:?} to do its thing!");

        // wait for the client, so no errors would go unnoticed
        tokio::time::timeout(TIMEOUT, client_task).await
            .expect("Client task timed out")
            .expect("Failed starting the client task")
            .expect("Client task logic resulted on error");

        assert_eq!(observed_sum.load(Relaxed), EXPECTED_SUM, "The sum of `count`s doesn't match");

    }


    #[cfg_attr(not(doc),test)]
    fn test_min_bytes_for_usize() {
        assert_eq!(0, min_bytes_for_usize(0));
        assert_eq!(1, min_bytes_for_usize(1));
        assert_eq!(1, min_bytes_for_usize(255));
        assert_eq!(2, min_bytes_for_usize(256));
        assert_eq!(8, min_bytes_for_usize(usize::MAX));
        assert_eq!(8, min_bytes_for_usize(1 << (64-7)));
    }
}