//! Provides the [new_socket_client!()] & [new_composite_socket_client!()] macros for instantiating clients, which may then be started by
//! [start_client_processor!()], or have the composite processors spawned by [spawn_composite_client_processor!()].
//!
//! The reactive processing logic will take in a `Stream` as parameter and may return another `Stream` as output,
//! which can be configured to automatically send yielded items to the remote party by composing it with the functions defined in
//! [crate::prelude::ResponsiveStream].
//!
//! Even if the `Stream` isn't upgraded to a "responsive stream", processors still allow you to send messages to the server and should be
//! preferred (as far as performance is concerned) for protocols where the count of messages IN don't nearly map 1/1 with the count of messages OUT.
//!
//! For every client processor, the `Stream` items may be a combination of fallible/non-fallible (using `Result<>` or not) & future/non-future.
//!
//! Instead of using the mentioned macros, you might want to take a look at [CompositeSocketClient] to access the inner implementation directly
//! -- both ways have the same flexibility, but the macro version takes in all parameters in the conveniently packed and documented [ConstConfig]
//! struct, instead of requiring several const generic parameters.


use crate::{
    socket_services::socket_client::common::upgrade_to_connection_event_tracking,
    types::{
        ProtocolEvent,
        MessagingMutinyStream,
    },
    socket_connection::{
        peer::Peer,
        socket_connection_handler::SocketConnectionHandler,
        connection_provider::{ClientConnectionManager, ConnectionChannel},
    },
};
use crate::socket_connection::connection::SocketConnection;
use crate::socket_services::types::MessagingService;
use crate::types::ConnectionEvent;
use std::{
    error::Error,
    fmt::Debug,
    future::Future,
    sync::{
        Arc,
        atomic::AtomicBool,
    },
};
use std::sync::atomic::Ordering::Relaxed;
use reactive_mutiny::prelude::{FullDuplexUniChannel, GenericUni};
use futures::{future::BoxFuture, Stream};
use tokio::io::AsyncWriteExt;
use log::{trace, warn, error, debug};


/// Instantiates & allocates resources for a stateless [CompositeSocketClient] (suitable for single protocol communications),
/// ready to be later started by [`start_client_processor!()`].
///
/// Params:
///   - `const_config`: [ConstConfig] -- the configurations for the client, enforcing const/compile time optimizations;
///   - `host`: IntoString -- the interface to listen to incoming connections;
///   - `port`: u16 -- the port to listen to incoming connections;
///
/// Example:
/// ```nocompile
///     let mut client = new_socket_client!(CONFIG, "google.com", 80);
/// ```
/// See [`new_composite_socket_client!()`] if you want to use the "Composite Protocol Stacking" pattern.
#[macro_export]
macro_rules! new_socket_client {
    ($const_config:    expr,
     $host:            expr,
     $port:            expr) => {
        $crate::new_composite_socket_client!($const_config, $host, $port, ())
    }
}
pub use new_socket_client;


/// Instantiates & allocates resources for a stateful [CompositeSocketClient] (suitable for the "Composite Protocol Stacking" pattern),
/// ready to have processors added by [spawn_client_processor!()].
///
/// Params:
///   - `const_config`: [ConstConfig] -- the configurations for the client, enforcing const/compile time optimizations;
///   - `const_config`: [ConstConfig] -- the configurations for the client, enforcing const/compile time optimizations;
///   - `host`: IntoString -- the interface to listen to incoming connections;
///   - `port`: u16 -- the port to listen to incoming connections;
///   - `state_type` -- The type (suggested: a `enum`) used by the "connection routing closure" (to be provided) to promote the "Composite Protocol Stacking" pattern.
///
/// Example:
/// ```nocompile
///     let mut client = new_composite_socket_client!(CONFIG, "localhost", 8768, MyStatesEnum);
/// ```
/// See [new_socket_client!()] if you want to use the "Composite Protocol Stacking" pattern.\
#[macro_export]
macro_rules! new_composite_socket_client {
    ($const_config:    expr,
     $ip:              expr,
     $port:            expr,
     $state_type:      ty) => {{
        const _CONFIG:      u64          = $const_config.into();
        $crate::prelude::CompositeSocketClient::<_CONFIG, $state_type>::new($ip, $port)
    }}
}
pub use new_composite_socket_client;


