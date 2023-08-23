//! Provides a server whose processor's output `Stream` won't be sent to the clients.\
//! See also [super::responsive_socket_server].
//!
//! The implementations here still allow you to send messages to the client and should be preferred (as far as performance is concerned)
//! if the messages in don't nearly map 1/1 with the messages out.


use crate::{ReactiveMessagingDeserializer, ReactiveMessagingSerializer};
use std::fmt::Debug;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;
use std::error::Error;
use reactive_mutiny::prelude::advanced::{
    ChannelUniMoveAtomic,
    ChannelUniMoveCrossbeam,
    ChannelUniMoveFullSync,
    UniMoveCrossbeam,
    UniZeroCopyAtomic,
    UniZeroCopyFullSync,
    FullDuplexUniChannel,
    ChannelCommon,
    ChannelProducer,
    GenericUni,
};
use async_trait::async_trait;
use futures::future::BoxFuture;
use futures::Stream;
use log::warn;
use crate::config::{Channels, ConstConfig};
use crate::prelude::{ConnectionEvent, MessagingMutinyStream};
use crate::socket_connection::{Peer, SocketConnectionHandler};
use crate::socket_server::common::upgrade_to_shutdown_tracking;
use crate::types::{ReactiveProcessorController, ReactiveUnresponsiveProcessor, ReactiveUnresponsiveProcessorAssociatedTypes};
use crate::socket_connection::common::{RetryableSender, ReactiveMessagingSender};


/// Instantiates & allocate resources for a [UnresponsiveSocketServer], ready to be later started.\
/// A Unresponsive Socket Server is defined by its `dialog_processor_builder_fn()`, which won't produce a `Stream` of messages
/// to be sent to the clients. To send messages, a call to [Peer::send()] must be done.\
/// See [new_responsive_socket_server!()] if your dialog model is near 1-to-1 regarding requests & answers.\
/// Params:
///   - `const_config: ConstConfig` -- the configurations for the server, enforcing const time optimizations;
///   - `interface_ip: IntoString` -- the interface to listen to incoming connections;
///   - `port: u16` -- the port to listen to incoming connections;
///   - `RemoteMessages` -- the type of the messages produced by the clients. See [ReactiveUnresponsiveProcessorAssociatedTypes] for the traits it should implement;
///   - `LocalMessage` -- the type of the messages produced by this server. See [ReactiveUnresponsiveProcessorAssociatedTypes] for the traits it should implement.
///   - `connection_events_handle_fn` -- the generic function to handle connected, disconnected and shutdown events (possibly to manage sessions). Sign it as:
///     ```nocompile
///      async fn connection_events_handler<SenderChannelType: FullDuplexUniChannel<ItemType=LocalMessages,
///                                                                                 DerivedItemType=LocalMessages> + Sync + Send + 'static>
///                                        (event: ConnectionEvent<SenderChannelType>)
///     ```
///   - `dialog_processor_builder_fn` -- the generic function that receives the `Stream` of client messages and returns another `Stream` of unspecified items
///                                       -- called once for each client. Notice sending messages to the client should be done through the `peer` parameter.
///                                      Sign the function as:
///     ```nocompile
///     fn processor<SenderChannelType: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Sync + Send + 'static,
///                  StreamItemType:    Deref<Target=RemoteMessages>>
///                 (client_addr:            String,
///                  connected_port:         u16,
///                  peer:                   Arc<Peer<SenderChannelType>>,
///                  client_messages_stream: impl Stream<Item=StreamItemType>)
///                 -> impl Stream<Item=ANY_TYPE>
///     ```
#[macro_export]
macro_rules! new_unresponsive_socket_server {
    ($const_config:    expr,
     $interface_ip:    expr,
     $port:            expr,
     $remote_messages: ty,
     $local_messages:  ty,
     $connection_events_handle_fn: expr,
     $dialog_processor_builder_fn: expr) => {
        {
            use crate::socket_server::unresponsive_socket_server::UnresponsiveSocketServer;
            const CONFIG:                    usize = $const_config.into();
            const PROCESSOR_BUFFER:          usize = $const_config.receiver_buffer as usize;
            const PROCESSOR_UNI_INSTRUMENTS: usize = $const_config.executor_instruments.into();
            const SENDER_BUFFER:             usize = $const_config.sender_buffer   as usize;
            match $const_config.channel {
                Channels::Atomic => {
                    let server = UnresponsiveSocketServer::<CONFIG,
                                                            $remote_messages,
                                                            $local_messages,
                                                            UniZeroCopyAtomic<$remote_messages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                                            ReactiveMessagingSender<CONFIG, $local_messages, ChannelUniMoveAtomic<$local_messages, SENDER_BUFFER, 1>> >
                                                         ::new($interface_ip.to_string(), $port);
                    server.spawn_unresponsive_processor($connection_events_handle_fn, $dialog_processor_builder_fn).await
                        .map_err(|err| format!("UnresponsiveSocketServer: error starting server with configs «{:?}»: {:?}", $const_config, err))
                },
                Channels::FullSync => {
                    let server = UnresponsiveSocketServer::<CONFIG,
                                                            $remote_messages,
                                                            $local_messages,
                                                            UniZeroCopyFullSync<$remote_messages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                                            ReactiveMessagingSender<CONFIG, $local_messages, ChannelUniMoveFullSync<$local_messages, SENDER_BUFFER, 1>> >
                                                         ::new($interface_ip.to_string(), $port);
                    server.spawn_unresponsive_processor($connection_events_handle_fn, $dialog_processor_builder_fn).await
                        .map_err(|err| format!("UnresponsiveSocketServer: error starting server with configs «{:?}»: {:?}", $const_config, err))
                },
                Channels::Crossbeam => {
                    let server = UnresponsiveSocketServer::<CONFIG,
                                                            $remote_messages,
                                                            $local_messages,
                                                            UniMoveCrossbeam<$remote_messages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                                            ReactiveMessagingSender<CONFIG, $local_messages, ChannelUniMoveCrossbeam<$local_messages, SENDER_BUFFER, 1>> >
                                                         ::new($interface_ip.to_string(), $port);
                    server.spawn_unresponsive_processor($connection_events_handle_fn, $dialog_processor_builder_fn).await
                        .map_err(|err| format!("UnresponsiveSocketServer: error starting server with configs «{:?}»: {:?}", $const_config, err))
                },
            }
        }
    }
}
pub use new_unresponsive_socket_server;

