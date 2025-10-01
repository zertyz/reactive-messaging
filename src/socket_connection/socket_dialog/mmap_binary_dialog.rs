use std::fmt::Debug;
use std::io;
use std::marker::PhantomData;
use std::sync::Arc;
use futures::StreamExt;
use log::{debug, error, trace, warn};
use reactive_mutiny::types::FullDuplexUniChannel;
use reactive_mutiny::uni::GenericUni;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use crate::config::ConstConfig;
use crate::prelude::{Peer, SocketConnection};
use crate::serde::{ReactiveMessagingConfig, ReactiveMessagingMemoryMappable};
use crate::socket_connection::common::ReactiveMessagingUniSender;
use crate::socket_connection::socket_dialog::dialog_types::SocketDialog;


pub struct MmapBinaryDialog<const CONFIG:       u64,
                            RemoteMessagesType: ReactiveMessagingMemoryMappable                                                     + Send + Sync + PartialEq + Debug + 'static,
                            LocalMessagesType:  ReactiveMessagingMemoryMappable + ReactiveMessagingConfig<LocalMessagesType>        + Send + Sync + PartialEq + Debug + 'static,
                            ProcessorUniType:   GenericUni<ItemType=RemoteMessagesType>                                             + Send + Sync                     + 'static,
                            SenderChannelType:  FullDuplexUniChannel<ItemType=LocalMessagesType, DerivedItemType=LocalMessagesType> + Send + Sync                     + 'static,
                            StateType:                                                                                                Send + Sync + Clone     + Debug + 'static = ()
                           > {
    _phantom_data: PhantomData<(RemoteMessagesType, LocalMessagesType, ProcessorUniType, SenderChannelType, StateType)>,
}

impl<const CONFIG:      u64,
    RemoteMessagesType: ReactiveMessagingMemoryMappable                                                     + Send + Sync + PartialEq + Debug + 'static,
    LocalMessagesType:  ReactiveMessagingMemoryMappable + ReactiveMessagingConfig<LocalMessagesType>        + Send + Sync + PartialEq + Debug + 'static,
    ProcessorUniType:   GenericUni<ItemType=RemoteMessagesType>                                             + Send + Sync                     + 'static,
    SenderChannelType:  FullDuplexUniChannel<ItemType=LocalMessagesType, DerivedItemType=LocalMessagesType> + Send + Sync                     + 'static,
    StateType:                                                                                                Send + Sync + Clone     + Debug + 'static
>
MmapBinaryDialog<CONFIG, RemoteMessagesType, LocalMessagesType, ProcessorUniType, SenderChannelType, StateType> {
    const CONST_CONFIG: ConstConfig = ConstConfig::from(CONFIG);
}

impl<const CONFIG:       u64,
     RemoteMessagesType: ReactiveMessagingMemoryMappable                                                     + Send + Sync + PartialEq + Debug + 'static,
     LocalMessagesType:  ReactiveMessagingMemoryMappable + ReactiveMessagingConfig<LocalMessagesType>        + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:   GenericUni<ItemType=RemoteMessagesType>                                             + Send + Sync                     + 'static,
     SenderChannelType:  FullDuplexUniChannel<ItemType=LocalMessagesType, DerivedItemType=LocalMessagesType> + Send + Sync                     + 'static,
     StateType:                                                                                                Send + Sync + Clone     + Debug + 'static
    >
Default
for MmapBinaryDialog<CONFIG, RemoteMessagesType, LocalMessagesType, ProcessorUniType, SenderChannelType, StateType> {
    fn default() -> Self {
        Self {
            _phantom_data: PhantomData,
        }
    }
}

impl<const CONFIG:       u64,
     RemoteMessagesType: ReactiveMessagingMemoryMappable                                                     + Send + Sync + PartialEq + Debug + 'static,
     LocalMessagesType:  ReactiveMessagingMemoryMappable + ReactiveMessagingConfig<LocalMessagesType>        + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:   GenericUni<ItemType=RemoteMessagesType>                                             + Send + Sync                     + 'static,
     SenderChannelType:  FullDuplexUniChannel<ItemType=LocalMessagesType, DerivedItemType=LocalMessagesType> + Send + Sync                     + 'static,
     StateType:                                                                                                Send + Sync + Clone     + Debug + 'static
    >
