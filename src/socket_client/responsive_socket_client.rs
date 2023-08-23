//! Provides a Client whose processor's output `Stream` yields messages to be sent back to the server.\
//! See also [super::unresponsive_socket_client]
//!
//! In case some messages yields no messages, the [ResponsiveMessages<>] trait provides a "No Answer" option, that may be returned by the dialog processor.\
//! If more than one message needs to be sent for each input, consider using [Stream::flat_map()] or reverting back to the [super::unresponsive_socket_client].


use std::error::Error;
use std::fmt::Debug;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use async_trait::async_trait;
use futures::future::BoxFuture;
use futures::Stream;
use log::warn;
use reactive_mutiny::prelude::advanced::{ChannelUniMoveAtomic, ChannelUniMoveCrossbeam, ChannelUniMoveFullSync, UniMoveCrossbeam, UniZeroCopyAtomic, UniZeroCopyFullSync};
use reactive_mutiny::prelude::{FullDuplexUniChannel, GenericUni};
use crate::{ReactiveMessagingDeserializer, ReactiveMessagingSerializer};
use crate::config::{Channels, ConstConfig};
use crate::prelude::{ConnectionEvent, MessagingMutinyStream};
use crate::socket_connection::{Peer, SocketConnectionHandler, common::{RetryableSender, ReactiveMessagingSender}};
use crate::socket_client::common::upgrade_to_shutdown_and_connected_state_tracking;
use crate::types::{ReactiveResponsiveProcessorAssociatedTypes, ReactiveProcessorController, ReactiveResponsiveProcessor, ResponsiveMessages};


/// Instantiates & allocate resources for a [ResponsiveSocketClient], ready to be later started.\
/// A Responsive Socket Client is defined by its `dialog_processor_builder_fn()`, which will produce a `Stream` of messages
/// to be automatically sent to the server.\
/// See [new_unresponsive_socket_client!()] if your dialog model is too off of 1-to-1, regarding requests & answers.\
/// Params:
///   - `const_config: ConstConfig` -- the configurations for the server, enforcing const time optimizations;
///   - `ip: IntoString` -- the ip to connect to;
///   - `port: u16` -- the port to connect to;
///   - `RemoteMessages` -- the type of the messages produced by the server. See [ReactiveResponsiveProcessorAssociatedTypes] for the traits it should implement;
///   - `LocalMessage` -- the type of the messages produced by this client. See [ReactiveResponsiveProcessorAssociatedTypes] for the traits it should implement.
///   - `connection_events_handle_fn` -- the generic function to handle connected, disconnected and shutdown events (possibly to manage sessions). Sign it as:
///     ```nocompile
///      async fn connection_events_handler<SenderChannelType: FullDuplexUniChannel<ItemType=LocalMessages,
///                                                                                 DerivedItemType=LocalMessages> + Sync + Send + 'static>
///                                        (event: ConnectionEvent<SenderChannelType>)
///     ```
///   - `dialog_processor_builder_fn` -- the generic function that receives the `Stream` of server messages and returns the `Stream` of client messages to
///                                      be sent to the server -- called when the connection is established. Sign it as:
///     ```nocompile
///     fn processor<SenderChannelType: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Sync + Send + 'static,
///                  StreamItemType:    Deref<Target=RemoteMessages>>
///                 (client_addr:            String,
///                  connected_port:         u16,
///                  peer:                   Arc<Peer<SenderChannelType>>,
///                  client_messages_stream: impl Stream<Item=StreamItemType>)
///                 -> impl Stream<Item=LocalMessages>
///     ```
#[macro_export]
macro_rules! new_responsive_socket_client {
    ($const_config:    expr,
     $interface_ip:    expr,
     $port:            expr,
     $remote_messages: ty,
     $local_messages:  ty,
     $connection_events_handle_fn: expr,
     $dialog_processor_builder_fn: expr) => {
        {
            use crate::socket_client::responsive_socket_client::ResponsiveSocketClient;
            const CONFIG:                    u64   = $const_config.into();
            const PROCESSOR_BUFFER:          usize = $const_config.receiver_buffer as usize;
            const PROCESSOR_UNI_INSTRUMENTS: usize = $const_config.executor_instruments.into();
            const SENDER_BUFFER:             usize = $const_config.sender_buffer   as usize;
            match $const_config.channel {
                Channels::Atomic => {
                    let server = ResponsiveSocketClient::<CONFIG,
                                                          $remote_messages,
                                                          $local_messages,
                                                          UniZeroCopyAtomic<$remote_messages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                                          ReactiveMessagingSender<CONFIG, $local_messages, ChannelUniMoveAtomic<$local_messages, SENDER_BUFFER, 1>> >
                                                       ::new($interface_ip.to_string(), $port);
                    server.spawn_responsive_processor($connection_events_handle_fn, $dialog_processor_builder_fn).await
                        .map_err(|err| format!("ResponsiveSocketClient: error starting client with configs «{:?}»: {:?}", $const_config, err))
                },
                Channels::FullSync => {
                    let server = ResponsiveSocketClient::<CONFIG,
                                                          $remote_messages,
                                                          $local_messages,
                                                          UniZeroCopyFullSync<$remote_messages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                                          ReactiveMessagingSender<CONFIG, $local_messages, ChannelUniMoveFullSync<$local_messages, SENDER_BUFFER, 1>> >
                                                       ::new($interface_ip.to_string(), $port);
                    server.spawn_responsive_processor($connection_events_handle_fn, $dialog_processor_builder_fn).await
                        .map_err(|err| format!("ResponsiveSocketClient: error starting client with configs «{:?}»: {:?}", $const_config, err))
                },
                Channels::Crossbeam => {
                    let server = ResponsiveSocketClient::<CONFIG,
                                                          $remote_messages,
                                                          $local_messages,
                                                          UniMoveCrossbeam<$remote_messages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                                          ReactiveMessagingSender<CONFIG, $local_messages, ChannelUniMoveCrossbeam<$local_messages, SENDER_BUFFER, 1>> >
                                                       ::new($interface_ip.to_string(), $port);
                    server.spawn_responsive_processor($connection_events_handle_fn, $dialog_processor_builder_fn).await
                        .map_err(|err| format!("ResponsiveSocketClient: error starting client with configs «{:?}»: {:?}", $const_config, err))
                },
            }
        }
    }
}
pub use new_responsive_socket_client;