/// Spawns a processor for a client (previously instantiated by [`new_composite_socket_client!()`]) that may communicate with the server using multiple protocols / multiple calls
/// to this macro with different parameters -- and finally calling [CompositeSocketClient::start_multi_protocol()] when the "Composite Protocol Stacking" is complete.
///
/// The given `dialog_processor_builder_fn` is a builder of `Stream`s, as specified in [CompositeSocketClient::spawn_processor()].\
/// If you don't need multiple protocols and don't want to follow the "Composite Protocol Stacking" pattern, see the [`start_client_processor!()`] macro instead.
///
/// Params:
///   - `const_config`: [ConstConfig] -- the configurations for the client, enforcing const/compile time optimizations;
///   - `message_kind`: One of the following [dialog_processor]s / [NessageForms]: `Textual`, `VariableBinary` or `MmapBinary`
///   - `channel_type`: One of the following `reactive-mutiny` channels to be used for message passing. Either `Atomic`, `FullSync` or `Crossbeam`;
///   - `socket_client`: [CompositeSocketClient] -- The object returned by the call to [`new_socket_client!()`];
///   - `remote_messages`: [ReactiveMessagingTextualDeserializer<>] -- the type of the messages produced by the server;
///   - `local_messages`: [ReactiveMessagingTextualSerializer<>] -- the type of the messages produced by this client -- should, additionally, implement the `Default` trait;
///   - `protocol_events_handler_fn`: async [Fn] -- The callback that receives the connection/protocol events [ProtocolEvent];
///   - `dialog_processor_builder_fn`: [FnOnce] -- The builder for the `Stream` that consumes server messages.
///
/// `protocol_events_handler_fn` -- a generic function (or closure) to handle "new peer", "peer left" and "service termination" events (possibly to manage sessions). Sign it as:
/// ```nocompile
///     async fn protocol_events_handler<const CONFIG:  u64,
///                                      LocalMessages: ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
///                                      SenderChannel: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync>
///                                     (_event: ProtocolEvent<CONFIG, LocalMessages, SenderChannel, StateType>) {...}
/// ```
///
/// `dialog_processor_builder_fn` -- the generic function (or closure) -- called once for each connection -- that receives the `Stream` of remote messages and returns
///                                  another `Stream`, that may be, optionally, sent out to the `peer` (see [crate::prelude::ResponsiveStream]). Sign it as:
/// ```nocompile
///     fn processor<const CONFIG:   u64,
///                  LocalMessages:  ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
///                  SenderChannel:  FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync,
///                  StreamItemType: Deref<Target=[your type for messages produced by the SERVER]>>
///                 (peer_addr:              String,
///                  connected_port:         u16,
///                  peer:                   Arc<Peer<CONFIG, LocalMessages, SenderChannel, StateType>>,
///                  remote_messages_stream: impl Stream<Item=StreamItemType>)
///                 -> impl Stream<Item=()> {...}
/// ```
///
/// Returns the `handle` that should be used by the closure given to [[CompositeSocketClient::start_multi_protocol()]] to refer to this processor / protocol.
///
/// For examples, please consult the unit tests at the end of this module.
#[macro_export]
macro_rules! spawn_client_processor {

    // "Textual", with the default Ron Serde
    ($const_config:                 expr,
     Textual,
     $channel_type:                 tt,     // one of `Atomic`, `FullSync`, `Crossbeam`
     $socket_client:                expr,
     $remote_messages:              ty,
     $local_messages:               ty,
     $protocol_events_handler_fn:   expr,
     $dialog_processor_builder_fn:  expr) => {{
        $crate::_define_processor_uni_and_sender_channel_types!($const_config, $channel_type, $remote_messages, $local_messages);
        let socket_dialog = $crate::socket_connection::socket_dialog::textual_dialog::TextualDialog::<_CONFIG, $remote_messages, $local_messages, $crate::prelude::ReactiveMessagingRonSerializer, $crate::prelude::ReactiveMessagingRonDeserializer, ProcessorUniType, SenderChannel, _>::default();
        use $crate::prelude::MessagingService;
        $socket_client.spawn_processor::<$remote_messages, $local_messages, ProcessorUniType, SenderChannel, _, _, _, _, _, _>(socket_dialog, $protocol_events_handler_fn, $dialog_processor_builder_fn).await
    }};

    // "Textual", with custom Serde
    ($const_config:                 expr,
     Textual,
     $serializer:                   tt,     // a type implementing `ReactiveMessagingSerializer`
     $deserializer:                 tt,     // a type implementing `ReactiveMessagingDeserializer`
     $channel_type:                 tt,     // one of `Atomic`, `FullSync`, `Crossbeam`
     $socket_client:                expr,
     $remote_messages:              ty,
     $local_messages:               ty,
     $protocol_events_handler_fn:   expr,
     $dialog_processor_builder_fn:  expr) => {{
        $crate::_define_processor_uni_and_sender_channel_types!($const_config, $channel_type, $remote_messages, $local_messages);
        let socket_dialog = $crate::socket_connection::socket_dialog::textual_dialog::TextualDialog::<_CONFIG, $remote_messages, $local_messages, $serializer, $deserializer, ProcessorUniType, SenderChannel, _>::default();
        use $crate::prelude::MessagingService;
        $socket_client.spawn_processor::<$remote_messages, $local_messages, ProcessorUniType, SenderChannel, _, _, _, _, _, _>(socket_dialog, $protocol_events_handler_fn, $dialog_processor_builder_fn).await
    }};

    // Variable Sized binaries, with the default Rkyv Serde
    ($const_config:                 expr,
     VariableBinary,
     $channel_type:                 tt,     // one of `Atomic`, `FullSync`, `Crossbeam`
     $socket_client:                expr,
     $remote_messages:              ty,
     $local_messages:               ty,
     $protocol_events_handler_fn:   expr,
     $dialog_processor_builder_fn:  expr) => {{
        type _DERIVED_REMOTE_MESSAGES = $crate::socket_connection::socket_dialog::serialized_binary_dialog::SerializedWrapperType::<$remote_messages, $crate::prelude::ReactiveMessagingRkyvFastDeserializer>;
        $crate::_define_processor_uni_and_sender_channel_types!($const_config, $channel_type, _DERIVED_REMOTE_MESSAGES, $local_messages);
        let socket_dialog = $crate::socket_connection::socket_dialog::serialized_binary_dialog::SerializedBinaryDialog::<_CONFIG, $remote_messages, $local_messages, $crate::prelude::ReactiveMessagingRkyvSerializer, $crate::prelude::ReactiveMessagingRkyvFastDeserializer, ProcessorUniType, SenderChannel, _>::default();
        use $crate::prelude::MessagingService;
        $socket_client.spawn_processor::<_DERIVED_REMOTE_MESSAGES, $local_messages, ProcessorUniType, SenderChannel, _, _, _, _, _, _>(socket_dialog, $protocol_events_handler_fn, $dialog_processor_builder_fn).await
    }};

    // Variable Sized binaries, with custom Serde
    ($const_config:                 expr,
     VariableBinary,
     $serializer:                   tt,     // a type implementing `ReactiveMessagingSerializer`
     $deserializer:                 tt,     // a type implementing `ReactiveMessagingDeserializer`
     $channel_type:                 tt,     // one of `Atomic`, `FullSync`, `Crossbeam`
     $socket_client:                expr,
     $remote_messages:              ty,
     $local_messages:               ty,
     $protocol_events_handler_fn:   expr,
     $dialog_processor_builder_fn:  expr) => {{
        type _DERIVED_REMOTE_MESSAGES = $crate::socket_connection::socket_dialog::serialized_binary_dialog::SerializedWrapperType::<$remote_messages, $crate::prelude::ReactiveMessagingRkyvFastDeserializer>;
        $crate::_define_processor_uni_and_sender_channel_types!($const_config, $channel_type, _DERIVED_REMOTE_MESSAGES, $local_messages);
        let socket_dialog = $crate::socket_connection::socket_dialog::serialized_binary_dialog::SerializedBinaryDialog::<_CONFIG, $remote_messages, $local_messages, $serializer, $deserializer, ProcessorUniType, SenderChannel, _>::default();
        use $crate::prelude::MessagingService;
        $socket_client.spawn_processor::<_DERIVED_REMOTE_MESSAGES, $local_messages, ProcessorUniType, SenderChannel, _, _, _, _, _, _>(socket_dialog, $protocol_events_handler_fn, $dialog_processor_builder_fn).await
    }};

    // Mmap Binary
    ($const_config:                 expr,
     MmapBinary,
     $channel_type:                 tt,     // one of `Atomic`, `FullSync`, `Crossbeam`
     $socket_client:                expr,
     $remote_messages:              ty,
     $local_messages:               ty,
     $protocol_events_handler_fn:   expr,
     $dialog_processor_builder_fn:  expr) => {{
        $crate::_define_processor_uni_and_sender_channel_types!($const_config, $channel_type, $remote_messages, $local_messages);
        let socket_dialog = $crate::socket_connection::socket_dialog::mmap_binary_dialog::MmapBinaryDialog::<_CONFIG, $remote_messages, $local_messages, ProcessorUniType, SenderChannel, _>::default();
        use $crate::prelude::MessagingService;
        $socket_client.spawn_processor::<$remote_messages, $local_messages, ProcessorUniType, SenderChannel, _, _, _, _, _, _>(socket_dialog, $protocol_events_handler_fn, $dialog_processor_builder_fn).await
    }};
}
pub use spawn_client_processor;


