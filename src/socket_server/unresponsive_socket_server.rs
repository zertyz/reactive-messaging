//! Structs that creates a server whose processor's output `Stream` won't be sent to the clients.\
//! See also [super::responsive_socket_server].
//!
//! The implementations here still allow you to send messages to the client and should be preferred (as far as performance is concerned)
//! if the messages in don't nearly map 1/1 with the messages out.
//!
//! TODO: The retrying mechanism should should be moved to the `peer.sender`


use crate::{ReactiveMessagingDeserializer, ReactiveMessagingSerializer, ResponsiveMessages};
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
use crate::socket_connection_handler::{Peer, SocketConnectionHandler};
use crate::socket_server::common::upgrade_to_shutdown_tracking;
use crate::socket_server::socket_server_types::{ServerUnresponsiveProcessor, SocketServerController, UnresponsiveSocketServerAssociatedTypes, UnresponsiveSocketServerGenericTypes};


/// This is the recommended one
pub struct UnresponsiveSocketServer<const CONFIG:                    usize,
                                    const PROCESSOR_BUFFER:          usize,
                                    const PROCESSOR_UNI_INSTRUMENTS: usize,
                                    const SENDER_BUFFER:             usize,
                                    RemoteMessages: ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug + 'static,
                                    LocalMessages:  ReactiveMessagingSerializer<LocalMessages>    + Send + Sync + PartialEq + Debug + 'static> {

    /// The interface to listen to incoming connections
    interface_ip:                String,
    /// The port to listen to incoming connections
    port:                        u16,
    _phanrom: PhantomData<(RemoteMessages, LocalMessages)>,
}

#[async_trait]
impl<const CONFIG:                    usize,
     const PROCESSOR_BUFFER:          usize,
     const PROCESSOR_UNI_INSTRUMENTS: usize,
     const SENDER_BUFFER:             usize,
     RemoteMessages: ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:  ReactiveMessagingSerializer<LocalMessages>    + Send + Sync + PartialEq + Debug + 'static>