/// Defines a Socket Client whose `dialog_processor` output streams will produce messages to be sent back to the server.\
/// Users of this struct may prefer to use it through the facility macro [new_responsive_socket_client!()]
#[derive(Debug)]
pub struct ResponsiveSocketClient<const CONFIG:        u64,
                                  RemoteMessages:      ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug + 'static,
                                  LocalMessages:       ReactiveMessagingSerializer<LocalMessages>    +
                                                       ResponsiveMessages<LocalMessages>             + Send + Sync + PartialEq + Debug + 'static,
                                  ProcessorUniType:    GenericUni<ItemType=RemoteMessages>           + Send + Sync                     + 'static,
                                  RetryableSenderImpl: RetryableSender<LocalMessages=LocalMessages>  + Send + Sync                     + 'static> {
    /// false if a disconnection happened, as tracked by the socket logic
    connected: Arc<AtomicBool>,
    /// the server ip to connect to
    ip:        String,
    /// the server port to connect to
    port:      u16,
    /// Signaler to stop the client
    client_shutdown_signaler: Option<tokio::sync::oneshot::Sender<u32>>,
    /// Signaler to cause [wait_for_shutdown()] to return
    local_shutdown_receiver:     Option<tokio::sync::oneshot::Receiver<()>>,
    _phantom: PhantomData<(RemoteMessages,LocalMessages,ProcessorUniType,RetryableSenderImpl)>
}

impl<const CONFIG:        u64,
     RemoteMessages:      ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:       ReactiveMessagingSerializer<LocalMessages>    +
                          ResponsiveMessages<LocalMessages>             + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:    GenericUni<ItemType=RemoteMessages>           + Send + Sync                     + 'static,
     RetryableSenderImpl: RetryableSender<LocalMessages=LocalMessages>  + Send + Sync                     + 'static>
ResponsiveSocketClient<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, RetryableSenderImpl> {

    /// Instantiates a client to connect to a TCP/IP Server:
    ///   `ip`:                   the server IP to connect to
    ///   `port`:                 the server port to connect to
    pub fn new<IntoString: Into<String>>
              (ip:   IntoString,
               port: u16)
              -> Self {
        Self {
            connected: Arc::new(AtomicBool::new(false)),
            ip: ip.into(),
            port,
            client_shutdown_signaler: None,
            local_shutdown_receiver: None,
            _phantom: PhantomData,
        }
    }
}

#[async_trait]
impl<const CONFIG:        u64,
     RemoteMessages:      ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:       ReactiveMessagingSerializer<LocalMessages>    +
                          ResponsiveMessages<LocalMessages>             + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:    GenericUni<ItemType=RemoteMessages>           + Send + Sync                     + 'static,
     RetryableSenderImpl: RetryableSender<LocalMessages=LocalMessages>  + Send + Sync                     + 'static>
