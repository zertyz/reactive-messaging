//! Provides the [new_socket_server!()] macro for instantiating servers, which may then be started with [spawn_unresponsive_server_processor!()] or [spawn_responsive_server_processor!()],
//! with each taking different variants of reactive processor logic.\
//! Both reactive processing logic variants will take in a `Stream` as parameter and should return another `Stream` as output:
//!   * Responsive: the reactive processor's output `Stream` yields items that are messages to be sent back to each client;
//!   * Unresponsive the reactive processor's output `Stream` won't be sent to the clients, allowing the `Stream`s to return items from any type.
//!
//! The Unresponsive processors still allow you to send messages to the client and should be preferred (as far as performance is concerned)
//! for protocols where the count of messages IN don't nearly map 1/1 with the count of messages OUT.
//!
//! For every server variant, the `Stream` items may be a combination of fallible/non-fallible (using `Result<>` or not) & future/non-future.
//!
//! Instead of using the mentioned macros, you might want take a look at [GenericSocketServer] to access the inner implementation directly
//! -- both ways have the same flexibility, but the macro version takes in all parameters in the conveniently packed and documented [ConstConfig]
//! struct, instead of requiring several const generic parameters.
//!
//! IMPLEMENTATION NOTE: There would be a common trait (that is no longer present) binding [SocketServer] to [GenericSocketServer]
//!                      -- the reason being Rust 1.71 still not supporting zero-cost async fn in traits.
//!                      Other API designs had been attempted, but using async in traits (through the `async-trait` crate) and the
//!                      necessary GATs to provide zero-cost-abstracts, revealed a compiler bug making writing processors very difficult to compile:
//!                        - https://github.com/rust-lang/rust/issues/96865


use crate::{
    socket_server::composite_socket_server::{
        new_composite_socket_server,
        spawn_responsive_composite_server_processor,
        spawn_unresponsive_composite_server_processor,
    },
    types::{
        ConnectionEvent,
        MessagingMutinyStream,
        ResponsiveMessages,
    },
    socket_connection::peer::Peer,
    ReactiveMessagingDeserializer,
    ReactiveMessagingSerializer,
    config::Channels,
};
use std::{
    fmt::Debug,
    future::Future,
    marker::PhantomData,
    sync::Arc,
};
use std::net::SocketAddr;
use futures::{future::BoxFuture, Stream};
use reactive_mutiny::prelude::advanced::{
    ChannelUniMoveAtomic,
    ChannelUniMoveCrossbeam,
    ChannelUniMoveFullSync,
    UniMoveCrossbeam,
    UniZeroCopyAtomic,
    UniZeroCopyFullSync,
    FullDuplexUniChannel,
    GenericUni,
};
use log::{error, warn};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;


/// Instantiates & allocates resources for a [GenericSocketServer], ready to be later started by [spawn_unresponsive_server_processor!()] or [spawn_responsive_server_processor!()].\
/// Params:
///   - `const_config`: [ConstConfig] -- the configurations for the server, enforcing const time optimizations;
///   - `interface_ip: IntoString` -- the interface to listen to incoming connections;
///   - `port: u16` -- the port to listen to incoming connections;
///   - `RemoteMessages`: [ReactiveMessagingDeserializer<>] -- the type of the messages produced by the clients;
///   - `LocalMessage`: [ReactiveMessagingSerializer<>] -- the type of the messages produced by this server -- should, additionally, implement the `Default` trait.
#[macro_export]
macro_rules! new_socket_server {
    ($const_config:    expr,
     $interface_ip:    expr,
     $port:            expr,
     $remote_messages: ty,
     $local_messages:  ty) => {
        new_composite_socket_server!($const_config, $interface_ip, $port, $remote_messages, $local_messages, ())
    }
}
pub use new_socket_server;