/// Starts a client (previously instantiated by [`new_socket_client!()`]) that will communicate with the server using a single protocol -- as defined by the given
/// `dialog_processor_builder_fn`, a builder of `Stream`s as specified in [CompositeSocketClient::spawn_processor()].
///
/// If you want to follow the "Composite Protocol Stacking" pattern, see the [`spawn_composite_client_processor!()`] macro instead.
///
/// Params:
///   - `const_config`: [ConstConfig] -- the configurations for the client, enforcing const/compile time optimizations;
///   - `message_kind`: One of the following [dialog_processor]s / [NessageForms]: `Textual`, `VariableBinary` or `MmapBinary`
///   - `channel_type`: One of the following `reactive-mutiny` channels to be used for message passing. Either `Atomic`, `FullSync` or `Crossbeam`;
///   - `socket_client`: [CompositeSocketClient] -- The object returned by the call to [`new_socket_client!()`];
///   - `remote_messages`: [ReactiveMessagingTextualDeserializer<>] -- the type of the messages produced by the server;
///   - `local_messages`: [ReactiveMessagingTextualSerializer<>] -- the type of the messages produced by this client -- should, additionally, implement the `Default` trait;
///   - `connection_events_handler_fn`: async [Fn] -- The callback that receives the connection/protocol events [ProtocolEvent];
///   - `dialog_processor_builder_fn`: [FnOnce] -- The builder for the `Stream` that consumes server messages.
///
/// `connection_events_handler_fn` -- a generic function (or closure) to handle "new peer", "peer left" and "service termination" events (possibly to manage sessions). Sign it as:
/// ```nocompile
///     async fn connection_events_handler<const CONFIG:  u64,
///                                        LocalMessages: ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
///                                        SenderChannel: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync>
///                                       (_event: ProtocolEvent<CONFIG, LocalMessages, SenderChannel, StateType>) {...}
/// ```
///
/// `dialog_processor_builder_fn` -- the generic function (or closure) -- called once for each connection -- that receives the `Stream` of remote messages and returns
///                                  another `Stream`, that may be, optionally, sent out to the `peer` (see [crate::prelude::ResponsiveStream]). Sign it as:
/// ```nocompile
///     fn unresponsive_processor<const CONFIG:   u64,
///                               LocalMessages:  ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
///                               SenderChannel:  FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync,
///                               StreamItemType: Deref<Target=[your type for messages produced by the SERVER]>>
///                              (peer_addr:              String,
///                               connected_port:         u16,
///                               peer:                   Arc<Peer<CONFIG, LocalMessages, SenderChannel, StateType>>,
///                               remote_messages_stream: impl Stream<Item=StreamItemType>)
///                              -> impl Stream<Item=ANY_TYPE> {...}
/// ```
///
/// For examples, please consult the unit tests at the end of this module.
#[macro_export]
macro_rules! start_client_processor {

    // use the default serializers for the given `message_form`
    ($const_config:                 expr,
     $message_form:                 tt,     // one of `Textual`, `VariableBinary` or `MmapBinary`
     $channel_type:                 tt,     // one of `Atomic`, `FullSync`, `Crossbeam`
     $socket_client:                expr,
     $remote_messages:              ty,
     $local_messages:               ty,
     $connection_events_handler_fn: expr,
     $dialog_processor_builder_fn:  expr) => {{
        use $crate::prelude::MessagingService;
        match $crate::spawn_client_processor!($const_config, $message_form, $channel_type, $socket_client, $remote_messages, $local_messages, $connection_events_handler_fn, $dialog_processor_builder_fn) {
            Ok(connection_channel) => $socket_client.start_single_protocol(connection_channel).await,
            Err(err) => Err(err),
        }
    }};

    // use the custom serializers for the given `message_form`
    ($const_config:                 expr,
     $message_form:                 tt,     // one of `Textual`, `VariableBinary` or `MmapBinary`
     $serializer:                   tt,     // a type implementing `ReactiveMessagingSerializer`
     $deserializer:                 tt,     // a type implementing `ReactiveMessagingDeserializer`
     $channel_type:                 tt,     // one of `Atomic`, `FullSync`, `Crossbeam`
     $socket_client:                expr,
     $remote_messages:              ty,
     $local_messages:               ty,
     $connection_events_handler_fn: expr,
     $dialog_processor_builder_fn:  expr) => {{
        use $crate::prelude::MessagingService;
        match $crate::spawn_client_processor!($const_config, $message_form, $serializer, $deserializer, $channel_type, $socket_client, $remote_messages, $local_messages, $connection_events_handler_fn, $dialog_processor_builder_fn) {
            Ok(connection_channel) => $socket_client.start_single_protocol(connection_channel).await,
            Err(err) => Err(err),
        }
    }};

}
pub use start_client_processor;
use crate::serde::ReactiveMessagingConfig;
use crate::socket_connection::socket_dialog::dialog_types::SocketDialog;