#[macro_export]
macro_rules! unresponsive_socket_server_type {
    ($const_config:    expr) => {
        const CONFIG:                    usize = $const_config.into();
        match $const_config.channel {
            Channels::Atomic => {},
            Channels::FullSync => {},
}
pub use new_unresponsive_socket_server_type;


/// Defines a Socket Server whose `dialog_processor` output stream items won't be sent back to the clients.\
/// Users of this struct may prefer to use it through the facility macro [new_responsive_socket_server!()]
#[derive(Debug)]
pub struct UnresponsiveSocketServer<const CONFIG: usize,
                                          RemoteMessages:      ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug + 'static,
                                          LocalMessages:       ReactiveMessagingSerializer<LocalMessages>    + Send + Sync + PartialEq + Debug + 'static,
                                          ProcessorUniType:    GenericUni<ItemType=RemoteMessages>           + Send + Sync                     + 'static,
                                          RetryableSenderImpl: RetryableSender<LocalMessages=LocalMessages>  + Send + Sync                     + 'static> {
    /// The interface to listen to incoming connections
    interface_ip:                String,
    /// The port to listen to incoming connections
    port:                        u16,
    /// Signaler to stop the server
    server_shutdown_signaler:    Option<tokio::sync::oneshot::Sender<u32>>,
    /// Signaler to cause [wait_for_shutdown()] to return
    local_shutdown_receiver:     Option<tokio::sync::oneshot::Receiver<()>>,
    _phantom: PhantomData<(RemoteMessages,LocalMessages,ProcessorUniType,RetryableSenderImpl)>
}

impl<const CONFIG: usize,
     RemoteMessages:      ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:       ReactiveMessagingSerializer<LocalMessages>    + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:    GenericUni<ItemType=RemoteMessages>           + Send + Sync                     + 'static,
     RetryableSenderImpl: RetryableSender<LocalMessages=LocalMessages>  + Send + Sync                     + 'static>
UnresponsiveSocketServer<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, RetryableSenderImpl> {

    /// Creates a new server instance listening on TCP/IP:
    ///   `interface_ip`:         the interface's IP to listen to -- 0.0.0.0 will cause listening to all network interfaces
    ///   `port`:                 what port to listen to
    ///   `processor_builder_fn`: a function to instantiate a new processor `Stream` whenever a new connection arrives
    pub fn new<IntoString: Into<String>>
              (interface_ip: IntoString,
               port: u16)
              -> Self {
        Self {
            interface_ip: interface_ip.into(),
            port,
            server_shutdown_signaler: None,
            local_shutdown_receiver: None,
            _phantom: PhantomData,
        }
    }

}

//#[async_trait]
impl<const CONFIG: usize,
     RemoteMessages:       ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:        ReactiveMessagingSerializer<LocalMessages>    + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:     GenericUni<ItemType=RemoteMessages>           + Send + Sync                     + 'static,
     RetryableSenderImpl:  RetryableSender<LocalMessages=LocalMessages>  + Send + Sync                     + 'static>
//ReactiveUnresponsiveProcessor for
UnresponsiveSocketServer<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, RetryableSenderImpl> {

    async fn spawn_unresponsive_processor<OutputStreamItemsType:                                                                                                                                                                                                                  Send + Sync + Debug       + 'static,
                                          ServerStreamType:               Stream<Item=OutputStreamItemsType>                                                                                                                                                                    + Send + Sync               + 'static,
                                          ConnectionEventsCallbackFuture: Future<Output=()>                                                                                                                                                                                     + Send                      + 'static,
                                          ConnectionEventsCallback:       Fn(/*server_event: */ConnectionEvent<RetryableSenderImpl>)                                                                                                          -> ConnectionEventsCallbackFuture + Send + Sync               + 'static,
                                          ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<RetryableSenderImpl>>, /*client_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> ServerStreamType               + Send + Sync               + 'static>

                                         (mut self,
                                          connection_events_callback:  ConnectionEventsCallback,
                                          dialog_processor_builder_fn: ProcessorBuilderFn)

                                         -> Result<impl ReactiveProcessorController, Box<dyn Error + Sync + Send>> {

        let (server_shutdown_sender, server_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (local_shutdown_sender, local_shutdown_receiver) = tokio::sync::oneshot::channel::<()>();
        self.server_shutdown_signaler = Some(server_shutdown_sender);
        self.local_shutdown_receiver = Some(local_shutdown_receiver);
        let listening_interface = self.interface_ip.clone();
        let port = self.port;

        let connection_events_callback = upgrade_to_shutdown_tracking(local_shutdown_sender, connection_events_callback);

        let socket_connection_handler = SocketConnectionHandler::<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, RetryableSenderImpl>::new();
        socket_connection_handler.server_loop_for_unresponsive_text_protocol(listening_interface.clone(),
                                                                             port,
                                                                             server_shutdown_receiver,
                                                                             connection_events_callback,
                                                                             dialog_processor_builder_fn).await
            .map_err(|err| format!("Error starting UnresponsiveSocketServer @ {listening_interface}:{port}: {:?}", err))?;
        Ok(self)
    }
}

impl<const CONFIG:   usize,
     RemoteMessages:      ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:       ReactiveMessagingSerializer<LocalMessages>    + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:    GenericUni<ItemType=RemoteMessages>           + Send + Sync                     + 'static,
     RetryableSenderImpl: RetryableSender<LocalMessages=LocalMessages>  + Send + Sync                     + 'static>
ReactiveProcessorController for
UnresponsiveSocketServer<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, RetryableSenderImpl> {

    fn shutdown_waiter(&mut self) -> Box<dyn FnOnce() -> BoxFuture<'static, Result<(), Box<dyn std::error::Error + Send + Sync>>> > {
        let mut local_shutdown_receiver = self.local_shutdown_receiver.take();
        Box::new(move || Box::pin({
            async move {
                if let Some(local_shutdown_receiver) = local_shutdown_receiver.take() {
                    match local_shutdown_receiver.await {
                        Ok(()) => {
                            Ok(())
                        },
                        Err(err) => Err(Box::from(format!("UnresponsiveSocketServer::wait_for_shutdown(): It is no longer possible to tell when the server will be shutdown: `one_shot` signal error: {err}")))
                    }
                } else {
                    Err(Box::from("UnresponsiveSocketServer: \"wait for shutdown\" requested, but the service was not started (or a previous shutdown was commanded) at the moment `shutdown_waiter()` was called"))
                }
            }
        }))
    }

    fn shutdown(mut self: Box<Self>, timeout_ms: u32) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match self.server_shutdown_signaler.take() {
            Some(server_sender) => {
                warn!("UnresponsiveSocketServer: Shutdown asked & initiated for server @ {}:{} -- timeout: {timeout_ms}ms", self.interface_ip, self.port);
                if let Err(_sent_value) = server_sender.send(timeout_ms) {
                    Err(Box::from("UnresponsiveSocketServer BUG: couldn't send shutdown signal to the network loop. Program is, likely, hanged. Please, investigate and fix!"))
                } else {
                    Ok(())
                }
            }
            None => {
                Err(Box::from("UnresponsiveSocketServer: Shutdown requested, but the service was not started. Ignoring..."))
            }
        }
    }

}

impl<const CONFIG:   usize,
     RemoteMessages:      ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:       ReactiveMessagingSerializer<LocalMessages>    + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:    GenericUni<ItemType=RemoteMessages>           + Send + Sync                     + 'static,
     RetryableSenderImpl: RetryableSender<LocalMessages=LocalMessages>  + Send + Sync                     + 'static>
ReactiveUnresponsiveProcessorAssociatedTypes for
UnresponsiveSocketServer<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, RetryableSenderImpl> {
    type RemoteMessages      = RemoteMessages;
    type LocalMessages       = LocalMessages;
    type ProcessorUniType    = ProcessorUniType;
    type RetryableSenderImpl = RetryableSenderImpl;
    type ConnectionEventType = ConnectionEvent<RetryableSenderImpl>;
    type StreamItemType      = ProcessorUniType::DerivedItemType;
    type StreamType          = MessagingMutinyStream<ProcessorUniType>;
}


/// Unit tests the [unresponsive_socket_server](self) module
#[cfg(any(test,doc))]
mod tests {
    use super::*;
    use crate::{new_unresponsive_socket_client, ron_deserializer, ron_serializer};
    use std::borrow::Borrow;
    use std::future;
    use std::ops::Deref;
    use std::pin::Pin;
    use std::sync::atomic::AtomicU32;
    use std::sync::atomic::Ordering::Relaxed;
    use std::time::Duration;
    use serde::{Deserialize, Serialize};
    use futures::stream::StreamExt;
    use tokio::sync::Mutex;


    /// Test that our types can be compiled & instantiated & are ready for usage
    #[cfg_attr(not(doc),tokio::test)]
    async fn doc_usage() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

        // demonstrates how to build a server out of generic functions, that will work with any channel
        ///////////////////////////////////////////////////////////////////////////////////////////////
        let mut server = new_unresponsive_socket_server!(
            ConstConfig::default(),
            "127.0.0.1",
            8050,
            DummyResponsiveClientAndServerMessages,
            DummyResponsiveClientAndServerMessages,
            connection_events_handler,
            processor
        )?;
        async fn connection_events_handler<RetryableSenderImpl: RetryableSender + Send + Sync + 'static>
                                          (_event: ConnectionEvent<RetryableSenderImpl>) {
        }
        fn processor<RetryableSenderImpl: RetryableSender + Send + Sync + 'static,
                     StreamItemType:      Deref<Target=DummyResponsiveClientAndServerMessages>>
                    (client_addr:            String,
                     connected_port:         u16,
                     peer:                   Arc<Peer<RetryableSenderImpl>>,
                     client_messages_stream: impl Stream<Item=StreamItemType>)
                    -> impl Stream<Item=()> {
            client_messages_stream.map(|payload| ())
        }
        let shutdown_waiter = server.shutdown_waiter();
        server.shutdown(200)?;
        shutdown_waiter().await?;

        // demonstrates how to use it with closures -- also allowing for any channel in the configs
        ///////////////////////////////////////////////////////////////////////////////////////////
        let mut server = new_unresponsive_socket_server!(
            ConstConfig::default(),
            "127.0.0.1",
            8050,
            DummyResponsiveClientAndServerMessages,
            DummyResponsiveClientAndServerMessages,
            |_| future::ready(()),
            |_, _, _, client_messages_stream| client_messages_stream.map(|payload| DummyResponsiveClientAndServerMessages::FloodPing)
        )?;
        let shutdown_waiter = server.shutdown_waiter();
        server.shutdown(200)?;
        shutdown_waiter().await?;

        // demonstrates how to use the concrete type
        ////////////////////////////////////////////
        // notice there may be a discrepancy in the `ConstConfig` you provide and the actual concrete types
        // you also provide for `UniProcessor` and `SenderChannel` -- therefore, this usage is not recommended
        // (but it is here anyway since it may bring, theoretically, a infinitesimal performance benefit)
        const CONFIG: ConstConfig = ConstConfig {
            receiver_buffer:      2048,
            sender_buffer:        1024,
            channel:              Channels::FullSync,
            executor_instruments: reactive_mutiny::prelude::Instruments::LogsWithExpensiveMetrics,
            ..ConstConfig::default()
        };
        type ProcessorUniType = UniZeroCopyFullSync<DummyResponsiveClientAndServerMessages, {CONFIG.receiver_buffer as usize}, 1, {CONFIG.executor_instruments.into()}>;
        type SenderChannelType = ChannelUniMoveFullSync<DummyResponsiveClientAndServerMessages, {CONFIG.sender_buffer as usize}, 1>;
        let server = UnresponsiveSocketServer :: <{CONFIG.into()},
                                                                          DummyResponsiveClientAndServerMessages,
                                                                          DummyResponsiveClientAndServerMessages,
                                                                          ProcessorUniType,
                                                                          ReactiveMessagingSender<{CONFIG.into()}, DummyResponsiveClientAndServerMessages, SenderChannelType> >
                                                                      :: new("127.0.0.1", 8050);
        let mut server = server.spawn_unresponsive_processor(
            |_| future::ready(()),
            |_, _, _, client_messages_stream| client_messages_stream.map(|payload| DummyResponsiveClientAndServerMessages::FloodPing)
        ).await?;
        let shutdown_waiter = server.shutdown_waiter();
        server.shutdown(200)?;
        shutdown_waiter().await?;

        Ok(())
    }

    /// assures the shutdown process is able to:
    ///   1) communicate with all clients
    ///   2) wait for up to the given timeout for them to gracefully disconnect
    ///   3) forcibly disconnect, if needed
    ///   4) notify any waiter on the server (after all the above steps are done) within the given timeout
    #[cfg_attr(not(doc),tokio::test(flavor = "multi_thread"))]
    async fn shutdown_process() {
        const PORT: u16 = 8051;

        // the shutdown timeout, in milliseconds
        let expected_max_shutdown_duration_ms = 543;
        // the tollerance, in milliseconds -- a too small shutdown duration means the server didn't wait for the client's disconnection; too much (possibly eternal) means it didn't enforce the timeout
        let tollerance_ms = 20;

        // sensors
        let client_received_messages_count_ref1 = Arc::new(AtomicU32::new(0));
        let client_received_messages_count_ref2 = Arc::clone(&client_received_messages_count_ref1);
        let server_received_messages_count_ref1 = Arc::new(AtomicU32::new(0));
        let server_received_messages_count_ref2 = Arc::clone(&server_received_messages_count_ref1);

        // start the server -- the test logic is here
//let client_peer_ref1 = Arc::<Mutex<Option<ReactiveMessagingSender<CONFIG, DummyResponsiveClientAndServerMessages, ChannelUniMoveFullSync<DummyResponsiveClientAndServerMessages, SENDER_BUFFER_SIZE, 1>>>>>::new(Mutex::new(None));
//         let client_peer_ref1 = Arc::new(Mutex::new(None));
//         let client_peer_ref2 = Arc::clone(&client_peer_ref1);

        const CONFIG: ConstConfig = ConstConfig {
            channel: Channels::FullSync,
            ..ConstConfig::default()
        };
        type ProcessorUniType = UniZeroCopyFullSync<DummyResponsiveClientAndServerMessages, {CONFIG.receiver_buffer as usize}, 1, {CONFIG.executor_instruments.into()}>;
        type SenderChannelType = ChannelUniMoveFullSync<DummyResponsiveClientAndServerMessages, {CONFIG.sender_buffer as usize}, 1>;
        type SenderType = ReactiveMessagingSender<{CONFIG.into()}, DummyResponsiveClientAndServerMessages, SenderChannelType>;
        let server = UnresponsiveSocketServer :: <{CONFIG.into()},
                                                                          DummyResponsiveClientAndServerMessages,
                                                                          DummyResponsiveClientAndServerMessages,
                                                                          ProcessorUniType,
                                                                          ReactiveMessagingSender<{CONFIG.into()}, DummyResponsiveClientAndServerMessages, SenderChannelType> >
                                                                      :: new("127.0.0.1", 805);

        //let connection_events_callback = move |connection_event: ConnectionEvent<SenderType>| {
        async fn connection_events_callback(connection_event: ConnectionEvent<SenderType>) {
//                let client_peer = Arc::clone(&client_peer_ref1);
//                async move {
                    match connection_event {
                        ConnectionEvent::PeerConnected { peer } => {
                            // register the client -- which will initiate the server shutdown further down in this test
//                            client_peer.lock().await.replace(peer);
                        },
                        ConnectionEvent::PeerDisconnected { peer: _, stream_stats: _ } => (),
                        ConnectionEvent::ApplicationShutdown { timeout_ms } => {
                            // send a message to the client (the first message, actually... that will initiate a flood of back-and-forth messages)
                            // then try to close the connection (which would only be gracefully done once all messages were sent... which may never happen).
//                            let client_peer = client_peer.lock().await;
//                            let client_peer = client_peer.as_ref().expect("No client is connected");
                            // send the flood starting message
//                            let _ = client_peer.send_async(DummyResponsiveClientAndServerMessages::FloodPing).await;
//                            client_peer.flush_and_close(Duration::from_millis(timeout_ms as u64)).await;
                            // guarantees this operation will take slightly more than the timeout+tolerance to complete
                            tokio::time::sleep(Duration::from_millis((timeout_ms+/*tollerance_ms+*/20+10) as u64)).await;
                        }
                    }
