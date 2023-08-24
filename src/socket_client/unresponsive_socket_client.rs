//! Provides a client whose processor's output `Stream` won't be sent to the server.\
//! See also [super::responsive_socket_client].
//!
//! The implementations here still allow you to send messages to the server and should be preferred (as far as performance is concerned)
//! if the messages in don't nearly map 1/1 with the messages out.


use crate::{ReactiveMessagingDeserializer, ReactiveMessagingSerializer};
use std::fmt::Debug;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;
use std::error::Error;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::time::SystemTime;
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
use tokio::sync::oneshot::error::TryRecvError;
use crate::config::{Channels, ConstConfig};
use crate::prelude::{ConnectionEvent, MessagingMutinyStream};
use crate::socket_connection::{Peer, SocketConnectionHandler, common::ReactiveMessagingSender};
use crate::types::{ReactiveProcessorController, ReactiveSocketClient, ReactiveUnresponsiveProcessorAssociatedTypes};


/// Instantiates & allocate resources for a [UnresponsiveSocketClient], ready to be later started.\
/// A Unresponsive Socket Client is defined by its `dialog_processor_builder_fn()`, which won't produce a `Stream` of messages
/// to be sent to the server. To send messages, a call to [Peer::send()] must be done.\
/// See [new_responsive_socket_client!()] if your dialog model is near 1-to-1 regarding requests & answers.\
/// Params:
///   - `const_config: ConstConfig` -- the configurations for the client, enforcing const time optimizations;
///   - `ip: IntoString` -- the ip to connect to;
///   - `port: u16` -- the port to connect to;
///   - `RemoteMessages` -- the type of the messages produced by the server. See [ReactiveUnresponsiveProcessorAssociatedTypes] for the traits it should implement;
///   - `LocalMessage` -- the type of the messages produced by this client. See [ReactiveUnresponsiveProcessorAssociatedTypes] for the traits it should implement.
///   - `connection_events_handle_fn` -- the generic function to handle connected, disconnected and shutdown events. Sign it as:
///     ```nocompile
///      async fn connection_events_handler<SenderChannelType: FullDuplexUniChannel<ItemType=LocalMessages,
///                                                                                 DerivedItemType=LocalMessages> + Sync + Send + 'static>
///                                        (event: ConnectionEvent<SenderChannelType>)
///     ```
///   - `dialog_processor_builder_fn` -- the generic function that receives the `Stream` of server messages and returns another `Stream` of unspecified items
///                                       -- called when the connection is established. Notice sending messages to the server should be done through the `peer` parameter.
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
macro_rules! new_unresponsive_socket_client {
    ($const_config:    expr,
     $interface_ip:    expr,
     $port:            expr,
     $remote_messages: ty,
     $local_messages:  ty,
     $connection_events_handle_fn: expr,
     $dialog_processor_builder_fn: expr) => {
        {
            use crate::socket_client::unresponsive_socket_client::UnresponsiveSocketClient;
            const CONFIG:                    u64   = $const_config.into();
            const PROCESSOR_BUFFER:          usize = $const_config.receiver_buffer as usize;
            const PROCESSOR_UNI_INSTRUMENTS: usize = $const_config.executor_instruments.into();
            const SENDER_BUFFER:             usize = $const_config.sender_buffer   as usize;
            match $const_config.channel {
                Channels::Atomic => {
                    let server = UnresponsiveSocketClient::<CONFIG,
                                                            $remote_messages,
                                                            $local_messages,
                                                            UniZeroCopyAtomic<$remote_messages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                                            ChannelUniMoveAtomic<$local_messages, SENDER_BUFFER, 1> >
                                                         ::new($interface_ip.to_string(), $port);
                    server.spawn_unresponsive_processor($connection_events_handle_fn, $dialog_processor_builder_fn).await
                        .map_err(|err| format!("UnresponsiveSocketClient: error starting client with configs «{:?}»: {:?}", $const_config, err))
                },
                Channels::FullSync => {
                    let server = UnresponsiveSocketClient::<CONFIG,
                                                            $remote_messages,
                                                            $local_messages,
                                                            UniZeroCopyFullSync<$remote_messages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                                            ChannelUniMoveFullSync<$local_messages, SENDER_BUFFER, 1> >
                                                         ::new($interface_ip.to_string(), $port);
                    server.spawn_unresponsive_processor($connection_events_handle_fn, $dialog_processor_builder_fn).await
                        .map_err(|err| format!("UnresponsiveSocketClient: error starting client with configs «{:?}»: {:?}", $const_config, err))
                },
                Channels::Crossbeam => {
                    let server = UnresponsiveSocketClient::<CONFIG,
                                                            $remote_messages,
                                                            $local_messages,
                                                            UniMoveCrossbeam<$remote_messages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                                            ChannelUniMoveCrossbeam<$local_messages, SENDER_BUFFER, 1> >
                                                         ::new($interface_ip.to_string(), $port);
                    server.spawn_unresponsive_processor($connection_events_handle_fn, $dialog_processor_builder_fn).await
                        .map_err(|err| format!("UnresponsiveSocketClient: error starting client with configs «{:?}»: {:?}", $const_config, err))
                },
            }
        }
    }
}
pub use new_unresponsive_socket_client;
use crate::socket_client::common::upgrade_to_shutdown_and_connected_state_tracking;
use crate::socket_connection::common::RetryableSender;