/// Real definition & implementation for our Socket Client, full of generic parameters.\
/// Probably you want to instantiate this structure through the sugared macros [new_socket_client!()] or [new_composite_socket_client!()] instead.
/// Generic Parameters:
///   - `CONFIG`:           the `u64` version of the [ConstConfig] instance used to build this struct -- from which `ProcessorUniType` and `SenderChannel` derive;
///   - `RemoteMessages`:   the messages that are generated by the server (usually an `enum`);
///   - `LocalMessages`:    the messages that are generated by this client (usually an `enum`);
///   - `ProcessorUniType`: an instance of a `reactive-mutiny`'s [Uni] type (using one of the zero-copy channels) --
///                         This [Uni] will execute the given client reactive logic for each incoming message (see how it is used in [new_socket_client!()]);
///   - `SenderChannel`:    an instance of a `reactive-mutiny`'s Uni movable `Channel`, which will provide a `Stream` of messages to be sent to the server;
///   - `StateType`:        The state type used by the "connection routing closure" (to be provided), enabling the "Composite Protocol Stacking" pattern.
pub struct CompositeSocketClient<const CONFIG: u64,
                                 StateType:    Send + Sync + Clone + Debug + 'static> {

    /// false if a disconnection happened, as tracked by the socket logic
    connected: Arc<AtomicBool>,
    /// The interface to listen to incoming connections
    ip: String,
    /// The port to listen to incoming connections
    port: u16,
    /// Signaler to stop this client -- allowing multiple subscribers
    client_termination_signaler: Option<tokio::sync::broadcast::Sender<()>>,
    /// Signaler to cause [Self::termination_waiter()]'s closure to return
    local_termination_is_complete_receiver: Option<tokio::sync::mpsc::Receiver<()>>,
    local_termination_is_complete_sender: tokio::sync::mpsc::Sender<()>,
    /// The client connection is returned by processors after they are done with it -- this connection
    /// may be routed to another processor if the "Composite Protocol Stacking" pattern is in play.
    returned_connection_source: Option<tokio::sync::mpsc::Receiver<SocketConnection<StateType>>>,
    returned_connection_sink: tokio::sync::mpsc::Sender<SocketConnection<StateType>>,
    /// The count of processors, for termination notification purposes
    spawned_processors_count: u32,
}
impl<const CONFIG: u64,
     StateType:    Send + Sync + Clone + Debug + 'static>
CompositeSocketClient<CONFIG, StateType> {

    /// Instantiates a client to connect to a TCP/IP Server:
    ///   `ip`:                   the server IP to connect to
    ///   `port`:                 the server port to connect to
    pub fn new<IntoString: Into<String>>
              (ip:   IntoString,
               port: u16)
              -> Self {
        let (returned_connection_sink, returned_connection_source) = tokio::sync::mpsc::channel::<SocketConnection<StateType>>(1);
        let (client_termination_signaler, _) = tokio::sync::broadcast::channel(1);
        let (local_termination_is_complete_sender, local_termination_is_complete_receiver) = tokio::sync::mpsc::channel(1);
        Self {
            connected:                               Arc::new(AtomicBool::new(false)),
            ip:                                      ip.into(),
            port,
            client_termination_signaler:             Some(client_termination_signaler),
            local_termination_is_complete_receiver:  Some(local_termination_is_complete_receiver),
            local_termination_is_complete_sender,
            returned_connection_source:              Some(returned_connection_source),
            returned_connection_sink,
            spawned_processors_count:                0,
        }
    }

    /// Returns the const configuration used for `self`
    const fn config() -> u64 {
        CONFIG
    }

    /// Tells if the connection is active & valid
    pub fn is_connected(&self) -> bool {
        self.connected.load(Relaxed)
    }
}

impl<const CONFIG: u64,
     StateType:    Send + Sync + Clone + Debug + 'static>