SocketDialog<CONFIG>
for MmapBinaryDialog<CONFIG, RemoteMessagesType, LocalMessagesType, ProcessorUniType, SenderChannelType, StateType> {
    type RemoteMessages = RemoteMessagesType;
    type DeserializedRemoteMessages = RemoteMessagesType;
    type LocalMessages = LocalMessagesType;
    type ProcessorUni  = ProcessorUniType;
    type SenderChannel = SenderChannelType;
    type State         = StateType;

    /// Dialog loop specialist for fixed-binary message forms, where each in or out event/command/parcel have the same size.
    #[inline(always)]
    async fn dialog_loop(self,
                         socket_connection:     &mut SocketConnection<StateType>,
                         peer:                  &Arc<Peer<CONFIG, Self::LocalMessages, Self::SenderChannel, StateType>>,
                         processor_sender:      &ReactiveMessagingUniSender<CONFIG, Self::RemoteMessages, <<Self as SocketDialog<CONFIG>>::ProcessorUni as GenericUni>::DerivedItemType, Self::ProcessorUni>)

                        -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

        // bad, Rust 1.76, bad... I cannot use const here: "error[E0401]: can't use generic parameters from outer item"
        let local_payload_size = std::mem::size_of::<LocalMessagesType>();
        let remote_payload_size = std::mem::size_of::<RemoteMessagesType>();

        // sanity checks on payload sizes
        debug_assert!(remote_payload_size as u32 <= Self::CONST_CONFIG.receiver_max_msg_size, "MmapBinary Dialog Loop: the given `CONST_CONFIG.receiver_max_msg_size` for the payload is too small (only {} bytes where {} would be needed) and this is probably a BUG in your program", Self::CONST_CONFIG.receiver_max_msg_size, remote_payload_size);
        debug_assert!(local_payload_size  as u32 <= Self::CONST_CONFIG.sender_max_msg_size,   "MmapBinary Dialog Loop: the given `CONST_CONFIG.sender_max_msg_size` for the payload is too small (only {} bytes where {} would be needed) and this is probably a BUG in your program", Self::CONST_CONFIG.sender_max_msg_size, local_payload_size);

        let (mut sender_stream, _) = peer.create_stream();

        // helper functions
        ///////////////////

        let allocate_reader_slot = || {
            let slot = processor_sender.reserve_slot().unwrap_or_else(|| panic!("Add retrying code here, bailing out because no more slots left. PROCESSOR SENDER buffer size is {} and {} are used", processor_sender.buffer_size(), processor_sender.pending_items_count()));
/*
            if let Err((abort_processor, error_msg_processor)) = processor_sender.send(remote_message).await {
                // log & send the error message to the remote peer
                error!("`dialog_loop_for_fixed_binary_form()`: {} -- `dialog_processor` is full of unprocessed messages ({}/{})", error_msg_processor, processor_sender.pending_items_count(), processor_sender.buffer_size());
                if let Err((abort_sender, error_msg_sender)) = peer.send_async(LocalMessagesType::processor_error_message(error_msg_processor)).await {
                        warn!("dialog_loop_for_fixed_binary_form(): {error_msg_sender} -- Slow reader {:?}", peer);
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
 */
            let bytes_buffer = unsafe {
                let ptr = (slot as *mut RemoteMessagesType).cast::<u8>();
                let len = remote_payload_size;
                std::slice::from_raw_parts_mut(ptr, len)
            };
            (slot, bytes_buffer, 0usize)
        };

        // The place where the incoming messages should be put
        let (mut reader_slot, mut reader_bytes_buffer, mut received_len) = allocate_reader_slot();

        'connection: loop {
            // wait for the socket to be readable or until we have something to write
            tokio::select!(

                biased;     // sending has priority over receiving

                // send?
                to_send = sender_stream.next() => {
                    match to_send {
                        Some(to_send) => {
                            // "serialize" -- actually, only get the pointer to the bytes, as we are zero-copy here
                            let to_send_bytes = unsafe {
                                let ptr = (&to_send as *const LocalMessagesType).cast::<u8>();
                                let len = local_payload_size;
                                std::slice::from_raw_parts(ptr, len)
                            };
                            // send
                            if let Err(err) = socket_connection.connection_mut().write_all(to_send_bytes).await {
                                warn!("`dialog_loop()` for mmap_binary: PROBLEM in the connection with {peer:#?} while WRITING '{to_send:?}': {err:?}");
                                socket_connection.report_closed();
                                break 'connection
                            }
// eprintln!(">>>> SENT: {to_send:?}");
                        },
                        None => {
                            debug!("`dialog_loop()` for mmap_binary: Sender for {peer:#?} ended (most likely, either `peer.flush_and_close()` or `peer.cancel_and_close()` was called on the `peer`");
                            break 'connection
                        }
                    }
                },

                // receive?
                read = socket_connection.connection_mut().read_buf(&mut reader_bytes_buffer) => {
                    match read {
                        Ok(n) if n > 0 => {
                            received_len += n;
                            if received_len == remote_payload_size {
// eprintln!("<<<< RECEIVED: {reader_slot:#?}");
                                _ = processor_sender.try_send_reserved(reader_slot);
                                // TODO: retry if sending was not possible
                                // prepare for the next read
                                (reader_slot, reader_bytes_buffer, received_len) = allocate_reader_slot();
                            }
                        },
                        Ok(_) /* zero bytes received -- the other end probably closed the connection */ => {
                            trace!("`dialog_loop_for_fixed_binary_form()`: EOF while reading from {:?} (peer id {}) -- it is out of bytes! Dropping the connection", peer.peer_address, peer.peer_id);
                            socket_connection.report_closed();
                            break 'connection
                        },
                        Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {},
                        Err(err) => {
                            error!("`dialog_loop_for_fixed_binary_form()`: ERROR in the connection with {:?} (peer id {}) while READING: '{:?}' -- dropping it", peer.peer_address, peer.peer_id, err);
                            socket_connection.report_closed();
                            break 'connection
                        },
                    }
                },
            );
        }
        _ = processor_sender.try_cancel_reservation(reader_slot);
        // TODO: retry the above
        Ok(())
    }

}

