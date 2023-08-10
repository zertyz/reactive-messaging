//! Provides a Server whose processor function yields messages to be sent back to the client
//! See also [super::unresponsive_socket_server]
//!
//! In case some messages yields no messages, the [ResponsiveMessages<>] trait provides a "No Answer" option, that may be returned by the dialog processor.\
//! If more than one message needs to be sent for each input, consider using [Stream::flat_map()] or reverting back to the [super::unresponsive_socket_server].


use std::error::Error;
use std::fmt::Debug;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;
use async_trait::async_trait;
use futures::future::BoxFuture;
use futures::Stream;
use log::warn;
use reactive_mutiny::prelude::advanced::{ChannelUniMoveAtomic, ChannelUniMoveCrossbeam, ChannelUniMoveFullSync, UniMoveCrossbeam, UniZeroCopyAtomic, UniZeroCopyFullSync};
use reactive_mutiny::prelude::{FullDuplexUniChannel, GenericUni};
use crate::{ReactiveMessagingDeserializer, ReactiveMessagingSerializer, ResponsiveMessages};
use crate::config::{Channels, ConstConfig};
use crate::prelude::{ConnectionEvent, MessagingMutinyStream};
use crate::socket_connection_handler::{Peer, SocketConnectionHandler};
use crate::socket_server::common::upgrade_to_shutdown_tracking;
use crate::socket_server::socket_server_types::{ResponsiveSocketServerAssociatedTypes, ServerResponsiveProcessor, SocketServerController};


/// This is the recommended one
pub struct ResponsiveSocketServer<const CONFIG:   usize,
                                  RemoteMessages: ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug + 'static,
                                  LocalMessages:  ReactiveMessagingSerializer<LocalMessages>    +
                                                  ResponsiveMessages<LocalMessages>             + Send + Sync + PartialEq + Debug + 'static> {

    /// The interface to listen to incoming connections
    interface_ip:                String,
    /// The port to listen to incoming connections
    port:                        u16,
}

#[async_trait]
impl<const CONFIG:   usize,
     RemoteMessages: ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:  ReactiveMessagingSerializer<LocalMessages>    +
                     ResponsiveMessages<LocalMessages>             + Send + Sync + PartialEq + Debug + 'static>
ServerResponsiveProcessor<Self::CONST_CONFIG> for
ResponsiveSocketServer<CONFIG, RemoteMessages, LocalMessages> {

    const CONST_CONFIG: ConstConfig = ConstConfig::from(CONFIG);

    /// Spawns a task to run a Server listening @ `self`'s `interface_ip` & `port` and returns, immediately,
    /// an object through which the caller may inquire some stats (if opted in) and request the server to shutdown.\
    /// The given `dialog_processor_builder_fn` will be called for each new client and will return a `reactive-mutiny` Stream
    /// that will produce non-futures & non-fallibles `ServerMessages` that will be sent to the clients.
    async fn spawn_responsive_processor<ServerStreamType:               Stream<Item=LocalMessages>                   + Send + 'static,
                                        ConnectionEventsCallbackFuture: Future<Output=()>                            + Send,
                                        ConnectionEventsCallback:       Fn(/*server_event: */Self::ConnectionEventType)                                                                                                  -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                        ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<Self::SenderChannelType>>, /*client_messages_stream: */Self::StreamType) -> ServerStreamType               + Send + Sync + 'static>

                                       (mut self,
                                        connection_events_callback:  ConnectionEventsCallback,
                                        dialog_processor_builder_fn: ProcessorBuilderFn)

                                       -> Result<dyn SocketServerController, Box<dyn std::error::Error + Sync + Send + 'static>> {

        match Self::CONST_CONFIG.channel {
            Channels::Atomic => {
                let server = CustomResponsiveSocketServer::<{Self::CONST_CONFIG.executor_instruments.into()},
                                                                 {Self::CONST_CONFIG.receiver_buffer as usize},
                                                                 RemoteMessages,
                                                                 LocalMessages,
                                                                 UniZeroCopyAtomic<RemoteMessages, {Self::CONST_CONFIG.receiver_buffer as usize}, {Self::CONST_CONFIG.executor_instruments.into()}>,
                                                                 ChannelUniMoveAtomic<LocalMessages, {Self::CONST_CONFIG.sender_buffer as usize}, 1> >  :: new(self.interface_ip, self.port);
                server.spawn_responsive_processor(connection_events_callback, dialog_processor_builder_fn)
            },
            Channels::FullSync => {
                let server = CustomResponsiveSocketServer::<{Self::CONST_CONFIG.executor_instruments.into()},
                                                                 {Self::CONST_CONFIG.receiver_buffer as usize},
                                                                 RemoteMessages,
                                                                 LocalMessages,
                                                                 UniZeroCopyFullSync<RemoteMessages, {Self::CONST_CONFIG.receiver_buffer as usize}, {Self::CONST_CONFIG.executor_instruments.into()}>,
                                                                 ChannelUniMoveFullSync<LocalMessages, {Self::CONST_CONFIG.sender_buffer as usize}, 1> > :: new(self.interface_ip, self.port);
                server.spawn_responsive_processor(connection_events_callback, dialog_processor_builder_fn)
            },
            Channels::Crossbean => {
                let server = CustomResponsiveSocketServer::<{Self::CONST_CONFIG.executor_instruments.into()},
                                                                 {Self::CONST_CONFIG.receiver_buffer as usize},
                                                                 RemoteMessages,
                                                                 LocalMessages,
                                                                 UniMoveCrossbeam<RemoteMessages, {Self::CONST_CONFIG.receiver_buffer as usize}, {Self::CONST_CONFIG.executor_instruments.into()}>,
                                                                 ChannelUniMoveCrossbeam<LocalMessages, {Self::CONST_CONFIG.sender_buffer as usize}, 1> > :: new(self.interface_ip, self.port);
                server.spawn_responsive_processor(connection_events_callback, dialog_processor_builder_fn)
            },
        }
    }

}