MessagingService<CONFIG>
for CompositeSocketClient<CONFIG, StateType> {
    type StateType = StateType;

    // TODO 2024-01-03: make this able to process the same connection as many times as needed, for symmetry with the server -- practically, allowing connection reuse
    #[inline(always)]
    async fn spawn_processor<RemoteMessages:                                                                                                                                                                                                                                                     Send + Sync + PartialEq + Debug + 'static,
                             LocalMessages:                  ReactiveMessagingConfig<LocalMessages>                                                                                                                                                                                            + Send + Sync + PartialEq + Debug + 'static,
                             ProcessorUniType:               GenericUni<ItemType=RemoteMessages>                                                                                                                                                                                               + Send + Sync                     + 'static,
                             SenderChannel:                  FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages>                                                                                                                                                       + Send + Sync                     + 'static,
                             OutputStreamItemsType:                                                                                                                                                                                                                                              Send + Sync             + Debug + 'static,
                             ServerStreamType:               Stream<Item=OutputStreamItemsType>                                                                                                                                                                                                + Send                            + 'static,
                             ConnectionEventsCallbackFuture: Future<Output=()>                                                                                                                                                                                                                 + Send                            + 'static,
                             ConnectionEventsCallback:       Fn(/*event: */ProtocolEvent<CONFIG, LocalMessages, SenderChannel, StateType>)                                                                                                                   -> ConnectionEventsCallbackFuture + Send + Sync                     + 'static,
                             ProcessorBuilderFn:             Fn(/*server_addr: */String, /*connected_port: */u16, /*peer: */Arc<Peer<CONFIG, LocalMessages, SenderChannel, StateType>>, /*server_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> ServerStreamType               + Send + Sync                     + 'static,
                             OriginalRemoteMessages:                                                                                                                                                                                                                                             Send + Sync + PartialEq + Debug + 'static>

                            (&mut self,
                             socket_dialog:               impl SocketDialog<CONFIG, RemoteMessages=OriginalRemoteMessages, LocalMessages=LocalMessages, ProcessorUni=ProcessorUniType, SenderChannel=SenderChannel, State=StateType> + 'static,
                             connection_events_callback:  ConnectionEventsCallback,
                             dialog_processor_builder_fn: ProcessorBuilderFn)

                             -> Result<ConnectionChannel<StateType>, Box<dyn Error + Sync + Send>> {

        let ip = self.ip.clone();
        let port = self.port;

        let returned_connection_sink = self.returned_connection_sink.clone();
        let local_termination_is_complete_sender = self.local_termination_is_complete_sender.clone();
        let client_termination_signaler = self.client_termination_signaler.clone();

        let connection_events_callback = upgrade_to_connection_event_tracking(&self.connected, local_termination_is_complete_sender, connection_events_callback);

        // the source of connections for this processor to start working on
        let mut connection_provider = ConnectionChannel::new();
        let mut connection_source = connection_provider.receiver()
            .ok_or_else(|| String::from("couldn't move the Connection Receiver out of the Connection Provider"))?;

        tokio::spawn(async move {
            /*while*/ if let Some(connection) = connection_source.recv().await {
                let client_termination_receiver = client_termination_signaler.expect("BUG! client_termination_signaler is NONE").subscribe();
                let socket_communications_handler = SocketConnectionHandler::<CONFIG, _>::new(socket_dialog);
                let result = socket_communications_handler.client(connection,
                                                                                                     client_termination_receiver,
                                                                                                     connection_events_callback,
                                                                                                     dialog_processor_builder_fn).await
                    .map_err(|err| format!("Error while executing the dialog processor: {err}"));
                match result {
                    Ok(socket_connection) => {
                        if let Err(err) = returned_connection_sink.send(socket_connection).await {
                            warn!("`reactive-messaging::CompositeGenericSocketClient`: ERROR returning the connection (after the unresponsive & textual processor ended) @ {ip}:{port}: {err}");
                        }
                    }
                    Err(err) => {
                        error!("`reactive-messaging::CompositeGenericSocketClient`: ERROR in client (unresponsive & textual) @ {ip}:{port}: {err}");
                    }
                }
            }
        });
        self.spawned_processors_count += 1;
        Ok(connection_provider)
    }

    async fn start_multi_protocol<ConnectionEventsCallbackFuture:  Future<Output=()> + Send>
                                 (&mut self,
                                  initial_connection_state:       StateType,
                                  mut connection_routing_closure: impl FnMut(/*socket_connection: */&SocketConnection<StateType>, /*is_reused: */bool) -> Option<tokio::sync::mpsc::Sender<SocketConnection<StateType>>> + Send + 'static,
                                  connection_events_callback:     impl Fn(/*event: */ConnectionEvent<StateType>)                                       -> ConnectionEventsCallbackFuture                                 + Send + 'static)
                                 -> Result<(), Box<dyn Error + Sync + Send>> {

        let mut connection_manager = ClientConnectionManager::<CONFIG>::new(&self.ip, self.port);
        let connection = connection_manager.connect_retryable().await
            .map_err(|err| format!("Error making client connection to {}:{} -- {err}", self.ip, self.port))?;
        let mut just_opened_connection = Some(connection);

        let mut returned_connection_source = self.returned_connection_source.take()
            .ok_or_else(|| String::from("couldn't move the 'Returned Connection Source' out of the Connection Channel"))?;

        let ip = self.ip.clone();
        let port = self.port;

        // let shutdown_signaler = self.client_termination_signaler.as_ref().expect("BUG! client_termination_signaler is NONE").clone();

        // Spawns the "connection routing task" to:
        //   - Listen to newly incoming connections as well as upgraded/downgraded ones shared between processors
        //   - Gives them to the `protocol_stacking_closure`
        //   - Routes them to the right processor or close the connection
        tokio::spawn(async move {

            loop {
                let (mut socket_connection, sender) = match just_opened_connection.take() {
                    // process the just-opened connection (once)
                    Some(just_opened_connection) => {
                        let just_opened_socket_connection = SocketConnection::new(just_opened_connection, initial_connection_state.clone());
                        connection_events_callback(ConnectionEvent::Connected(&just_opened_socket_connection)).await;
                        let sender = connection_routing_closure(&just_opened_socket_connection, false);
                        (just_opened_socket_connection, sender)
                    },
                    // process connections returned by the processors (after they ended processing them)
                    None => {
                        let Some(returned_socket_connection) = returned_connection_source.recv().await else { break };
                        let sender = (!returned_socket_connection.closed())
                            .then_some(())
                            .and_then(|_| connection_routing_closure(&returned_socket_connection, true));
                        (returned_socket_connection, sender)
                    },
                };

                // route the connection to another processor or drop it
                match sender {
                    Some(sender) => {
                        trace!("`reactive-messaging::CompositeSocketClient`: ROUTING the connection with the server @ {ip}:{port} to another processor");
                        if let Err(err) = sender.send(socket_connection).await {
                            error!("`reactive-messaging::CompositeSocketClient`: BUG(?) in the client connected to the server @ {ip}:{port} while re-routing the connection: THE NEW (ROUTED) PROCESSOR CAN NO LONGER RECEIVE CONNECTIONS -- THE CONNECTION WILL BE DROPPED: {err}");
                            break
                        }
                    },
                    None => {
                        connection_events_callback(ConnectionEvent::Disconnected(&socket_connection)).await;
                        if let Err(err) = socket_connection.connection_mut().shutdown().await {
                            debug!("`reactive-messaging::CompositeSocketClient`: ERROR in the client connected to the server @ {ip}:{port} while shutting down the connection (after the processors ended): {err}");
                        }
                        break
                    },
                }
            }
            // loop ended
            trace!("`reactive-messaging::CompositeSocketClient`: The 'Connection Routing Task' for the client connected to the server @ {ip}:{port} ended -- hopefully, due to a graceful client termination.");
            // // guarantees this client is properly shutdown
            //_ = shutdown_signaler.send(())
        });
        Ok(())
    }

    fn termination_waiter(&mut self) -> Box<dyn FnOnce() -> BoxFuture<'static, Result<(), Box<dyn Error + Send + Sync>>>> {
        let mut local_termination_receiver = self.local_termination_is_complete_receiver.take();
        let mut latch = self.spawned_processors_count;
        Box::new(move || Box::pin({
            async move {
                if let Some(mut local_termination_receiver) = local_termination_receiver.take() {
                    while latch > 0 {
                        match local_termination_receiver.recv().await {
                            Some(()) => latch -= 1,
                            None     => return Err(Box::from(String::from("CompositeGenericSocketClient::termination_waiter(): It is no longer possible to tell when the client will be terminated: the broadcast channel was closed")))
                        }
                    }
                    Ok(())
                } else {
                    Err(Box::from("CompositeGenericSocketClient: \"wait for termination\" requested, but the client was not started (or a previous service termination was commanded) at the moment `termination_waiter()` was called"))
                }
            }
        }))
    }

    async fn terminate(mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        match self.client_termination_signaler.take() {
            Some(client_sender) => {
                warn!("`reactive-messaging::CompositeGenericSocketClient`: Shutdown asked & initiated for client connected @ {}:{}", self.ip, self.port);
                _ = client_sender.send(());
                Ok(())
            }
            None => {
                Err(Box::from("Shutdown requested, but the service was not started. Ignoring..."))
            }
        }
    }
}


/// Unit tests the [socket_server](self) module
#[cfg(any(test,doc))]
mod tests {
    use super::*;
    use crate::prelude::*;
    use std::{future, ops::Deref};
    use std::sync::atomic::Ordering::Relaxed;
    use std::time::Duration;
    use futures::StreamExt;
    use serde::{Deserialize, Serialize};
    use crate::{new_socket_server, start_server_processor};
    use crate::socket_connection::socket_dialog::textual_dialog::TextualDialog;

    const REMOTE_SERVER: &str = "66.45.249.218";


    /// Test that our instantiation macro is able to produce clients backed by all possible channel types
    #[cfg_attr(not(doc), test)]
    fn single_protocol_instantiation() {
        let _atomic_client = new_socket_client!(
            ConstConfig {
                ..ConstConfig::default()
            },
            REMOTE_SERVER, 443);

        let _fullsync_client = new_socket_client!(
            ConstConfig {
                ..ConstConfig::default()
            },
            REMOTE_SERVER, 443);

        let _crossbeam_client = new_socket_client!(
            ConstConfig {
                ..ConstConfig::default()
            },
            REMOTE_SERVER, 443);
    }