//                }
            };

        //let dialog_processor_builder_fn = move |_, _, _, client_messages: MessagingMutinyStream<ProcessorUniType>| {
        fn dialog_processor_builder_fn(client_addr: String, connected_port: u16, peer: Arc<Peer<SenderType>>, client_messages: MessagingMutinyStream<ProcessorUniType>) -> impl Stream<Item=DummyResponsiveClientAndServerMessages> {
                //let server_received_messages_count = Arc::clone(&server_received_messages_count_ref1);
                client_messages.map(move |client_message| {
                    std::mem::forget(client_message);   // TODO 2023-07-15: investigate this reactive-mutiny related bug: it seems OgreUnique doesn't like the fact that this type doesn't need dropping? (no internal strings)... or is it a reactive-messaging bug?
//                    server_received_messages_count.fetch_add(1, Relaxed);
                    DummyResponsiveClientAndServerMessages::FloodPing
                })
            };
        let start_future = server.spawn_unresponsive_processor(
            connection_events_callback,
            dialog_processor_builder_fn
        );
        let start_future = fix_rust_type_inference(start_future);
        let start_future: Pin<Box<dyn Future<Output=Result<Box<dyn ReactiveProcessorController + Send>, Box<dyn Error + Sync + Send>>>>> = Box::pin(start_future);
        let mut server = start_future.await.expect("Starting the server");

        /// Workaround a silly Rust Compiler bug that, as it seems, is hard to fix (it has been biting rustaceans for ~1.5 years).
        /// See: https://github.com/rust-lang/rust/issues/96865
        ///      https://github.com/rust-lang/rust/issues/102211
        ///      and the zero cost workaround implemented here: https://play.rust-lang.org/?version=nightly&mode=debug&edition=2021&gist=554fb0f6c23beadeb4d239fcf5b7d433
        fn fix_rust_type_inference<'f, O>(fut: impl 'f + Send + Future<Output=O>) -> impl 'f + Send + Future<Output=O> { fut }

        // start a client that will engage in a flood ping with the server when provoked (never closing the connection)
        let _client = new_unresponsive_socket_client!(
            ConstConfig::default(),
            "127.0.0.1",
            8051,
            DummyResponsiveClientAndServerMessages,
            DummyResponsiveClientAndServerMessages,
            |_: ConnectionEvent<_>| async {},
            move |_, _, _, server_messages| {
                let client_received_messages_count = Arc::clone(&client_received_messages_count_ref1);
                server_messages.map(move |server_message| {
                    std::mem::forget(server_message);   // TODO 2023-07-15: investigate this reactive-mutiny related bug: it seems OgreUnique doesn't like the fact that this type doesn't need dropping? (no internal strings)... or is it a reactive-messaging bug?
                    client_received_messages_count.fetch_add(1, Relaxed);
                    DummyResponsiveClientAndServerMessages::FloodPing
                })
            }
        ).expect("Starting the client");

        // wait for the client to connect
        // while client_peer_ref2.lock().await.is_none() {
        //     tokio::time::sleep(Duration::from_millis(1)).await;
        // }
        // shutdown the server & wait until the shutdown process is complete
        let wait_for_server_shutdown = server.shutdown_waiter();
        server.shutdown(expected_max_shutdown_duration_ms)
            .expect("Signaling the server of the shutdown intention");
        let start = std::time::SystemTime::now();
        wait_for_server_shutdown().await
            .expect("Waiting for the server to live it's life and to complete the shutdown process");
        let elapsed_ms = start.elapsed().unwrap().as_millis();
        assert!(client_received_messages_count_ref2.load(Relaxed) > 1, "The client didn't receive any messages (no 'server is shutting down' notification)");
        assert!(server_received_messages_count_ref2.load(Relaxed) > 1, "The server didn't receive any messages (no 'gracefully disconnecting' after being notified that the server is shutting down)");
        assert!(elapsed_ms.abs_diff(expected_max_shutdown_duration_ms as u128) < tollerance_ms as u128,
                "The server shutdown (of a never compling client) didn't complete in a reasonable time, meaning the shutdown code is wrong. Timeout: {}ms; Tollerance: {}ms; Measured Time: {}ms",
                expected_max_shutdown_duration_ms, tollerance_ms, elapsed_ms);
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
    enum DummyResponsiveClientAndServerMessages {
        #[default]
        FloodPing,
    }

    impl Deref for DummyResponsiveClientAndServerMessages {
        type Target = DummyResponsiveClientAndServerMessages;
        fn deref(&self) -> &Self::Target {
            self
        }
    }

    impl ReactiveMessagingSerializer<DummyResponsiveClientAndServerMessages> for DummyResponsiveClientAndServerMessages {
        #[inline(always)]
        fn serialize(remote_message: &DummyResponsiveClientAndServerMessages, buffer: &mut Vec<u8>) {
            ron_serializer(remote_message, buffer)
                .expect("unresponsive_socket_server.rs unit tests: No errors should have happened here!")
        }
        #[inline(always)]
        fn processor_error_message(err: String) -> DummyResponsiveClientAndServerMessages {
            panic!("unresponsive_socket_server.rs unit tests: protocol error when none should have happened: {err}");
        }
    }
    impl ReactiveMessagingDeserializer<DummyResponsiveClientAndServerMessages> for DummyResponsiveClientAndServerMessages {
        #[inline(always)]
        fn deserialize(local_message: &[u8]) -> Result<DummyResponsiveClientAndServerMessages, Box<dyn std::error::Error + Sync + Send>> {
            ron_deserializer(local_message)
        }
    }

}