/// This is the internal one. Users must use [UnresponsiveSocketServer] instead.
#[derive(Debug)]
pub struct CustomResponsiveSocketServer<const PROCESSOR_UNI_INSTRUMENTS:      usize,
                                        const PROCESSOR_BUFFER_SIZE_PER_PEER: usize,
                                        RemoteMessages:    ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
                                        LocalMessages:     ReactiveMessagingSerializer<LocalMessages>                                  +
                                                           ResponsiveMessages<LocalMessages>                                           + Send + Sync + PartialEq + Debug + 'static,
                                        ProcessorUniType:  GenericUni<PROCESSOR_UNI_INSTRUMENTS, ItemType=RemoteMessages>              + Send + Sync + 'static,
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

impl<const PROCESSOR_UNI_INSTRUMENTS:      usize,
     const PROCESSOR_BUFFER_SIZE_PER_PEER: usize,
     RemoteMessages:    ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:     ReactiveMessagingSerializer<LocalMessages>                                  +
                        ResponsiveMessages<LocalMessages>                                           + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:  GenericUni<PROCESSOR_UNI_INSTRUMENTS, ItemType=RemoteMessages>              + Send + Sync + 'static,
     SenderChannelType: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Sync + Send + 'static>
CustomResponsiveSocketServer<PROCESSOR_UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE_PER_PEER, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannelType> {

    /// Creates a new server instance listening on TCP/IP:
    ///   `interface_ip`:         the interface's IP to listen to -- 0.0.0.0 will cause listening to all network interfaces
    ///   `port`:                 what port to listen to
    ///   `processor_builder_fn`: a function to instantiate a new processor `Stream` whenever a new connection arrives
    pub fn new<IntoString: Into<String>>
              (interface_ip: IntoString,
               port:         u16)
               -> Self {
        Self {
            interface_ip: interface_ip.into(),
            port,
            server_shutdown_signaler: None,
            local_shutdown_receiver: None,
            _phantom: PhantomData,
        }
    }

    /// Useful for Generic Programming, shares the internal generic parameter
    pub const fn uni_instruments() -> usize {
        Self::PROCESSOR_UNI_INSTRUMENTS
    }
}

#[async_trait]
impl<const PROCESSOR_UNI_INSTRUMENTS:      usize,
     const PROCESSOR_BUFFER_SIZE_PER_PEER: usize,
     RemoteMessages:    ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:     ReactiveMessagingSerializer<LocalMessages>                                  +
                        ResponsiveMessages<LocalMessages>                                           + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:  GenericUni<PROCESSOR_UNI_INSTRUMENTS, ItemType=RemoteMessages>              + Send + Sync + 'static,
     SenderChannelType: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Sync + Send + 'static>
