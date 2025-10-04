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

        // returns `Ok` if successful or `Err(err_msg)` if the connection is to be aborted -- after exhausting all possible retries and, possibly, informing the peer
        macro_rules! allocate_reader_slot {
            () => {{
                match processor_sender.reserve_slot().await {
                    Ok(slot) => {
                        let bytes_buffer = unsafe {
                            let ptr = (slot as *mut RemoteMessagesType).cast::<u8>();
                            let len = remote_payload_size;
                            std::slice::from_raw_parts_mut(ptr, len)
                        };
                        Ok((slot, bytes_buffer, 0usize))
                    },
                    Err((abort_processor, processor_error_message)) => {
                        // log & send the error message to the remote peer, if desired
                        error!("`dialog_loop() for mmap`: {} -- `dialog_processor` is full of unprocessed messages ({}/{})", processor_error_message, processor_sender.pending_items_count(), processor_sender.buffer_size());
                        let mut err = None;
                        // inform the peer?
                        if let Some(error_message_to_send) = LocalMessagesType::processor_error_message(processor_error_message.clone()) {
                            if let Err((abort_sender, error_msg_sender)) = peer.send_async(error_message_to_send).await {
                                let err_msg = format!("`dialog_loop() for mmap`: {error_msg_sender} -- Slow reader {:?}", peer);
                                warn!("{err_msg}");
                                if abort_sender {
                                    socket_connection.report_closed();
                                    err.replace(Err(err_msg));
                                }
                            }
                        }
                        if abort_processor {
                            socket_connection.report_closed();
                            _ = err.get_or_insert(Err(processor_error_message));
                        } else {
                            let err_msg = String::from("`dialog_loop() for mmap`: Couldn't reserve a slot to receive a next message. Aborting the connection even if the retryer said not to do so... as the last line of defense crumbled.");
                            warn!("{err_msg}");
                            _ = err.get_or_insert(Err(err_msg));
                        }
                        err.unwrap()
                    }
                }
            }}
        }

        // The place where the incoming messages should be put
        let (mut reader_slot, mut reader_bytes_buffer, mut received_len) = allocate_reader_slot!()?;

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
                        },
                        None => {
                            debug!("`dialog_loop()` for mmap_binary: Sender for {peer:#?} ended (most likely, either `peer.flush_and_close()` or `peer.cancel_and_close()` was called on the `peer`");
                            break 'connection
                        }
                    }
                },

                // receive?
                read = socket_connection.connection_mut().read(&mut reader_bytes_buffer[received_len..]) => {
                    match read {
                        Ok(n) if n > 0 => {
                            received_len += n;
                            if received_len == remote_payload_size {
                                processor_sender.send_reserved(reader_slot);
                                // prepare for the next read
                                (reader_slot, reader_bytes_buffer, received_len) = match allocate_reader_slot!() {
                                    Ok(val) => val,
                                    Err(_) => break 'connection,
                                };
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
        Ok(())
    }

}

#[cfg(test)]
mod tests {
    use std::future;
    use std::net::ToSocketAddrs;
    use std::sync::atomic::AtomicU32;
    use std::sync::atomic::Ordering::Relaxed;
    use std::time::{Duration, Instant};
    use super::*;
    use crate::prelude::{ConstConfig, ResponsiveStream, ServerConnectionHandler};
    use crate::config::RetryingStrategies;
    use reactive_mutiny::prelude::advanced::{ChannelUniMoveAtomic, ChannelUniMoveFullSync, UniZeroCopyAtomic, UniZeroCopyFullSync};
    use tokio::net::TcpStream;
    use tokio::sync::Mutex;
    use crate::socket_connection::socket_connection_handler::SocketConnectionHandler;
    use crate::types::ProtocolEvent;
    use crate::unit_test_utils::next_server_port;

    /// Happy path ping-pong usage
    #[cfg_attr(not(doc),tokio::test)]
    async fn memory_mapped_binary_messages() {

        const TIMEOUT: Duration = Duration::from_millis(1000);
        const COUNT_LIMIT        : u32 = 100;
        const EXPECTED_SUM       : u32 = (COUNT_LIMIT / 2) * (COUNT_LIMIT + 1);
        const LISTENING_INTERFACE: &str = "127.0.0.1";
        let port = next_server_port();

        // this is a ping test, so no buffers nor retrying is necessary
        const TEST_CONFIG: ConstConfig = ConstConfig {
            receiver_channel_size: 2,   // by using 2 on both we are sure never to be denied sending messages
            sender_channel_size: 2,     // to the client & server output & stream processors
            retrying_strategy: RetryingStrategies::DoNotRetry,
            // retrying_strategy: RetryingStrategies::RetryWithBackoffUpTo(3),    // Allow the above channel sizes to be 1
            ..ConstConfig::default()
        };
        const TEST_CONFIG_U64:      u64   = TEST_CONFIG.into();
        const TEST_UNI_INSTRUMENTS: usize = TEST_CONFIG.executor_instruments.into();
        type AtomicProcessorUni<PayloadType>      = UniZeroCopyAtomic<PayloadType, { TEST_CONFIG.receiver_channel_size as usize}, 1, TEST_UNI_INSTRUMENTS>;
        type AtomicSenderChannel<PayloadType>     = ChannelUniMoveAtomic<PayloadType, { TEST_CONFIG.sender_channel_size as usize}, 1>;
        type FullSyncProcessorUni<PayloadType>    = UniZeroCopyFullSync<PayloadType, { TEST_CONFIG.receiver_channel_size as usize}, 1, TEST_UNI_INSTRUMENTS>;
        type FullSyncSenderChannel<PayloadType>   = ChannelUniMoveFullSync<PayloadType, { TEST_CONFIG.sender_channel_size as usize}, 1>;


        let observed_sum = Arc::new(AtomicU32::new(0));
        let observed_sum_clone = observed_sum.clone();
        let (_client_shutdown_sender, client_shutdown_receiver) = tokio::sync::broadcast::channel(1);
    
        // server
        let mut connection_provider = ServerConnectionHandler::new(LISTENING_INTERFACE, port, ()).await
            .expect("Sanity Check: couldn't start the Connection Provider server event loop");
        let new_connections_source = connection_provider.connection_receiver()
            .expect("Sanity Check: couldn't move the Connection Receiver out of the Connection Provider");
        let socket_communications_handler = SocketConnectionHandler::<TEST_CONFIG_U64, MmapBinaryDialog<TEST_CONFIG_U64, Mmappable, Mmappable, AtomicProcessorUni<Mmappable>, AtomicSenderChannel<Mmappable>, ()>>::new(MmapBinaryDialog::default());
        let (returned_connections_sink, mut _server_connections_source) = tokio::sync::mpsc::channel::<SocketConnection<()>>(2);
        socket_communications_handler.server_loop(
            LISTENING_INTERFACE, port, new_connections_source, returned_connections_sink,
            |connection_event| async {
                match connection_event {
                    ProtocolEvent::PeerArrived { peer } => {
                        assert!(peer.send_async(Mmappable { n: 1 }).await.is_ok(), "couldn't send");
                    },
                    ProtocolEvent::PeerLeft { peer: _, stream_stats: _ } => {},
                    ProtocolEvent::LocalServiceTermination => {
                        println!("Test Server: shutdown was requested... No connection will receive the drop message (nor will be even closed) because I, the server creator, intentionally didn't keep track of the connected clients for this test -- nor my models account for a 'about to disconnect' message!");
                    }
                }
            },
            move |_client_addr, _client_port, peer, client_messages_stream| {
                client_messages_stream
                    .map(|client_message| Mmappable { n: client_message.n + 1 })
                    .to_responsive_stream(peer, |_, _| ())
            }
        ).await.expect("Starting the server");
    
        println!("### Waiting a little for the server to start...");
        tokio::time::sleep(Duration::from_millis(10)).await;
    
        // client
        let tokio_connection = TcpStream::connect(format!("{}:{}", LISTENING_INTERFACE, port).to_socket_addrs().expect("Error resolving address").next().unwrap()).await.expect("Error connecting");
        let socket_connection = SocketConnection::new(tokio_connection, ());
        let client_communications_handler = SocketConnectionHandler::<TEST_CONFIG_U64, MmapBinaryDialog<TEST_CONFIG_U64, Mmappable, Mmappable, FullSyncProcessorUni<Mmappable>, FullSyncSenderChannel<Mmappable>, ()>>::new(MmapBinaryDialog::default());
        let client_task = tokio::spawn(
            client_communications_handler.client(
                socket_connection, client_shutdown_receiver,
                move |_connection_event| future::ready(()),
                move |_client_addr, _client_port, peer, server_messages_stream| {
                    let observed_sum = observed_sum_clone.clone();
                    server_messages_stream
                        .inspect(move |server_message| { observed_sum.fetch_add(server_message.n, Relaxed); })
                        .take_while(|server_message| future::ready(server_message.n < COUNT_LIMIT))
                        .map(|server_message| Mmappable { n: server_message.n })
                        .to_responsive_stream(peer, |_, _| ())
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

        #[derive(Debug, PartialEq, Default)]
        struct Mmappable {
            n: u32,
        }
        impl ReactiveMessagingMemoryMappable for Mmappable {}
        impl ReactiveMessagingConfig<Mmappable> for Mmappable {}

    }

    /// Asserts we are able to send messages to a slow processing peer -- and not lose any of them due to the use of retrying strategies
    #[cfg_attr(not(doc),tokio::test)]
    async fn slow_reader() {

        const COUNT_LIMIT       : u32 = 1024;
        const EXPECTED_SUM      : u32 = (COUNT_LIMIT / 2) * (COUNT_LIMIT + 1);
        const SLOW_READER_MILLIS: u64 = 1;
        const TIMEOUT: Duration = Duration::from_millis(6 * COUNT_LIMIT as u64 * SLOW_READER_MILLIS + 1);   // we apply a big tolerance factor -- needed due to the Backoff + jitter retrying
        const LISTENING_INTERFACE: &str = "127.0.0.1";
        let port = next_server_port();

        // this is a flood test, have the buffers considerably smaller than the `COUNT_LIMIT`
        const TEST_CONFIG: ConstConfig = ConstConfig {
            receiver_channel_size: 1,
            sender_channel_size: 1, // set this > COUNT_LIMIT to test this test is really testing what it proposes to test -- it should fail and tell there is a bug in this test
            // retrying_strategy: RetryingStrategies::DoNotRetry,   // set this to see the test fail when the sending channel gets full
            retrying_strategy: RetryingStrategies::RetryWithBackoffUpTo(16/*SLOW_READER_MILLIS as u8 + 1*/),  // set this lower than `SLOW_READER_MILLIS` to also see this test fail
            receiver_max_msg_size: 1024*1024,
            sender_max_msg_size: 1024*1024,
            ..ConstConfig::default()
        };
        const TEST_CONFIG_U64:      u64   = TEST_CONFIG.into();
        const TEST_UNI_INSTRUMENTS: usize = TEST_CONFIG.executor_instruments.into();
        type AtomicProcessorUni<PayloadType>      = UniZeroCopyAtomic<PayloadType, { TEST_CONFIG.receiver_channel_size as usize}, 1, TEST_UNI_INSTRUMENTS>;
        type AtomicSenderChannel<PayloadType>     = ChannelUniMoveAtomic<PayloadType, { TEST_CONFIG.sender_channel_size as usize}, 1>;
        type FullSyncProcessorUni<PayloadType>    = UniZeroCopyFullSync<PayloadType, { TEST_CONFIG.receiver_channel_size as usize}, 1, TEST_UNI_INSTRUMENTS>;
        type FullSyncSenderChannel<PayloadType>   = ChannelUniMoveFullSync<PayloadType, { TEST_CONFIG.sender_channel_size as usize}, 1>;


        let server_observed_sum = Arc::new(AtomicU32::new(0));      // sum the values of the messages the client sends back to the server, acknowledging them as received
        let server_observed_sum_clone = server_observed_sum.clone();
        let client_observed_sum = Arc::new(AtomicU32::new(0));      // sum of the messages the client received
        let client_observed_sum_clone = client_observed_sum.clone();
        let (_client_shutdown_sender, client_shutdown_receiver) = tokio::sync::broadcast::channel(1);

        let server_burst_task = Arc::new(Mutex::new(None));
        let server_burst_task_clone = server_burst_task.clone();

        // server
        let mut connection_provider = ServerConnectionHandler::new(LISTENING_INTERFACE, port, ()).await
            .expect("Sanity Check: couldn't start the Connection Provider server event loop");
        let new_connections_source = connection_provider.connection_receiver()
            .expect("Sanity Check: couldn't move the Connection Receiver out of the Connection Provider");
        let socket_communications_handler = SocketConnectionHandler::<TEST_CONFIG_U64, MmapBinaryDialog<TEST_CONFIG_U64, Mmappable, Mmappable, AtomicProcessorUni<Mmappable>, AtomicSenderChannel<Mmappable>, ()>>::new(MmapBinaryDialog::default());
        let (returned_connections_sink, mut _server_connections_source) = tokio::sync::mpsc::channel::<SocketConnection<()>>(2);
        socket_communications_handler.server_loop(
            LISTENING_INTERFACE, port, new_connections_source, returned_connections_sink,
            move |connection_event| {
                let burst_task = server_burst_task_clone.clone();
                async move {
                    match connection_event {
                        ProtocolEvent::PeerArrived { peer } => {
                            // test requirement: upon connecting, the server will send a burst of messages
                            burst_task.lock().await.replace(tokio::spawn(async move {
                                let start = Instant::now();
                                for n in 1..=COUNT_LIMIT {
                                    assert_eq!(peer.send_async(Mmappable { n, ..Mmappable::default() }).await, Ok(()), "couldn't send element #{n}");
                                }
                                let elapsed = start.elapsed();
                                assert!(elapsed.as_millis() > SLOW_READER_MILLIS as u128, "Test bug: it seems the burst happened instantly -- we didn't see the 'slow reader' effects, as all the sending was done in only {elapsed:?}. Are the sender buffer too big?");
                                println!("### All the sending was performed in {elapsed:?} -- {}/{}", peer.pending_items_count(), peer.buffer_size());
                            }));
                        },
                        ProtocolEvent::PeerLeft { peer: _, stream_stats: _ } => {},
                        ProtocolEvent::LocalServiceTermination => {
                            println!("Test Server: shutdown was requested... No connection will receive the drop message (nor will be even closed) because I, the server creator, intentionally didn't keep track of the connected clients for this test -- nor my models account for a 'about to disconnect' message!");
                        }
                    }
                }
            },
            move |_client_addr, _client_port, _peer, client_messages_stream| {
                let server_observed_sum = server_observed_sum_clone.clone();
                client_messages_stream.inspect(move |client_message| { server_observed_sum.fetch_add(client_message.n, Relaxed); })
            }
        ).await.expect("Starting the server");

        println!("### Waiting a little for the server to start...");
        tokio::time::sleep(Duration::from_millis(10)).await;

        // client
        let tokio_connection = TcpStream::connect(format!("{}:{}", LISTENING_INTERFACE, port).to_socket_addrs().expect("Error resolving address").next().unwrap()).await.expect("Error connecting");
        let socket_connection = SocketConnection::new(tokio_connection, ());
        let client_communications_handler = SocketConnectionHandler::<TEST_CONFIG_U64, MmapBinaryDialog<TEST_CONFIG_U64, Mmappable, Mmappable, FullSyncProcessorUni<Mmappable>, FullSyncSenderChannel<Mmappable>, ()>>::new(MmapBinaryDialog::default());
        let _start = Instant::now();
        let client_task = tokio::spawn(
            client_communications_handler.client(
                socket_connection, client_shutdown_receiver,
                move |_connection_event| future::ready(()),
                move |_client_addr, _client_port, peer, server_messages_stream| {
                    let observed_sum = client_observed_sum_clone.clone();
                    server_messages_stream.then(move |server_message| {
                        let peer = peer.clone();
                        // println!("Server sent: {} -- {:?}", server_message.n, _start.elapsed());
                        observed_sum.fetch_add(server_message.n, Relaxed);
                        async move {
                            // test requirement: this client is a slow reader
                            tokio::time::sleep(Duration::from_millis(SLOW_READER_MILLIS)).await;
                            // acknowledges the message sent by the server
                            assert!(peer.send_async(Mmappable { n: server_message.n, ..Mmappable::default() }).await.is_ok(), "client couldn't send");
                            if server_message.n == COUNT_LIMIT {
                                peer.flush_and_close(Duration::from_millis(SLOW_READER_MILLIS)).await;
                            }
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

        assert_eq!(server_observed_sum.load(Relaxed), EXPECTED_SUM, "The sum of `count`s doesn't match");
        assert_eq!(client_observed_sum.load(Relaxed), EXPECTED_SUM, "The sum of `count`s doesn't match");

        let mut server_burst_task = server_burst_task.lock().await;
        if let Some(server_burst_task) = server_burst_task.take() {
            server_burst_task.await.expect("Server Burst task panicked");
        } else {
            panic!("Test BUG: The burst task was empty!");
        }

        #[derive(Debug, PartialEq)]
        struct Mmappable {
            n: u32,
            /// We keep a big object because the kernel itself buffers socket data
            /// -- this value minimizes the number of objects we must send for the
            /// slow reader effects to be perceived.
            extra_data: [u8; 6*1024],
        }
        impl ReactiveMessagingMemoryMappable for Mmappable {}
        impl ReactiveMessagingConfig<Mmappable> for Mmappable {}

        impl Default for Mmappable {
            fn default() -> Self {
                Self {
                    n: 0,
                    extra_data: [0; 6*1024],
                }
            }
        }

    }

}