ReactiveResponsiveProcessor for
ResponsiveSocketClient<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, RetryableSenderImpl> {

    async fn spawn_responsive_processor<ServerStreamType:                Stream<Item=LocalMessages>                                                                                                                                                                                        + Send        + 'static,
                                        ConnectionEventsCallbackFuture:  Future<Output=()>                                                                                                                                                                                                 + Send,
                                        ConnectionEventsCallback:        Fn(/*server_event: */ConnectionEvent<Self::RetryableSenderImpl>)                                                                                                                -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                        ProcessorBuilderFn:              Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<Self::RetryableSenderImpl>>, /*client_messages_stream: */MessagingMutinyStream<Self::ProcessorUniType>) -> ServerStreamType               + Send + Sync + 'static>

                                       (mut self,
                                        connection_events_callback:  ConnectionEventsCallback,
                                        dialog_processor_builder_fn: ProcessorBuilderFn)

                                       -> Result<Box<dyn ReactiveProcessorController>, Box<dyn std::error::Error + Sync + Send>> {

        let (server_shutdown_sender, server_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (local_shutdown_sender, local_shutdown_receiver) = tokio::sync::oneshot::channel::<()>();
        self.client_shutdown_signaler = Some(server_shutdown_sender);
        self.local_shutdown_receiver = Some(local_shutdown_receiver);
        let ip = self.ip.clone();
        let port = self.port;

        let connection_events_callback = upgrade_to_shutdown_and_connected_state_tracking(&self.connected, local_shutdown_sender, connection_events_callback);

        let socket_connection_handler = SocketConnectionHandler::<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, RetryableSenderImpl>::new();
        socket_connection_handler.client_for_responsive_text_protocol
            (ip.clone(),
             port,
             server_shutdown_receiver,
             connection_events_callback,
             dialog_processor_builder_fn).await
            .map_err(|err| format!("Error starting ResponsiveSocketClient @ {ip}:{port}: {:?}", err))?;
        Ok(Box::new(self))
    }
}

impl<const CONFIG:        u64,
     RemoteMessages:      ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:       ReactiveMessagingSerializer<LocalMessages>    +
                          ResponsiveMessages<LocalMessages>             + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:    GenericUni<ItemType=RemoteMessages>           + Send + Sync + 'static,
     RetryableSenderImpl: RetryableSender<LocalMessages=LocalMessages>  + Send + Sync + 'static>
ReactiveProcessorController for
ResponsiveSocketClient<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, RetryableSenderImpl> {

    fn shutdown_waiter(&mut self) -> Box<dyn FnOnce() -> BoxFuture<'static, Result<(), Box<dyn std::error::Error + Send + Sync>>> > {
        let mut local_shutdown_receiver = self.local_shutdown_receiver.take();
        Box::new(move || Box::pin({
            async move {
                if let Some(local_shutdown_receiver) = local_shutdown_receiver.take() {
                    match local_shutdown_receiver.await {
                        Ok(()) => {
                            Ok(())
                        },
                        Err(err) => Err(Box::from(format!("ResponsiveSocketClient::wait_for_shutdown(): It is no longer possible to tell when the server will be shutdown: `one_shot` signal error: {err}")))
                    }
                } else {
                    Err(Box::from("ResponsiveSocketClient: \"wait for shutdown\" requested, but the service was not started (or a previous shutdown was commanded) at the moment `shutdown_waiter()` was called"))
                }
            }
        }))
    }

    fn shutdown(mut self: Box<Self>, timeout_ms: u32) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match self.client_shutdown_signaler.take() {
            Some(server_sender) => {
                warn!("ResponsiveSocketClient: Shutdown asked & initiated for server @ {}:{} -- timeout: {timeout_ms}ms", self.ip, self.port);
                if let Err(_sent_value) = server_sender.send(timeout_ms) {
                    Err(Box::from("ResponsiveSocketClient BUG: couldn't send shutdown signal to the network loop. Program is, likely, hanged. Please, investigate and fix!"))
                } else {
                    Ok(())
                }
            }
            None => {
                Err(Box::from("ResponsiveSocketClient: Shutdown requested, but the service was not started. Ignoring..."))
            }
        }
    }

}