/// Defines a Socket Client whose `dialog_processor` output stream items won't be sent back to the server.\
/// Users of this struct may prefer to use it through the facility macro [new_responsive_socket_client!()]
#[derive(Debug)]
pub struct UnresponsiveSocketClient<const CONFIG:           u64,
                                          RemoteMessages:   ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
                                          LocalMessages:    ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
                                          ProcessorUniType: GenericUni<ItemType=RemoteMessages>                                         + Send + Sync                     + 'static,
                                          SenderChannel:    FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync> {
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
    _phantom: PhantomData<(RemoteMessages,LocalMessages,ProcessorUniType,SenderChannel)>
}

impl<const CONFIG:     u64,
     RemoteMessages:   ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:    ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType: GenericUni<ItemType=RemoteMessages>                                         + Send + Sync                     + 'static,
     SenderChannel:    FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync>
UnresponsiveSocketClient<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannel> {

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

//#[async_trait]
impl<const CONFIG:     u64,
     RemoteMessages:   ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:    ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType: GenericUni<ItemType=RemoteMessages>                                         + Send + Sync                     + 'static,
     SenderChannel:    FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync>
//ReactiveUnresponsiveProcessor for
UnresponsiveSocketClient<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannel> {

    pub async fn spawn_unresponsive_processor<OutputStreamItemsType:                                                                                                                                                                                                                                   Send + Sync + Debug + 'static,
                                          ServerStreamType:               Stream<Item=OutputStreamItemsType>                                                                                                                                                                                     + Send                + 'static,
                                          ConnectionEventsCallbackFuture: Future<Output=()>                                                                                                                                                                                                      + Send,
                                          ConnectionEventsCallback:       Fn(/*server_event: */ConnectionEvent<CONFIG, LocalMessages, SenderChannel>)                                                                                                          -> ConnectionEventsCallbackFuture + Send + Sync         + 'static,
                                          ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<CONFIG, LocalMessages, SenderChannel>>, /*client_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> ServerStreamType               + Send + Sync         + 'static>

                                         (mut self,
                                          connection_events_callback: ConnectionEventsCallback,
                                          dialog_processor_builder_fn: ProcessorBuilderFn)

                                         -> Result<Box<dyn ReactiveProcessorController + Send>, Box<dyn Error + Sync + Send>> {

        let (client_shutdown_sender, client_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (local_shutdown_sender, local_shutdown_receiver) = tokio::sync::oneshot::channel::<()>();
        self.client_shutdown_signaler = Some(client_shutdown_sender);
        self.local_shutdown_receiver = Some(local_shutdown_receiver);
        let ip = self.ip.clone();
        let port = self.port;

        let connection_events_callback = upgrade_to_shutdown_and_connected_state_tracking(&self.connected, local_shutdown_sender, connection_events_callback);

        let socket_connection_handler = SocketConnectionHandler::<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannel>::new();
        socket_connection_handler.client_for_unresponsive_text_protocol(ip.clone(),
                                                                        port,
                                                                        client_shutdown_receiver,
                                                                        connection_events_callback,
                                                                        dialog_processor_builder_fn).await
            .map_err(|err| format!("Error starting UnresponsiveSocketClient @ {ip}:{port}: {:?}", err))?;
        Ok(Box::new(self))
    }
}

impl<const CONFIG:     u64,
     RemoteMessages:   ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:    ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType: GenericUni<ItemType=RemoteMessages>                                         + Send + Sync                     + 'static,
     SenderChannel:    FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync>
ReactiveProcessorController for
UnresponsiveSocketClient<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannel> {

    fn shutdown_waiter(&mut self) -> Box<dyn FnOnce() -> BoxFuture<'static, Result<(), Box<dyn std::error::Error + Send + Sync>>> > {
        let mut local_shutdown_receiver = self.local_shutdown_receiver.take();
        Box::new(move || Box::pin({
            async move {
                if let Some(mut local_shutdown_receiver) = local_shutdown_receiver.take() {
                    match local_shutdown_receiver.await {
                        Ok(()) => {
                            Ok(())
                        },
                        Err(err) => Err(Box::from(format!("UnresponsiveSocketClient::wait_for_shutdown(): It is no longer possible to tell when the client will be shutdown: `one_shot` signal error: {err}")))
                    }
                } else {
                    Err(Box::from("UnresponsiveSocketClient: \"wait for shutdown\" requested, but the service was not started (or a previous shutdown was commanded) at the moment `shutdown_waiter()` was called"))
                }
            }
        }))
    }

    fn shutdown(mut self: Box<Self>, timeout_ms: u32) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match self.client_shutdown_signaler.take() {
            Some(server_sender) => {
                warn!("UnresponsiveSocketClient: Shutdown asked & initiated for client connecting @ {}:{} -- timeout: {timeout_ms}ms", self.ip, self.port);
                if let Err(_sent_value) = server_sender.send(timeout_ms) {
                    Err(Box::from("UnresponsiveSocketClient BUG: couldn't send shutdown signal to the network loop. Program is, likely, hanged. Please, investigate and fix!"))
                } else {
                    Ok(())
                }
            }
            None => {
                Err(Box::from("UnresponsiveSocketClient: Shutdown requested, but the service was not started. Ignoring..."))
            }
        }
    }

}

impl<const CONFIG:     u64,
     RemoteMessages:   ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:    ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType: GenericUni<ItemType=RemoteMessages>                                         + Send + Sync                     + 'static,
     SenderChannel:    FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync>
ReactiveSocketClient for
UnresponsiveSocketClient<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannel> {

    fn is_connected(&self) -> bool {
        self.connected.load(Relaxed)
    }
}


// impl<const CONFIG:        u64,
//      RemoteMessages:      ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug + 'static,
//      LocalMessages:       ReactiveMessagingSerializer<LocalMessages>    + Send + Sync + PartialEq + Debug + 'static,
//      ProcessorUniType:    GenericUni<ItemType=RemoteMessages>           + Send + Sync                     + 'static,
//      RetryableSenderImpl: RetryableSender<LocalMessages=LocalMessages>  + Send + Sync                     + 'static>
// ReactiveUnresponsiveProcessorAssociatedTypes for
// UnresponsiveSocketClient<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, RetryableSenderImpl> {
//     type RemoteMessages      = RemoteMessages;
//     type LocalMessages       = LocalMessages;
//     type ProcessorUniType    = ProcessorUniType;
//     type RetryableSenderImpl = RetryableSenderImpl;
//     type ConnectionEventType = ConnectionEvent<RetryableSenderImpl>;
//     type StreamItemType      = ProcessorUniType::DerivedItemType;
//     type StreamType          = MessagingMutinyStream<ProcessorUniType>;
// }


/// Unit tests the [unresponsive_socket_client](self) module
#[cfg(any(test,doc))]
mod tests {
    use super::*;
    use crate::{ron_deserializer, ron_serializer};
    use std::borrow::Borrow;
    use std::future;
    use std::ops::Deref;
    use serde::{Deserialize, Serialize};
    use futures::stream::StreamExt;


    /// Test that our types can be compiled & instantiated & are ready for usage
    #[cfg_attr(not(doc),tokio::test)]
    async fn doc_usage() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

        // demonstrates how to build a client out of generic functions, that will work with any channel
        ///////////////////////////////////////////////////////////////////////////////////////////////
        let mut client = new_unresponsive_socket_client!(
            ConstConfig::default(),
            "66.45.249.218",
            443,
            DummyResponsiveClientAndServerMessages,
            DummyResponsiveClientAndServerMessages,
            connection_events_handler,
            processor
        )?;
        async fn connection_events_handler<const CONFIG:  u64,
                                           LocalMessages: ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
                                           SenderChannel: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync>
                                          (_event: ConnectionEvent<CONFIG, LocalMessages, SenderChannel>) {
        }
        fn processor<const CONFIG:   u64,
                     LocalMessages:  ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
                     SenderChannel:  FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync,
                     StreamItemType: Deref<Target=DummyResponsiveClientAndServerMessages>>
                    (client_addr:            String,
                     connected_port:         u16,
                     peer:                   Arc<Peer<CONFIG, LocalMessages, SenderChannel>>,
                     client_messages_stream: impl Stream<Item=StreamItemType>)
                    -> impl Stream<Item=()> {
            client_messages_stream.map(|payload| ())
        }
        let shutdown_waiter = client.shutdown_waiter();
        client.shutdown(200)?;
        shutdown_waiter().await?;

        // demonstrates how to use it with closures -- also allowing for any channel in the configs
        ///////////////////////////////////////////////////////////////////////////////////////////
        let mut client = new_unresponsive_socket_client!(
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
        let client = UnresponsiveSocketClient :: <{CONFIG.into()},
                                                                          DummyResponsiveClientAndServerMessages,
                                                                          DummyResponsiveClientAndServerMessages,
                                                                          ProcessorUniType,
                                                                          SenderChannelType>
                                                                      :: new("66.45.249.218",443);
        let mut client = client.spawn_unresponsive_processor(
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
                .expect("unresponsive_socket_client.rs unit tests: No errors should have happened here!")
        }
        #[inline(always)]
        fn processor_error_message(err: String) -> DummyResponsiveClientAndServerMessages {
            panic!("unresponsive_socket_client.rs unit tests: protocol error when none should have happened: {err}");
        }
    }
    impl ReactiveMessagingDeserializer<DummyResponsiveClientAndServerMessages> for DummyResponsiveClientAndServerMessages {
        #[inline(always)]
        fn deserialize(local_message: &[u8]) -> Result<DummyResponsiveClientAndServerMessages, Box<dyn std::error::Error + Sync + Send>> {
            ron_deserializer(local_message)
        }
    }

}