//! Common types used across this crate

use crate::socket_connection::Peer;
use std::{
    fmt::Debug,
    sync::Arc,
};
use std::future::Future;
use async_trait::async_trait;
use futures::future::BoxFuture;
use futures::Stream;
use reactive_mutiny::prelude::{FullDuplexUniChannel, GenericUni, MutinyStream};
use crate::{ReactiveMessagingDeserializer, ReactiveMessagingSerializer};
use crate::socket_connection::common::RetryableSender;


/// Concrete type of the `Stream`s this crate produces.\
/// Type for the `Stream` we create when reading from the remote peer.\
/// This type is intended to be used only for the first level of `dialog_processor_builder()`s you pass to
/// the [SocketClient] or [SocketServer], as Rust Generics isn't able to infer a generic `Stream` type
/// in this situation (in which the `Stream` is created inside the generic function itself).\
/// If your logic uses functions that receive `Stream`s, you'll want flexibility to do whatever you want
/// with the `Stream` (which would no longer be a `MutinyStream`), so declare such functions as:
/// ```no_compile
///     fn dialog_processor<RemoteStreamType: Stream<Item=SocketProcessorDerivedType<RemoteMessages>>>
///                        (remote_messages_stream: RemoteStreamType) -> impl Stream<Item=LocalMessages> { ... }
pub type MessagingMutinyStream<GenericUniType: GenericUni> = MutinyStream<'static, GenericUniType::ItemType, GenericUniType::UniChannelType, GenericUniType::DerivedItemType>;


/// Adherents will, typically, also implement [ReactiveMessagingUnresponsiveSerializer].\
/// By upgrading your type with this trait, it is possible to build a "Responsive Processor", where the returned `Stream`
/// contains the messages to be sent as an answer to the remote peer.\
/// This trait, therefore, specifies (to the internal sender) how to handle special response cases, like "no answer" and "disconnection" messages.
pub trait ResponsiveMessages<LocalPeerMessages: ResponsiveMessages<LocalPeerMessages> + Send + PartialEq + Debug> {

    /// Informs the internal sender if the given `processor_answer` is a "disconnect" message & command (issued by the messages processor logic)\
    /// -- in which case, the network processor will send it and, immediately, close the connection.\
    /// IMPLEMENTORS: #[inline(always)]
    fn is_disconnect_message(processor_answer: &LocalPeerMessages) -> bool;

    /// Tells if internal sender if the given `processor_answer` represents a "no message" -- a message that should produce no answer to the peer.\
    /// IMPLEMENTORS: #[inline(always)]
    fn is_no_answer_message(processor_answer: &LocalPeerMessages) -> bool;
}


/// The internal events a reactive processor (for a server or client) shares with the user code.\
/// The user code may use those events to maintain a list of connected clients, be notified of stop/close/quit requests, init/de-init sessions, etc.
/// Note that the `Peer` objects received in those events may be used, at any time, to send messages to the clients -- like "Shutting down. Goodbye".
/// IMPLEMENTATION NOTE: GAT traits (to reduce the number of generic parameters) couldn't be used here -- even after applying this compiler bug workaround https://github.com/rust-lang/rust/issues/102211#issuecomment-1513931928
///                      -- the "error: implementation of `std::marker::Send` is not general enough" bug kept on popping up in user provided closures that called other async functions.
#[derive(Debug)]
pub enum ConnectionEvent<const CONFIG:  u64,
                         LocalMessages: ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
                         SenderChannel: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync> {
    PeerConnected       {peer: Arc<Peer<CONFIG, LocalMessages, SenderChannel>>},
    PeerDisconnected    {peer: Arc<Peer<CONFIG, LocalMessages, SenderChannel>>, stream_stats: Arc<dyn reactive_mutiny::stream_executor::StreamExecutorStats + Sync + Send>},
    ApplicationShutdown {timeout_ms: u32},
}


/// Controls a Server or Client after they have been started -- shutting down, asking for stats, ....
pub trait ReactiveProcessorController: Send {
    /// Returns an async closure that blocks until [Self::shutdown()] is called.
    /// Example:
    /// ```no_compile
    ///     self.shutdown_waiter()().await;
    fn shutdown_waiter(&mut self) -> Box<dyn FnOnce() -> BoxFuture<'static, Result<(), Box<dyn std::error::Error + Send + Sync>>> >;

    /// Notifies the processor it is time to shutdown.\
    /// A shutdown is considered graceful if it could be accomplished in less than `timeout_ms` milliseconds.\
    /// It is a good practice that the `connection_events_handler()` you provided when starting the processor
    /// uses this time to inform all peers that a remote-initiated disconnection (due to a shutdown) is happening.
    fn shutdown(self: Box<Self>, timeout_ms: u32) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}