    /// Test that our instantiation macro is able to produce clients backed by all possible channel types
    #[cfg_attr(not(doc), test)]
    fn composite_protocol_instantiation() {
        let _atomic_client = new_composite_socket_client!(
            ConstConfig {
                ..ConstConfig::default()
            },
            REMOTE_SERVER, 443, () );

        let _fullsync_client = new_composite_socket_client!(
            ConstConfig {
                ..ConstConfig::default()
            },
            REMOTE_SERVER, 443, () );

        let _crossbeam_client = new_composite_socket_client!(
            ConstConfig {
                ..ConstConfig::default()
            },
            REMOTE_SERVER, 443, () );
    }

    /// Test that our client types are ready for usage
    /// (showcases the "single protocol" case)
    #[cfg_attr(not(doc),tokio::test)]
    async fn doc_usage() {

        const TEST_CONFIG: ConstConfig = ConstConfig::default();

        // demonstrates how to build an unresponsive client
        ///////////////////////////////////////////////////
        // using fully typed generic functions that will work with all possible configs
        let mut client = new_socket_client!(
            TEST_CONFIG,
            REMOTE_SERVER,
            443);
        start_client_processor!(TEST_CONFIG, Textual, Atomic, client,
            DummyClientAndServerMessages,
            DummyClientAndServerMessages,
            connection_events_handler,
            unresponsive_processor
        ).expect("Error starting a single protocol client");
        async fn connection_events_handler<const CONFIG:  u64,
                                           LocalMessages: ReactiveMessagingConfig<LocalMessages>                                      + Send + Sync + PartialEq + Debug,
                                           SenderChannel: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync>
                                          (_event: ProtocolEvent<CONFIG, LocalMessages, SenderChannel>) {
        }
        fn unresponsive_processor<const CONFIG:   u64,
                                  LocalMessages:  ReactiveMessagingConfig<LocalMessages>                                      + Send + Sync + PartialEq + Debug,
                                  SenderChannel:  FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync,
                                  StreamItemType: Deref<Target=DummyClientAndServerMessages>>
                                 (_client_addr:           String,
                                  _connected_port:        u16,
                                  _peer:                  Arc<Peer<CONFIG, LocalMessages, SenderChannel>>,
                                  client_messages_stream: impl Stream<Item=StreamItemType>)
                                 -> impl Stream<Item=()> {
            client_messages_stream.map(|_payload| ())
        }
        let wait_for_termination = client.termination_waiter();
        client.terminate().await.expect("Error on client Termination command");
        wait_for_termination().await.expect("Error waiting for client Termination");
        warn!("1st DONE");

        // demonstrates how to build a responsive client
        ////////////////////////////////////////////////
        // using fully typed generic functions that will work with all possible configs
        let mut client = new_socket_client!(
            TEST_CONFIG,
            REMOTE_SERVER,
            443);
        start_client_processor!(TEST_CONFIG, Textual, Atomic, client,
            DummyClientAndServerMessages,
            DummyClientAndServerMessages,
            connection_events_handler,
            responsive_processor
        ).expect("Error starting a single protocol client");
        fn responsive_processor<const CONFIG:   u64,
                                SenderChannel:  FullDuplexUniChannel<ItemType=DummyClientAndServerMessages, DerivedItemType=DummyClientAndServerMessages> + Send + Sync,
                                StreamItemType: Deref<Target=DummyClientAndServerMessages>>
                               (_client_addr:           String,
                                _connected_port:        u16,
                                peer:                   Arc<Peer<CONFIG, DummyClientAndServerMessages, SenderChannel>>,
                                client_messages_stream: impl Stream<Item=StreamItemType>)
                               -> impl Stream<Item=()> {
            client_messages_stream
                .map(|_payload| DummyClientAndServerMessages::FloodPing)
                .to_responsive_stream(peer, |_, _| ())
        }
        let wait_for_termination = client.termination_waiter();
        client.terminate().await.expect("Error on client Termination command");
        wait_for_termination().await.expect("Error waiting for client Termination");
        warn!("2nd DONE");

        // demonstrates how to use it with closures -- also allowing for any channel in the configs
        ///////////////////////////////////////////////////////////////////////////////////////////
        let mut client = new_socket_client!(
            TEST_CONFIG,
            REMOTE_SERVER,
            443);
        start_client_processor!(TEST_CONFIG, Textual, Atomic, client,
            DummyClientAndServerMessages,
            DummyClientAndServerMessages,
            |_| future::ready(()),
            |_, _, _, client_messages_stream| client_messages_stream.map(|_payload| DummyClientAndServerMessages::FloodPing)
        ).expect("Error starting a single protocol client");
        let wait_for_termination = client.termination_waiter();
        client.terminate().await.expect("Error on client Termination command");
        wait_for_termination().await.expect("Error waiting for client Termination");
        warn!("3rd DONE");

        // demonstrates how to use the concrete type
        ////////////////////////////////////////////
        // notice there may be a discrepancy in the `ConstConfig` you provide and the actual concrete types
        // you also provide for `UniProcessor` and `SenderChannel` -- therefore, this usage is not recommended
        // (but it is here anyway since it may bring, theoretically, an infinitesimal performance benefit)
        const CUSTOM_CONFIG: ConstConfig = ConstConfig {
            receiver_channel_size: 2048,
            sender_channel_size:   1024,
            executor_instruments:  reactive_mutiny::prelude::Instruments::LogsWithExpensiveMetrics,
            ..ConstConfig::default()
        };
        let mut client = CompositeSocketClient :: <{CUSTOM_CONFIG.into()},
                                                                          () >
                                                                      :: new(REMOTE_SERVER,443);
        type ProcessorUniType = UniZeroCopyFullSync<DummyClientAndServerMessages, {CUSTOM_CONFIG.receiver_channel_size as usize}, 1, {CUSTOM_CONFIG.executor_instruments.into()}>;
        type SenderChannelType = ChannelUniMoveFullSync<DummyClientAndServerMessages, {CUSTOM_CONFIG.sender_channel_size as usize}, 1>;
        let socket_dialog_handler = TextualDialog::<{CUSTOM_CONFIG.into()}, DummyClientAndServerMessages, DummyClientAndServerMessages, ReactiveMessagingRonSerializer, ReactiveMessagingRonDeserializer, ProcessorUniType, SenderChannelType, ()>::default();
        let connection_channel = client.spawn_processor::<DummyClientAndServerMessages,
                                                                                    DummyClientAndServerMessages,
                                                                                    ProcessorUniType,
                                                                                    SenderChannelType,
                                                                                    _, _, _, _, _, _> (
            socket_dialog_handler,
            |_| future::ready(()),
            |_, _, _, client_messages_stream| client_messages_stream.map(|_payload| DummyClientAndServerMessages::FloodPing)
        ).await.expect("Error spawning a protocol processor");
        client.start_single_protocol(connection_channel).await.expect("Error starting a single protocol client");
        let wait_for_termination = client.termination_waiter();
        client.terminate().await.expect("Error on client Termination command");
        wait_for_termination().await.expect("Error waiting for client Termination");
        warn!("4th DONE");
    }