/// See [GenericSocketServer::spawn_unresponsive_processor()]
#[macro_export]
macro_rules! spawn_unresponsive_server_processor {
    ($socket_server:                expr,
     $connection_events_handler_fn: expr,
     $dialog_processor_builder_fn:  expr) => {{
        let connection_channel = spawn_unresponsive_composite_server_processor!($socket_server, $connection_events_handler_fn, $dialog_processor_builder_fn)?;
        $socket_server.start_with_single_protocol(connection_channel).await
    }}
}
pub use spawn_unresponsive_server_processor;


/// See [GenericSocketServer::spawn_responsive_processor()]
#[macro_export]
macro_rules! spawn_responsive_server_processor {
    ($socket_server:                expr,
     $connection_events_handler_fn: expr,
     $dialog_processor_builder_fn:  expr) => {{
        let connection_channel = spawn_responsive_composite_server_processor!($socket_server, $connection_events_handler_fn, $dialog_processor_builder_fn)?;
        $socket_server.start_with_single_protocol(connection_channel).await
    }}
}
pub use spawn_responsive_server_processor;


/// Unit tests the [socket_server](self) module
#[cfg(any(test,doc))]
mod tests {
    use super::*;
    use crate::prelude::{
        ConstConfig,
        CompositeSocketServer,
        GenericCompositeSocketServer,
        SocketClient,
        GenericSocketClient,
        new_socket_client,
        ron_deserializer,
        ron_serializer,
        spawn_responsive_client_processor,
    };
    use std::{
        future,
        ops::Deref,
        sync::atomic::{AtomicU32, Ordering::Relaxed},
        time::Duration,
    };
    use serde::{Deserialize, Serialize};
    use futures::StreamExt;
    use tokio::sync::Mutex;


    /// Test that our instantiation macro is able to produce servers backed by all possible channel types
    #[cfg_attr(not(doc),test)]
    fn instantiation() {
        let atomic_server = new_socket_server!(
            ConstConfig {
                channel: Channels::Atomic,
                ..ConstConfig::default()
            },
            "127.0.0.1", 8040, String, String);
        assert!(matches!(atomic_server, CompositeSocketServer::Atomic(_)), "an Atomic Server couldn't be instantiated");

        let fullsync_server  = new_socket_server!(
            ConstConfig {
                channel: Channels::FullSync,
                ..ConstConfig::default()
            },
            "127.0.0.1", 8041, String, String);
        assert!(matches!(fullsync_server, CompositeSocketServer::FullSync(_)), "a FullSync Server couldn't be instantiated");

        let crossbeam_server = new_socket_server!(
            ConstConfig {
                channel: Channels::Crossbeam,
                ..ConstConfig::default()
            },
            "127.0.0.1", 8042, String, String);
        assert!(matches!(crossbeam_server, CompositeSocketServer::Crossbeam(_)), "a Crossbeam Server couldn't be instantiated");
    }

    /// Test that our server types are ready for usage
    #[cfg_attr(not(doc),tokio::test)]
    async fn doc_usage() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

        // demonstrates how to build an unresponsive server
        ///////////////////////////////////////////////////
        // using fully typed generic functions that will work with all possible configs
        let mut server = new_socket_server!(
            ConstConfig::default(),
            "127.0.0.1",
            8043,
            DummyClientAndServerMessages,
            DummyClientAndServerMessages);
        spawn_unresponsive_server_processor!(server,
            connection_events_handler,
            unresponsive_processor
        )?;
        async fn connection_events_handler<const CONFIG:  u64,
                                           LocalMessages: ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
                                           SenderChannel: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync>
                                          (_event: ConnectionEvent<CONFIG, LocalMessages, SenderChannel>) {
        }
        fn unresponsive_processor<const CONFIG:   u64,
                                  LocalMessages:  ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
                                  SenderChannel:  FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync,
                                  StreamItemType: Deref<Target=DummyClientAndServerMessages>>
                                 (_client_addr:           String,
                                  _connected_port:        u16,
                                  _peer:                  Arc<Peer<CONFIG, LocalMessages, SenderChannel>>,
                                  client_messages_stream: impl Stream<Item=StreamItemType>)
                                 -> impl Stream<Item=()> {
            client_messages_stream.map(|_payload| ())
        }
        let termination_waiter = server.termination_waiter();
        server.terminate().await?;
        termination_waiter().await?;