// /// Adds the capacity of processing incoming message `Stream`s that won't produce any output back to their peer.\
// /// See [ReactiveResponsiveProcessor] if you want the processor to produce the appropriate output `Stream` of messages to be sent back as responses.
// #[async_trait]
// pub trait ReactiveUnresponsiveProcessor: ReactiveUnresponsiveProcessorAssociatedTypes {
//
//     /// Spawns a task to start the local processor and returns, immediately,
//     /// an object through which the caller may inquire some stats (if opted in) and request the processor to shutdown.\
//     /// The given `dialog_processor_builder_fn` will be called for each new connection and will return a `Stream`
//     /// that will produce non-futures & non-fallibles items that won't be sent to the peer.\
//     /// -- if you want the processor to produce "answer messages" to the peers, see [ReactiveResponsiveProcessor::spawn_responsive_processor()].
//     async fn spawn_unresponsive_processor<OutputStreamItemsType:                                                                                                                                                                                                                              Send + Sync + Debug + 'static,
//                                           ServerStreamType:               Stream<Item=OutputStreamItemsType>                                                                                                                                                                                + Send + Sync         + 'static,
//                                           ConnectionEventsCallbackFuture: Future<Output=()>                                                                                                                                                                                                 + Send                + 'static,
//                                           ConnectionEventsCallback:       Fn(/*server_event: */ConnectionEvent<Self::RetryableSenderImpl>)                                                                                                                -> ConnectionEventsCallbackFuture + Send + Sync         + 'static,
//                                           ProcessorBuilderFn:             Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<Self::RetryableSenderImpl>>, /*client_messages_stream: */MessagingMutinyStream<Self::ProcessorUniType>) -> ServerStreamType               + Send + Sync         + 'static>
//
//                                          (mut self,
//                                           connection_events_callback: ConnectionEventsCallback,
//                                           dialog_processor_builder_fn: ProcessorBuilderFn)
//
//                                          -> Result<Box<dyn ReactiveProcessorController + Send>, Box<dyn std::error::Error + Sync + Send>>;
//
// }


// /// Adds the capacity of processing incoming message `Stream`s that will produce the appropriate output `Stream` of messages to be sent back as responses.\
// /// See [ReactiveUnresponsiveProcessor] if you want a processor that will produce no output messages.
// #[async_trait]
// pub trait ReactiveResponsiveProcessor: ReactiveResponsiveProcessorAssociatedTypes {
//
//     /// Spawns a task to start the local processor and returns, immediately,
//     /// an object through which the caller may inquire some stats (if opted in) and request the processor to shutdown.\
//     /// The given `dialog_processor_builder_fn` will be called for each new connection and will return a `Stream`
//     /// that will produce non-futures & non-fallibles items that will be sent to the peer.\
//     /// -- if you don't want the processor to produce "answer messages", see [ReactiveUnresponsiveProcessor::spawn_unresponsive_processor()].
//     async fn spawn_responsive_processor<ServerStreamType:                Stream<Item=Self::LocalMessages>                                                                                                                                                                                  + Send        + 'static,
//                                         ConnectionEventsCallbackFuture:  Future<Output=()>                                                                                                                                                                                                 + Send        + 'static,
//                                         ConnectionEventsCallback:        Fn(/*server_event: */ConnectionEvent<Self::RetryableSenderImpl>)                                                                                                                -> ConnectionEventsCallbackFuture + Send + Sync + 'static,
//                                         ProcessorBuilderFn:              Fn(/*client_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<Self::RetryableSenderImpl>>, /*client_messages_stream: */MessagingMutinyStream<Self::ProcessorUniType>) -> ServerStreamType               + Send + Sync + 'static>
//
//                                        (mut self,
//                                         connection_events_callback:  ConnectionEventsCallback,
//                                         dialog_processor_builder_fn: ProcessorBuilderFn)
//
//                                        -> Result<Box<dyn ReactiveProcessorController>, Box<dyn std::error::Error + Sync + Send>>;
//
// }


/// Additional contract for Socket Clients
pub trait ReactiveSocketClient {

    /// Tells if this client is connected to the server
    fn is_connected(&self) -> bool;
}


/// Helps to infer some types used by [UnresponsiveSocketServer] & [UnresponsiveSocketClient] implementations -- also helping to simplify Generic Programming when used as a Generic Parameter.\
pub trait ReactiveUnresponsiveProcessorAssociatedTypes {
    type RemoteMessages:      ReactiveMessagingDeserializer<Self::RemoteMessages>                                     + Send + Sync + PartialEq + Debug + 'static;
    type LocalMessages:       ReactiveMessagingSerializer<Self::LocalMessages>                                        + Send + Sync + PartialEq + Debug + 'static;
    type ProcessorUniType:    GenericUni<ItemType=Self::RemoteMessages>                                               + Send + Sync                     + 'static;
    type RetryableSenderImpl: RetryableSender                                                                         + Send + Sync                     + 'static;
    type ConnectionEventType;
    type StreamItemType;
    type StreamType;
}

/// Helps to infer some types used by [ResponsiveSocketServer] & [ResponsiveSocketClient] implementations -- also helping to simplify Generic Programming when used as a Generic Parameter.\
pub trait ReactiveResponsiveProcessorAssociatedTypes {
    type RemoteMessages:      ReactiveMessagingDeserializer<Self::RemoteMessages>                                     + Send + Sync + PartialEq + Debug + 'static;
    type LocalMessages:       ReactiveMessagingSerializer<Self::LocalMessages>                                        +
                              ResponsiveMessages<Self::LocalMessages>                                                 + Send + Sync + PartialEq + Debug + 'static;
    type ProcessorUniType:    GenericUni<ItemType=Self::RemoteMessages>                                               + Send + Sync                     + 'static;
    type RetryableSenderImpl: RetryableSender                                                                         + Send + Sync                     + 'static;
    type ConnectionEventType;
    type StreamItemType;
    type StreamType;
}