    /// Ensures the termination of a client works according to the specification.\
    /// The following clients and servers will only exchange a message when
    /// they want the other party to disconnect.
    #[cfg_attr(not(doc),tokio::test(flavor = "multi_thread"))]
    async fn termination_process() {
        const IP: &str = "127.0.0.1";
        const PORT: u16 = 8030;
        const TEST_CONFIG: ConstConfig = ConstConfig::default();

        // CASE 1: locally initiated termination -- client still being active
        let connected_to_client = Arc::new(AtomicBool::new(false));
        let connected_to_client_ref = Arc::clone(&connected_to_client);
        let mut server = new_socket_server!(TEST_CONFIG, IP, PORT);
        start_server_processor!(TEST_CONFIG, Textual, Atomic, server, String, String,
            move |event| {
                let connected_to_client_ref = Arc::clone(&connected_to_client_ref);
                async move {
                    match event {
                        ProtocolEvent::PeerArrived { .. } => {
                            connected_to_client_ref.store(true, Relaxed);
                        },
                        ProtocolEvent::PeerLeft { .. } => {},
                        ProtocolEvent::LocalServiceTermination => {},
                    }
                }
            },
            |_, _, _, stream| stream
        ).expect("Error starting the server");
        let mut client = new_socket_client!(TEST_CONFIG, IP, PORT);
        start_client_processor!(TEST_CONFIG, Textual, Atomic, client, String, String,
            |_| future::ready(()),
            |_, _, _, stream| stream
        ).expect("Error starting the client");
        let termination_waiter = client.termination_waiter();
        // sleep a little for the connection to be established.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(connected_to_client.load(Relaxed), "Client didn't connect to server");
        assert!(client.is_connected(), "`client` didn't report any connection");
        client.terminate().await.expect("Could not terminate the client");
        _ = tokio::time::timeout(Duration::from_millis(100), termination_waiter()).await
            .expect("Timed out (>100ms) waiting the the client's termination");
        server.terminate().await.expect("Could not terminate the server");

        // CASE 2: automatic termination after disconnection
        let connected_to_client = Arc::new(AtomicBool::new(false));
        let connected_to_client_ref = Arc::clone(&connected_to_client);
        let mut server = new_socket_server!(TEST_CONFIG, IP, PORT);
        start_server_processor!(TEST_CONFIG, Textual, Atomic, server, String, String,
            move |event| {
                let connected_to_client_ref = Arc::clone(&connected_to_client_ref);
                async move {
                    match event {
                        ProtocolEvent::PeerArrived { peer } => {
                            connected_to_client_ref.store(true, Relaxed);
                            peer.send(String::from("Goodbye")).expect("Couldn't send");
                        },
                        ProtocolEvent::PeerLeft { .. } => {},
                        ProtocolEvent::LocalServiceTermination => {},
                    }
                }
            },
            |_, _, _, stream| stream
        ).expect("Error starting the server");
        let mut client = new_socket_client!(TEST_CONFIG, IP, PORT);
        start_client_processor!(TEST_CONFIG, Textual, Atomic, client, String, String,
            |_| future::ready(()),
            |_, _, peer, stream| stream.map(move |_msg| peer.cancel_and_close())     // close the connection when any message arrives
        ).expect("Error starting the client");
        let termination_waiter = client.termination_waiter();
        // sleep a little for the communications to go on.
        // After this, the server should have disconnected the client and calling `termination_waiter()` should return immediately
        // (without the need to call `client.terminate()`)
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(connected_to_client.load(Relaxed), "Client didn't connect to server");
        _ = tokio::time::timeout(Duration::from_millis(1), termination_waiter()).await
            .expect("A disconnected client should signal its `termination_waiter()` for an immediate return -- what didn't happen");
        assert!(!client.is_connected(), "`client` didn't report the disconnection");
        server.terminate().await.expect("Could not terminate the server");

    }

    /// assures the "Composite Protocol Stacking" pattern is supported & correctly implemented:
    ///   1) Just-opened client connections are always handled by the first processor
    ///   2) Connections can be routed freely among processors
    ///   3) "Last States" are taken into account, enabling the "connection routing closure"
    ///   4) Connections can be closed after the last processor are through with them
    /// -- for these, the client will send a message whenever it enters a state -- then the server will say "OK"
    ///    and the client will proceed to the next state, until the last one -- which closes the connection.
    #[cfg_attr(not(doc),tokio::test(flavor = "multi_thread"))]
    async fn composite_protocol_stacking_pattern() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

        const IP: &str = "127.0.0.1";
        const PORT: u16 = 8031;
        const TEST_CONFIG: ConstConfig = ConstConfig::default();

        // start the server that will only listen to messages until it is disconnected
        let mut server = new_socket_server!(
            TEST_CONFIG,
            IP,
            PORT);
        start_server_processor!(TEST_CONFIG, Textual, Atomic, server, String, String,
            |_| future::ready(()),
            move |_, _, peer, client_messages| client_messages
                .map(|msg| {
                    println!("SERVER RECEIVED: {msg} -- answering with 'OK'");
                    String::from("OK")
                })
                .to_responsive_stream(peer, |_, _| ())

        )?;
        let server_termination_waiter = server.termination_waiter();

        let mut client = new_composite_socket_client!(
            TEST_CONFIG,
            IP,
            PORT,
            Protocols );

        #[derive(Debug,PartialEq,Clone)]
        enum Protocols {
            Handshake,
            WelcomeAuthenticatedFriend,
            AccountSettings,
            GoodbyeOptions,
            Disconnect,
        }