ServerResponsiveProcessor<PROCESSOR_UNI_INSTRUMENTS> for
CustomResponsiveSocketServer<PROCESSOR_UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE_PER_PEER, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannelType> {

    type ServerController = Self;

    /// Spawns a task to run a Server listening @ `self`'s `interface_ip` & `port` and returns, immediately,
    /// an object through which the caller may inquire some stats (if opted in) and request the server to shutdown.\
    /// The given `dialog_processor_builder_fn` will be called for each new client and will return a `reactive-mutiny` Stream
    /// that will produce non-futures & non-fallibles `ServerMessages` that will be sent to the clients.
    async fn spawn_responsive_processor<const CONFIG:                     usize,
                                        ServerStreamType:                 Stream<Item=<Self as ResponsiveSocketServerAssociatedTypes<PROCESSOR_UNI_INSTRUMENTS>>::LocalMessages>                                                                                                                                                                                                                         + Send + 'static,
                                        ConnectionEventsCallbackFuture:   Future<Output=()>                                                                                                                                                                                                                                                                                                              + Send,
                                        ConnectionEventsCallback:         Fn(/*server_event: */<Self as ResponsiveSocketServerAssociatedTypes<PROCESSOR_UNI_INSTRUMENTS>>::ConnectionEventType)                                                                                                                                                                        -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
                                        ProcessorBuilderFn:               Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<<Self as ResponsiveSocketServerAssociatedTypes<PROCESSOR_UNI_INSTRUMENTS>>::SenderChannelType>>, /*client_messages_stream: */<Self as ResponsiveSocketServerAssociatedTypes<PROCESSOR_UNI_INSTRUMENTS>>::StreamType) -> ServerStreamType               + Send + Sync + 'static>

                                       (mut self,
                                        connection_events_callback:  ConnectionEventsCallback,
                                        dialog_processor_builder_fn: ProcessorBuilderFn)

                                       -> Result<Self::ServerController, Box<dyn std::error::Error + Sync + Send + 'static>> {

        let (server_shutdown_sender, server_shutdown_receiver) = tokio::sync::oneshot::channel::<u32>();
        let (local_shutdown_sender, local_shutdown_receiver) = tokio::sync::oneshot::channel::<()>();
        self.server_shutdown_signaler = Some(server_shutdown_sender);
        self.local_shutdown_receiver = Some(local_shutdown_receiver);
        let listening_interface = self.interface_ip.to_string();
        let port = self.port;

        let connection_events_callback = upgrade_to_shutdown_tracking(local_shutdown_sender, connection_events_callback);

        let socket_connection_handler = SocketConnectionHandler::<CONFIG, RemoteMessages, LocalMessages, Self::ProcessorUniType, Self::SenderChannelType, PROCESSOR_UNI_INSTRUMENTS>::new();
        socket_connection_handler.server_loop_for_responsive_text_protocol
            (listening_interface.to_string(),
             port,
             server_shutdown_receiver,
             connection_events_callback,
             dialog_processor_builder_fn).await
            .map_err(|err| format!("Error starting SocketServer @ {listening_interface}:{port}: {:?}", err))?;
        Ok(self)
    }
}

impl<const PROCESSOR_UNI_INSTRUMENTS:      usize,
     const PROCESSOR_BUFFER_SIZE_PER_PEER: usize,
     RemoteMessages:    ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:     ReactiveMessagingSerializer<LocalMessages>                                  +
                        ResponsiveMessages<LocalMessages>                                           + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:  GenericUni<PROCESSOR_UNI_INSTRUMENTS, ItemType=RemoteMessages>              + Send + Sync + 'static,
     SenderChannelType: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Sync + Send + 'static>
SocketServerController for
CustomResponsiveSocketServer<PROCESSOR_UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE_PER_PEER, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannelType> {

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

impl<const PROCESSOR_UNI_INSTRUMENTS:      usize,
     const PROCESSOR_BUFFER_SIZE_PER_PEER: usize,
     RemoteMessages:    ReactiveMessagingDeserializer<RemoteMessages>                               + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:     ReactiveMessagingSerializer<LocalMessages>                                  +
                        ResponsiveMessages<LocalMessages>                                           + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:  GenericUni<PROCESSOR_UNI_INSTRUMENTS, ItemType=RemoteMessages>              + Send + Sync + 'static,
     SenderChannelType: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Sync + Send + 'static>
ResponsiveSocketServerAssociatedTypes<PROCESSOR_UNI_INSTRUMENTS> for
CustomResponsiveSocketServer<PROCESSOR_UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE_PER_PEER, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannelType> {

    const PROCESSOR_UNI_INSTRUMENTS: usize = PROCESSOR_UNI_INSTRUMENTS;
    type RemoteMessages      = RemoteMessages;
    type LocalMessages       = LocalMessages;
    type ProcessorUniType    = ProcessorUniType;
    type SenderChannelType   = SenderChannelType;
    type ConnectionEventType = ConnectionEvent<SenderChannelType>;
    type StreamItemType      = ProcessorUniType::DerivedItemType;
    type StreamType          = MessagingMutinyStream<PROCESSOR_UNI_INSTRUMENTS, ProcessorUniType>;
}