#[cfg(test)]
mod tests {
    use std::future;
    use std::net::ToSocketAddrs;
    use std::sync::atomic::AtomicU32;
    use std::sync::atomic::Ordering::Relaxed;
    use std::time::Duration;
    use super::*;
    use crate::prelude::{ConstConfig, ServerConnectionHandler};
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
    async fn memory_mapped_binary_messages() {

        #[derive(Debug, PartialEq, Default)]
        struct Mmappable {
            count: u32,
        }
        impl ReactiveMessagingMemoryMappable for Mmappable {}
        impl ReactiveMessagingConfig<Mmappable> for Mmappable {}
    
        const TIMEOUT: Duration = Duration::from_millis(1000);
        const LISTENING_INTERFACE: &str = "127.0.0.1";
        const COUNT_LIMIT        : u32 = 100;
        const EXPECTED_SUM       : u32 = (COUNT_LIMIT / 2) * (COUNT_LIMIT + 1);
        let port = next_server_port();
        let observed_sum = Arc::new(AtomicU32::new(0));
        let observed_sum_clone = observed_sum.clone();
        let (_client_shutdown_sender, client_shutdown_receiver) = tokio::sync::broadcast::channel(1);
    
        // server
        let mut connection_provider = ServerConnectionHandler::new(LISTENING_INTERFACE, port, ()).await
            .expect("Sanity Check: couldn't start the Connection Provider server event loop");
        let new_connections_source = connection_provider.connection_receiver()
            .expect("Sanity Check: couldn't move the Connection Receiver out of the Connection Provider");
        let socket_communications_handler = SocketConnectionHandler::<DEFAULT_TEST_CONFIG_U64, MmapBinaryDialog<DEFAULT_TEST_CONFIG_U64, Mmappable, Mmappable, AtomicTestUni<Mmappable>, AtomicSenderChannel<Mmappable>, ()>>::new(MmapBinaryDialog::default());
        let (returned_connections_sink, mut _server_connections_source) = tokio::sync::mpsc::channel::<SocketConnection<()>>(2);
        socket_communications_handler.server_loop(
            LISTENING_INTERFACE, port, new_connections_source, returned_connections_sink,
            |connection_event| {
                match connection_event {
                    ProtocolEvent::PeerArrived { peer } => {
                        assert!(peer.send(Mmappable { count: 1 }).is_ok(), "couldn't send");
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
                        assert!(peer.send(Mmappable { count: client_message.count + 1 }).is_ok(), "server couldn't send");
                    }
                })
            }
        ).await.expect("Starting the server");
    
        println!("### Waiting a little for the server to start...");
        tokio::time::sleep(Duration::from_millis(10)).await;
    
        // client
        let tokio_connection = TcpStream::connect(format!("{}:{}", LISTENING_INTERFACE, port).to_socket_addrs().expect("Error resolving address").next().unwrap()).await.expect("Error connecting");
        let socket_connection = SocketConnection::new(tokio_connection, ());
        let client_communications_handler = SocketConnectionHandler::<DEFAULT_TEST_CONFIG_U64, MmapBinaryDialog<DEFAULT_TEST_CONFIG_U64, Mmappable, Mmappable, FullSyncTestUni<Mmappable>, FullSyncSenderChannel<Mmappable>, ()>>::new(MmapBinaryDialog::default());
        let client_task = tokio::spawn(
            client_communications_handler.client(
                socket_connection, client_shutdown_receiver,
                move |_connection_event| future::ready(()),
                move |_client_addr, _client_port, peer, server_messages_stream| {
                    let observed_sum = observed_sum_clone.clone();
                    server_messages_stream.inspect(move |server_message| {
                        println!("Server said: {:?}", server_message);
                        observed_sum.fetch_add(server_message.count, Relaxed);
                        if server_message.count >= COUNT_LIMIT {
                            peer.cancel_and_close();
                        } else {
                            assert!(peer.send(Mmappable { count: server_message.count }).is_ok(), "client couldn't send");
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
}