        // demonstrates how to build a responsive server
        ////////////////////////////////////////////////
        // using fully typed generic functions that will work with all possible configs
        let mut server = new_socket_server!(
            ConstConfig::default(),
            "127.0.0.1",
            8043,
            DummyClientAndServerMessages,
            DummyClientAndServerMessages);
        spawn_responsive_server_processor!(server,
            connection_events_handler,
            responsive_processor
        )?;
        fn responsive_processor<const CONFIG:   u64,
                                SenderChannel:  FullDuplexUniChannel<ItemType=DummyClientAndServerMessages, DerivedItemType=DummyClientAndServerMessages> + Send + Sync,
                                StreamItemType: Deref<Target=DummyClientAndServerMessages>>
                               (_client_addr:           String,
                                _connected_port:        u16,
                                _peer:                  Arc<Peer<CONFIG, DummyClientAndServerMessages, SenderChannel>>,
                                client_messages_stream: impl Stream<Item=StreamItemType>)
                               -> impl Stream<Item=DummyClientAndServerMessages> {
            client_messages_stream.map(|_payload| DummyClientAndServerMessages::FloodPing)
        }
        let termination_waiter = server.termination_waiter();
        server.terminate().await?;
        termination_waiter().await?;

        // demonstrates how to use it with closures -- also allowing for any channel in the configs
        ///////////////////////////////////////////////////////////////////////////////////////////
        let mut server = new_socket_server!(
            ConstConfig::default(),
            "127.0.0.1",
            8043,
            DummyClientAndServerMessages,
            DummyClientAndServerMessages);
        spawn_unresponsive_server_processor!(server,
            |_| future::ready(()),
            |_, _, _, client_messages_stream| client_messages_stream.map(|_payload| DummyClientAndServerMessages::FloodPing)
        )?;
        let termination_waiter = server.termination_waiter();
        server.terminate().await?;
        termination_waiter().await?;

        // demonstrates how to use the internal & generic implementation
        ////////////////////////////////////////////////////////////////
        // notice there may be a discrepancy in the `ConstConfig` you provide and the actual concrete types
        // you also provide for `UniProcessor` and `SenderChannel` -- therefore, this usage is not recommended
        const CONFIG: ConstConfig = ConstConfig {
            receiver_buffer:      2048,
            sender_buffer:        1024,
            channel:              Channels::FullSync,
            executor_instruments: reactive_mutiny::prelude::Instruments::LogsWithExpensiveMetrics,
            ..ConstConfig::default()
        };
        type ProcessorUniType = UniZeroCopyFullSync<DummyClientAndServerMessages, {CONFIG.receiver_buffer as usize}, 1, {CONFIG.executor_instruments.into()}>;
        type SenderChannelType = ChannelUniMoveFullSync<DummyClientAndServerMessages, {CONFIG.sender_buffer as usize}, 1>;
        let mut server = GenericCompositeSocketServer :: <{CONFIG.into()},
                                                                                      DummyClientAndServerMessages,
                                                                                      DummyClientAndServerMessages,
                                                                                      ProcessorUniType,
                                                                                      SenderChannelType,
                                                                                      ()>
                                                                                  :: new("127.0.0.1", 8043);
        let connection_channel = server.spawn_unresponsive_processor(
            |_| future::ready(()),
            |_, _, _, client_messages_stream| client_messages_stream.map(|_payload| DummyClientAndServerMessages::FloodPing)
        ).await?;
        server.start_with_single_protocol(connection_channel).await?;
        let termination_waiter = server.termination_waiter();
        server.terminate().await?;
        termination_waiter().await?;