        // first level processors shouldn't do anything until the client says something meaningful -- newcomers must know, a priori, who they are talking to (a security measure)
        let handshake_processor_greeted = Arc::new(AtomicBool::new(false));
        let handshake_processor_greeted_ref = Arc::clone(&handshake_processor_greeted);
        let handshake_processor = spawn_client_processor!(TEST_CONFIG, Textual, Atomic, client, String, String,
            |connection_event| async {
                if let ProtocolEvent::PeerArrived { peer  } = connection_event {
                    peer.send_async(String::from("Client is at `Handshake`")).await
                        .expect("Sending failed");
                }
            },
            move |_, _, peer, server_messages_stream| {
                assert_eq!(peer.try_take_state(), Some(Some(Protocols::Handshake)), "Connection is in a wrong state");
                let handshake_processor_greeted_ref = Arc::clone(&handshake_processor_greeted_ref);
                server_messages_stream.then(move |_payload| {
                    let peer = Arc::clone(&peer);
                    handshake_processor_greeted_ref.store(true, Relaxed);
                    async move {
                        peer.set_state(Protocols::WelcomeAuthenticatedFriend).await;
                        peer.flush_and_close(Duration::from_secs(1)).await;
                    }
                })
            }
        )?;

        // deeper processors should inform the server that they are now subjected to a new processor / protocol, so they may adjust accordingly
        let welcome_authenticated_friend_processor_greeted = Arc::new(AtomicBool::new(false));
        let welcome_authenticated_friend_processor_greeted_ref = Arc::clone(&welcome_authenticated_friend_processor_greeted);
        let welcome_authenticated_friend_processor = spawn_client_processor!(TEST_CONFIG, Textual, Atomic, client, String, String,
            |connection_event| async {
                if let ProtocolEvent::PeerArrived { peer  } = connection_event {
                    peer.send_async(String::from("Client is at `WelcomeAuthenticatedFriend`")).await
                        .expect("Sending failed");
                }
            },
            move |_, _, peer, server_messages_stream| {
                assert_eq!(peer.try_take_state(), Some(Some(Protocols::WelcomeAuthenticatedFriend)), "Connection is in a wrong state");
                let welcome_authenticated_friend_processor_greeted_ref = Arc::clone(&welcome_authenticated_friend_processor_greeted_ref);
                server_messages_stream.then(move |_payload| {
                    let peer = Arc::clone(&peer);
                    welcome_authenticated_friend_processor_greeted_ref.store(true, Relaxed);
                    async move {
                        peer.set_state(Protocols::AccountSettings).await;
                        peer.flush_and_close(Duration::from_secs(1)).await;
                    }
                })
            }
        )?;

        let account_settings_processor_greeted = Arc::new(AtomicBool::new(false));
        let account_settings_processor_greeted_ref = Arc::clone(&account_settings_processor_greeted);
        let account_settings_processor = spawn_client_processor!(TEST_CONFIG, Textual, Atomic, client, String, String,
            |connection_event| async {
                if let ProtocolEvent::PeerArrived { peer  } = connection_event {
                    peer.send_async(String::from("Client is at `AccountSettings`")).await
                        .expect("Sending failed");
                }
            },
            move |_, _, peer, server_messages_stream| {
                assert_eq!(peer.try_take_state(), Some(Some(Protocols::AccountSettings)), "Connection is in a wrong state");
                let account_settings_processor_greeted_ref = Arc::clone(&account_settings_processor_greeted_ref);
                server_messages_stream.then(move |_payload| {
                    let peer = Arc::clone(&peer);
                    account_settings_processor_greeted_ref.store(true, Relaxed);
                    async move {
                        peer.set_state(Protocols::GoodbyeOptions).await;
                        peer.flush_and_close(Duration::from_secs(1)).await;
                    }
                })
            }
        )?;

        let goodbye_options_processor_greeted = Arc::new(AtomicBool::new(false));
        let goodbye_options_processor_greeted_ref = Arc::clone(&goodbye_options_processor_greeted);
        let goodbye_options_processor = spawn_client_processor!(TEST_CONFIG, Textual, Atomic, client, String, String,
            |connection_event| async {
                if let ProtocolEvent::PeerArrived { peer  } = connection_event {
                    peer.send_async(String::from("Client is at `GoodbyeOptions`")).await
                        .expect("Sending failed");
                }
            },
            move |_, _, peer, server_messages_stream| {
                assert_eq!(peer.try_take_state(), Some(Some(Protocols::GoodbyeOptions)), "Connection is in a wrong state");
                let goodbye_options_processor_greeted_ref = Arc::clone(&goodbye_options_processor_greeted_ref);
                server_messages_stream.then(move |_payload| {
                    let peer = Arc::clone(&peer);
                    goodbye_options_processor_greeted_ref.store(true, Relaxed);
                    async move {
                        peer.set_state(Protocols::Disconnect).await;
                        peer.flush_and_close(Duration::from_secs(1)).await;
                    }
                })
            }
        )?;

        // this closure will route the connections based on the states the processors above had set
        // (it will be called whenever a protocol processor ends -- "returning" the connection)
        let connection_routing_closure = move |socket_connection: &SocketConnection<Protocols>, _|
            match socket_connection.state() {
                Protocols::Handshake                  => Some(handshake_processor.clone_sender()),
                Protocols::WelcomeAuthenticatedFriend => Some(welcome_authenticated_friend_processor.clone_sender()),
                Protocols::AccountSettings            => Some(account_settings_processor.clone_sender()),
                Protocols::GoodbyeOptions             => Some(goodbye_options_processor.clone_sender()),
                Protocols::Disconnect                 => None,
            };
        client.start_multi_protocol(Protocols::Handshake, connection_routing_closure, |_| future::ready(())).await?;

        let client_waiter = client.termination_waiter();
        // wait for the client to do its stuff
        _ = tokio::time::timeout(Duration::from_secs(5), client_waiter()).await
            .expect("TIMED OUT (>5s) Waiting for the server & client to do their stuff & disconnect the client");

        // terminate the server & wait until the shutdown process is complete
        server.terminate().await?;
        server_termination_waiter().await?;

        assert!(handshake_processor_greeted.load(Relaxed),                    "`Handshake` processor wasn't requested");
        assert!(welcome_authenticated_friend_processor_greeted.load(Relaxed), "`WelcomeAuthenticatedFriend` processor wasn't requested");
        assert!(account_settings_processor_greeted.load(Relaxed),             "`AccountSettings` processor wasn't requested");
        assert!(goodbye_options_processor_greeted.load(Relaxed),              "`GoodbyeOptions` processor wasn't requested");

        Ok(())
    }


    #[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
    enum DummyClientAndServerMessages {
        #[default]
        FloodPing,
    }

    impl ReactiveMessagingConfig<DummyClientAndServerMessages> for DummyClientAndServerMessages {}

    impl Deref for DummyClientAndServerMessages {
        type Target = DummyClientAndServerMessages;
        fn deref(&self) -> &Self::Target {
            self
        }
    }

}