impl<const CONFIG:        u64,
     RemoteMessages:      ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:       ReactiveMessagingSerializer<LocalMessages>    +
                          ResponsiveMessages<LocalMessages>             + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:    GenericUni<ItemType=RemoteMessages>           + Send + Sync                     + 'static,
     RetryableSenderImpl: RetryableSender<LocalMessages=LocalMessages>  + Send + Sync                     + 'static>
ReactiveResponsiveProcessorAssociatedTypes for
ResponsiveSocketClient<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, RetryableSenderImpl> {
    type RemoteMessages      = RemoteMessages;
    type LocalMessages       = LocalMessages;
    type ProcessorUniType    = ProcessorUniType;
    type RetryableSenderImpl = RetryableSenderImpl;
    type ConnectionEventType = ConnectionEvent<RetryableSenderImpl>;
    type StreamItemType      = ProcessorUniType::DerivedItemType;
    type StreamType          = MessagingMutinyStream<ProcessorUniType>;
}


/// Unit tests the [responsive_socket_server](self) module
#[cfg(any(test,doc))]
mod tests {
    use super::*;
    use crate::{ron_deserializer, ron_serializer};
    use std::borrow::Borrow;
    use std::future;
    use std::ops::Deref;
    use std::time::Duration;
    use serde::{Deserialize, Serialize};
    use futures::stream::StreamExt;


    /// Test that our types can be compiled & instantiated & are ready for usage
    #[cfg_attr(not(doc),tokio::test)]
    async fn doc_usage() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

        // demonstrates how to build a client out of generic functions, that will work with any channel
        ///////////////////////////////////////////////////////////////////////////////////////////////
        let mut client = new_responsive_socket_client!(
            ConstConfig::default(),
            "66.45.249.218",
            443,
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
                    -> impl Stream<Item=DummyResponsiveClientAndServerMessages> {
            client_messages_stream.map(|payload| DummyResponsiveClientAndServerMessages::FloodPing)
        }
        let shutdown_waiter = client.shutdown_waiter();
        client.shutdown(200)?;
        shutdown_waiter().await?;

        // demonstrates how to use it with closures -- also allowing for any channel in the configs
        ///////////////////////////////////////////////////////////////////////////////////////////
        let mut client = new_responsive_socket_client!(
            ConstConfig::default(),
            "66.45.249.218",
            443,
            DummyResponsiveClientAndServerMessages,
            DummyResponsiveClientAndServerMessages,
            |_| future::ready(()),
            |_, _, _, client_messages_stream| client_messages_stream.map(|payload| DummyResponsiveClientAndServerMessages::FloodPing)
        )?;
        let shutdown_waiter = client.shutdown_waiter();
        client.shutdown(200)?;
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
        let client = ResponsiveSocketClient :: <{CONFIG.into()},
                                                                       DummyResponsiveClientAndServerMessages,
                                                                       DummyResponsiveClientAndServerMessages,
                                                                       ProcessorUniType,
                                                                       ReactiveMessagingSender<{CONFIG.into()}, DummyResponsiveClientAndServerMessages, SenderChannelType> >
                                                                   :: new("66.45.249.218", 443);
        let mut client = client.spawn_responsive_processor(
            |_| future::ready(()),
            |_, _, _, client_messages_stream| client_messages_stream.map(|payload| DummyResponsiveClientAndServerMessages::FloodPing)
        ).await?;
        let shutdown_waiter = client.shutdown_waiter();
        client.shutdown(200)?;
        shutdown_waiter().await?;

        Ok(())
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
                .expect("responsive_socket_client.rs unit tests: No errors should have happened here!")
        }
        #[inline(always)]
        fn processor_error_message(err: String) -> DummyResponsiveClientAndServerMessages {
            panic!("responsive_socket_client.rs unit tests: protocol error when none should have happened: {err}");
        }
    }
    impl ResponsiveMessages<DummyResponsiveClientAndServerMessages> for DummyResponsiveClientAndServerMessages {
        #[inline(always)]
        fn is_disconnect_message(_processor_answer: &DummyResponsiveClientAndServerMessages) -> bool {
            false
        }
        #[inline(always)]
        fn is_no_answer_message(_processor_answer: &DummyResponsiveClientAndServerMessages) -> bool {
            false
        }
    }
    impl ReactiveMessagingDeserializer<DummyResponsiveClientAndServerMessages> for DummyResponsiveClientAndServerMessages {
        #[inline(always)]
        fn deserialize(local_message: &[u8]) -> Result<DummyResponsiveClientAndServerMessages, Box<dyn std::error::Error + Sync + Send>> {
            ron_deserializer(local_message)
        }
    }

}