ServerUnresponsiveProcessor for
UnresponsiveSocketServer<CONFIG, PROCESSOR_BUFFER, PROCESSOR_UNI_INSTRUMENTS, SENDER_BUFFER, RemoteMessages, LocalMessages> {

    const CONST_CONFIG: ConstConfig = ConstConfig::from(CONFIG);

    async fn spawn_unresponsive_processor<OutputStreamItemsType:                                                                                                                                                                                                                Send + Sync + Debug + 'static,
                                          ServerStreamType:               Stream<Item=OutputStreamItemsType>                                                                                                                                                                  + Send                + 'static,
                                          ConnectionEventsCallbackFuture: Future<Output=()>                                                                                                                                                                                   + Send,
                                          ProcessorUniType:               GenericUni<ItemType=Self::RemoteMessages>                                                                                                                                                           + Send + Sync         + 'static,
                                          SenderChannelType:              FullDuplexUniChannel<ItemType=Self::LocalMessages, DerivedItemType=Self::LocalMessages>                                                                                                             + Sync + Send         + 'static,
                                          ConnectionEventsCallback:       Fn(/*server_event: */ConnectionEvent<SenderChannelType>)                                                                                                          -> ConnectionEventsCallbackFuture + Send + Sync         + 'static,
                                          ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<SenderChannelType>>, /*client_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> ServerStreamType               + Send + Sync         + 'static>

                                           (mut self,
                                            connection_events_callback:  ConnectionEventsCallback,
                                            dialog_processor_builder_fn: ProcessorBuilderFn)

                                           -> Result<Arc<dyn SocketServerController>, Box<dyn std::error::Error + Sync + Send + 'static>> {

        match Self::CONST_CONFIG.channel {
            Channels::Atomic => {
        //         let atomic_movable_channel = ChannelUniMoveAtomic::<LocalMessages, SENDER_BUFFER, 1>::new(self.interface_ip.clone());
        //         fn is_full_duplex<T: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages>,
        //                           LocalMessages>(full_duplex_impl: T) {}
        //         is_full_duplex(atomic_movable_channel);
                let server = CustomUnresponsiveSocketServer::<CONFIG,
                                                                                           RemoteMessages,
                                                                                           LocalMessages,
                                                                                           UniZeroCopyAtomic<RemoteMessages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                                                                           ChannelUniMoveAtomic<LocalMessages, SENDER_BUFFER, 1> > :: new(self.interface_ip, self.port);
                server.spawn_unresponsive_processor::<OutputStreamItemsType,
                                                      ServerStreamType,
                                                      ConnectionEventsCallbackFuture,
                                                      ProcessorUniType,
                                                      SenderChannelType,
                                                      ConnectionEventsCallback,
                                                      ProcessorBuilderFn>
                                                     (connection_events_callback,
                                                      dialog_processor_builder_fn)
                    .await
            },
            Channels::FullSync => {
                let server = CustomUnresponsiveSocketServer::<CONFIG,
                                                                                           RemoteMessages,
                                                                                           LocalMessages,
                                                                                           UniZeroCopyFullSync<RemoteMessages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                                                                           ChannelUniMoveFullSync<LocalMessages, SENDER_BUFFER, 1> > :: new(self.interface_ip, self.port);
                server.spawn_unresponsive_processor::<OutputStreamItemsType,
                                                      ServerStreamType,
                                                      ConnectionEventsCallbackFuture,
                                                      ProcessorUniType,
                                                      SenderChannelType,
                                                      ConnectionEventsCallback,
                                                      ProcessorBuilderFn>
                                                     (connection_events_callback,
                                                      dialog_processor_builder_fn)
                    .await
            },
            Channels::Crossbean => {
                let server = CustomUnresponsiveSocketServer::<CONFIG,
                                                                                           RemoteMessages,
                                                                                           LocalMessages,
                                                                                           UniMoveCrossbeam<RemoteMessages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                                                                           ChannelUniMoveCrossbeam<LocalMessages, SENDER_BUFFER, 1> > :: new(self.interface_ip, self.port);
                server.spawn_unresponsive_processor::<OutputStreamItemsType,
                                                      ServerStreamType,
                                                      ConnectionEventsCallbackFuture,
                                                      ProcessorUniType,
                                                      SenderChannelType,
                                                      ConnectionEventsCallback,
                                                      ProcessorBuilderFn>
                                                     (connection_events_callback,
                                                      dialog_processor_builder_fn)
                    .await
            },
        }
    }
}

impl<const CONFIG:                    usize,
     const PROCESSOR_BUFFER:          usize,
     const PROCESSOR_UNI_INSTRUMENTS: usize,
     const SENDER_BUFFER:             usize,
     RemoteMessages: ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:  ReactiveMessagingSerializer<LocalMessages>    + Send + Sync + PartialEq + Debug + 'static>
UnresponsiveSocketServerGenericTypes for
UnresponsiveSocketServer<CONFIG, PROCESSOR_BUFFER, PROCESSOR_UNI_INSTRUMENTS, SENDER_BUFFER, RemoteMessages, LocalMessages> {
    type RemoteMessages = RemoteMessages;
    type LocalMessages  = LocalMessages;
    type StreamItemType = ();
    type StreamType     = ();
}


/// This is the internal one. Users must use [UnresponsiveSocketServer] instead.
#[derive(Debug)]
pub struct CustomUnresponsiveSocketServer<const CONFIG:   usize,
                                          RemoteMessages:    ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
                                          LocalMessages:     ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
                                          ProcessorUniType:  GenericUni<ItemType=RemoteMessages>                                         + Send + Sync + 'static,
                                          SenderChannelType: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Sync + Send + 'static> {
    /// The interface to listen to incoming connections
    interface_ip:                String,
    /// The port to listen to incoming connections
    port:                        u16,
    /// Signaler to stop the server
    server_shutdown_signaler:    Option<tokio::sync::oneshot::Sender<u32>>,
    /// Signaler to cause [wait_for_shutdown()] to return
    local_shutdown_receiver:     Option<tokio::sync::oneshot::Receiver<()>>,
    _phantom: PhantomData<(RemoteMessages,LocalMessages,ProcessorUniType,SenderChannelType)>
}

impl<const CONFIG:   usize,
     RemoteMessages:    ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:     ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:  GenericUni<ItemType=RemoteMessages>                                         + Send + Sync + 'static,
     SenderChannelType: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Sync + Send + 'static>
CustomUnresponsiveSocketServer<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannelType> {

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

#[async_trait]
impl<const CONFIG:   usize,
     RemoteMessages:     ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:      ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
     _ProcessorUniType:  GenericUni<ItemType=RemoteMessages>                                         + Send + Sync + 'static,
     _SenderChannelType: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Sync + Send + 'static>
ServerUnresponsiveProcessor for
CustomUnresponsiveSocketServer<CONFIG, RemoteMessages, LocalMessages, _ProcessorUniType, _SenderChannelType> {

    const CONST_CONFIG: ConstConfig = ConstConfig::from(CONFIG);

    async fn spawn_unresponsive_processor<OutputStreamItemsType:                                                                                                                                                                                                                Send + Sync + Debug + 'static,
                                          ServerStreamType:               Stream<Item=OutputStreamItemsType>                                                                                                                                                                  + Send                + 'static,
                                          ConnectionEventsCallbackFuture: Future<Output=()>                                                                                                                                                                                   + Send,
                                          ProcessorUniType:               GenericUni<ItemType=Self::RemoteMessages>                                                                                                                                                           + Send + Sync         + 'static,
                                          SenderChannelType:              FullDuplexUniChannel<ItemType=Self::LocalMessages, DerivedItemType=Self::LocalMessages>                                                                                                             + Sync + Send         + 'static,
                                          ConnectionEventsCallback:       Fn(/*server_event: */ConnectionEvent<SenderChannelType>)                                                                                                          -> ConnectionEventsCallbackFuture + Send + Sync         + 'static,
                                          ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<SenderChannelType>>, /*client_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> ServerStreamType               + Send + Sync         + 'static>

                                         (mut self,
                                          connection_events_callback: ConnectionEventsCallback,
                                          dialog_processor_builder_fn: ProcessorBuilderFn)

                                         -> Result<Arc<dyn SocketServerController>, Box<dyn Error + Sync + Send + 'static>> {

        let (server_shutdown_sender, server_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (local_shutdown_sender, local_shutdown_receiver) = tokio::sync::oneshot::channel::<()>();
        self.server_shutdown_signaler = Some(server_shutdown_sender);
        self.local_shutdown_receiver = Some(local_shutdown_receiver);
        let listening_interface = self.interface_ip.to_string();
        let port = self.port;

        let connection_events_callback = upgrade_to_shutdown_tracking(local_shutdown_sender, connection_events_callback);

        let socket_connection_handler = SocketConnectionHandler::<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannelType>::new();
        socket_connection_handler.server_loop_for_unresponsive_text_protocol(listening_interface.to_string(),
                                                                             port,
                                                                             server_shutdown_receiver,
                                                                             connection_events_callback,
                                                                             dialog_processor_builder_fn).await
            .map_err(|err| format!("Error starting SocketServer @ {listening_interface}:{port}: {:?}", err))?;
        Ok(Arc::new(self))
    }
}

impl<const CONFIG:   usize,
     RemoteMessages:    ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:     ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:  GenericUni<ItemType=RemoteMessages>                                         + Send + Sync + 'static,
     SenderChannelType: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Sync + Send + 'static>
SocketServerController for
CustomUnresponsiveSocketServer<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannelType> {

    fn shutdown_waiter(&mut self) -> Box<dyn FnOnce() -> BoxFuture<'static, Result<(), Box<dyn std::error::Error + Send + Sync>>> > {
        let mut local_shutdown_receiver = self.local_shutdown_receiver.take();
        Box::new(move || Box::pin({
            async move {
                if let Some(local_shutdown_receiver) = local_shutdown_receiver.take() {
                    match local_shutdown_receiver.await {
                        Ok(()) => {
                            Ok(())
                        },
                        Err(err) => Err(Box::from(format!("SocketServer::wait_for_shutdown(): It is no longer possible to tell when the server will be shutdown: `one_shot` signal error: {err}")))
                    }
                } else {
                    Err(Box::from("SocketServer: \"wait for shutdown\" requested, but the service was not started (or a previous shutdown was commanded) at the moment `shutdown_waiter()` was called"))
                }
            }
        }))
    }

    fn shutdown(mut self, timeout_ms: u32) -> Result<(), Box<dyn std::error::Error>> {
        match self.server_shutdown_signaler.take() {
            Some(server_sender) => {
                warn!("Socket Server: Shutdown asked & initiated for server @ {}:{} -- timeout: {timeout_ms}ms", self.interface_ip, self.port);
                if let Err(_sent_value) = server_sender.send(timeout_ms) {
                    Err(Box::from("Socket Server BUG: couldn't send shutdown signal to the network loop. Program is, likely, hanged. Please, investigate and fix!"))
                } else {
                    Ok(())
                }
            }
            None => {
                Err(Box::from("Socket Server: Shutdown requested, but the service was not started. Ignoring..."))
            }
        }
    }

}

impl<const CONFIG:   usize,
     RemoteMessages:    ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:     ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:  GenericUni<ItemType=RemoteMessages>                                         + Send + Sync + 'static,
     SenderChannelType: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Sync + Send + 'static>
UnresponsiveSocketServerAssociatedTypes for
CustomUnresponsiveSocketServer<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannelType> {

    type ProcessorUniType    = ProcessorUniType;
    type SenderChannelType   = SenderChannelType;
    type ConnectionEventType = ConnectionEvent<SenderChannelType>;
}

impl<const CONFIG:   usize,
     RemoteMessages:    ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:     ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:  GenericUni<ItemType=RemoteMessages>                                         + Send + Sync + 'static,
     SenderChannelType: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Sync + Send + 'static>
UnresponsiveSocketServerGenericTypes for
CustomUnresponsiveSocketServer<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannelType> {

    type RemoteMessages      = RemoteMessages;
    type LocalMessages       = LocalMessages;
    type StreamItemType      = ProcessorUniType::DerivedItemType;
    type StreamType          = MessagingMutinyStream<ProcessorUniType>;
}