        Ok(())
    }

    /// assures the shutdown process is able to:
    ///   1) communicate with all clients
    ///   2) wait for up to the given timeout for them to gracefully disconnect
    ///   3) forcibly disconnect, if needed
    ///   4) notify any waiter on the server (after all the above steps are done) within the given timeout
    #[cfg_attr(not(doc),tokio::test(flavor = "multi_thread"))]
    async fn shutdown_process() {
        const PORT: u16 = 8044;

        // the shutdown timeout, in milliseconds
        let expected_max_shutdown_duration_ms = 543;
        // the tollerance, in milliseconds -- a too small shutdown duration means the server didn't wait for the client's disconnection; too much (possibly eternal) means it didn't enforce the timeout
        let max_time_ms = 20;

        // sensors
        let client_received_messages_count_ref1 = Arc::new(AtomicU32::new(0));
        let client_received_messages_count_ref2 = Arc::clone(&client_received_messages_count_ref1);
        let server_received_messages_count_ref1 = Arc::new(AtomicU32::new(0));
        let server_received_messages_count_ref2 = Arc::clone(&server_received_messages_count_ref1);

        // start the server -- the test logic is here
        let client_peer_ref1 = Arc::new(Mutex::new(None));
        let client_peer_ref2 = Arc::clone(&client_peer_ref1);

        const CONFIG: ConstConfig = ConstConfig {
            channel: Channels::FullSync,
            ..ConstConfig::default()
        };
        type ProcessorUniType = UniZeroCopyFullSync<DummyClientAndServerMessages, {CONFIG.receiver_buffer as usize}, 1, {CONFIG.executor_instruments.into()}>;
        type SenderChannelType = ChannelUniMoveFullSync<DummyClientAndServerMessages, {CONFIG.sender_buffer as usize}, 1>;
        let mut server = GenericCompositeSocketServer :: <{CONFIG.into()},
                                                                                      DummyClientAndServerMessages,
                                                                                      DummyClientAndServerMessages,
                                                                                      ProcessorUniType,
                                                                                      SenderChannelType,
                                                                                      ()>
                                                                                  :: new("127.0.0.1", PORT);
        let connection_channel = server.spawn_responsive_processor(
            move |connection_event: ConnectionEvent<{CONFIG.into()}, DummyClientAndServerMessages, SenderChannelType>| {
                let client_peer = Arc::clone(&client_peer_ref1);
                async move {
                    match connection_event {
                        ConnectionEvent::PeerConnected { peer } => {
                            // register the client -- which will initiate the server shutdown further down in this test
                            client_peer.lock().await.replace(peer);
                        },
                        ConnectionEvent::PeerDisconnected { peer: _, stream_stats: _ } => (),
                        ConnectionEvent::LocalServiceTermination => {
                            // send a message to the client (the first message, actually... that will initiate a flood of back-and-forth messages)
                            // then try to close the connection (which would only be gracefully done once all messages were sent... which may never happen).
                            let client_peer = client_peer.lock().await;
                            let client_peer = client_peer.as_ref().expect("No client is connected");
                            // send the flood starting message
                            let _ = client_peer.send_async(DummyClientAndServerMessages::FloodPing).await;
                            client_peer.flush_and_close(Duration::ZERO).await;
                        }
                    }
                }
            },
            move |_, _, _, client_messages: MessagingMutinyStream<ProcessorUniType>| {
                let server_received_messages_count = Arc::clone(&server_received_messages_count_ref1);
                client_messages.map(move |client_message| {
                    std::mem::forget(client_message);   // TODO 2023-07-15: investigate this reactive-mutiny related bug: it seems OgreUnique doesn't like the fact that this type doesn't need dropping? (no internal strings)... or is it a reactive-messaging bug?
                    server_received_messages_count.fetch_add(1, Relaxed);
                    DummyClientAndServerMessages::FloodPing
                })
            }
        ).await.expect("Spawning the server processor");
        server.start_with_single_protocol(connection_channel).await.expect("Starting the server");

        // start a client that will engage in a flood ping with the server when provoked (never closing the connection)
        let mut client = new_socket_client!(
            ConstConfig::default(),
            "127.0.0.1",
            PORT,
            DummyClientAndServerMessages,
            DummyClientAndServerMessages);
        spawn_responsive_client_processor!(
            client,
            |_| async {},
            move |_, _, _, server_messages| {
                let client_received_messages_count = Arc::clone(&client_received_messages_count_ref1);
                server_messages.map(move |server_message| {
                    std::mem::forget(server_message);   // TODO 2023-07-15: investigate this reactive-mutiny related bug: it seems OgreUnique doesn't like the fact that this type doesn't need dropping? (no internal strings)... or is it a reactive-messaging bug?
                    client_received_messages_count.fetch_add(1, Relaxed);
                    DummyClientAndServerMessages::FloodPing
                })
            }
        ).expect("Starting the client");

        // wait for the client to connect
        while client_peer_ref2.lock().await.is_none() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        // shutdown the server & wait until the shutdown process is complete
        let wait_for_server_termination = server.termination_waiter();
        server.terminate().await
            .expect("ERROR Signaling the server of the shutdown intention");
        let start = std::time::SystemTime::now();
        _ = tokio::time::timeout(Duration::from_secs(5), wait_for_server_termination()).await
            .expect("ERROR Waiting for the server to live it's life and to complete the termination process");
        let elapsed_ms = start.elapsed().unwrap().as_millis();
        assert!(client_received_messages_count_ref2.load(Relaxed) > 1, "The client didn't receive any messages (not even the 'server is shutting down' notification)");
        assert!(server_received_messages_count_ref2.load(Relaxed) > 1, "The server didn't receive any messages (not even 'gracefully disconnecting' after being notified that the server is shutting down)");
        assert!(elapsed_ms <= max_time_ms as u128,
                "The server shutdown (of a never complying client) didn't complete in a reasonable time, meaning the shutdown code is wrong. Maximum acceptable time: {}ms; Measured Time: {}ms",
                max_time_ms, elapsed_ms);
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
    enum DummyClientAndServerMessages {
        #[default]
        FloodPing,
    }

    impl Deref for DummyClientAndServerMessages {
        type Target = DummyClientAndServerMessages;
        fn deref(&self) -> &Self::Target {
            self
        }
    }

    impl ReactiveMessagingSerializer<DummyClientAndServerMessages> for DummyClientAndServerMessages {
        #[inline(always)]
        fn serialize(remote_message: &DummyClientAndServerMessages, buffer: &mut Vec<u8>) {
            ron_serializer(remote_message, buffer)
                .expect("socket_server.rs unit tests: No errors should have happened here!")
        }
        #[inline(always)]
        fn processor_error_message(err: String) -> DummyClientAndServerMessages {
            panic!("socket_server.rs unit tests: protocol error when none should have happened: {err}");
        }
    }
    impl ResponsiveMessages<DummyClientAndServerMessages> for DummyClientAndServerMessages {
        #[inline(always)]
        fn is_disconnect_message(_processor_answer: &DummyClientAndServerMessages) -> bool {
            false
        }
        #[inline(always)]
        fn is_no_answer_message(_processor_answer: &DummyClientAndServerMessages) -> bool {
            false
        }
    }
    impl ReactiveMessagingDeserializer<DummyClientAndServerMessages> for DummyClientAndServerMessages {
        #[inline(always)]
        fn deserialize(local_message: &[u8]) -> Result<DummyClientAndServerMessages, Box<dyn std::error::Error + Sync + Send>> {
            ron_deserializer(local_message)
        }
    }
}