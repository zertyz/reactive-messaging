//! Provides shenanigans for creating servers with the given reactive processor logic -- that will receive and return `Stream`s.\
//! Two variants exist:
//!   * Responsive: the reactive processor's output `Stream` yields items that are messages to be sent back to each client;
//!   * Unresponsive the reactive processor's output `Stream` won't be sent to the clients, allowing them to return items from any type.
//!
//! The Unresponsive processors still allow you to send messages to the client and should be preferred (as far as performance is concerned)
//! for protocols where the count of messages IN don't nearly map 1/1 with the count of messages OUT.
//!
//! For every server variant, the `Stream` items may be a combination of fallible/non-fallible (using `Result<>` or not) & future/non-future.
//!
//! Use [SocketServer] for better usability or [GenericSocketServer] for greater flexibility.


use std::fmt::Debug;
use std::marker::PhantomData;
use reactive_mutiny::prelude::advanced::{ChannelUniMoveAtomic, ChannelUniMoveCrossbeam, ChannelUniMoveFullSync, UniMoveCrossbeam, UniZeroCopyAtomic, UniZeroCopyFullSync};
use crate::{ReactiveMessagingDeserializer, ReactiveMessagingSerializer};
use crate::socket_connection::common::ReactiveMessagingSender;



#[macro_export]
macro_rules! new_socket_server {
    ($const_config:    expr,
     $interface_ip:    expr,
     $port:            expr,
     $remote_messages: ty,
     $local_messages:  ty) => {{
        use crate::socket_server:SocketServer;
        const CONFIG:                    u64   = $const_config.into();
        const PROCESSOR_BUFFER:          usize = $const_config.receiver_buffer as usize;
        const PROCESSOR_UNI_INSTRUMENTS: usize = $const_config.executor_instruments.into();
        const SENDER_BUFFER:             usize = $const_config.sender_buffer   as usize;
        match $const_config.channel {
            Channels::Atomic => SocketServer::<CONFIG,
                                               $remote_messages,
                                               $local_messages,
                                               PROCESSOR_BUFFER,
                                               PROCESSOR_UNI_INSTRUMENTS,
                                               SENDER_BUFFER>
                                            ::Atomic(GenericSocketServer::<CONFIG,
                                                                           $remote_messages,
                                                                           $local_messages,
                                                                           UniZeroCopyAtomic<RemoteMessages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                                                           ReactiveMessagingSender<{CONFIG as usize}, LocalMessages, ChannelUniMoveAtomic<LocalMessages, SENDER_BUFFER, 1>> >
                                                                        ::new() ),
            Channels::FullSync => SocketServer::<CONFIG,
                                                 $remote_messages,
                                                 $local_messages,
                                                 PROCESSOR_BUFFER,
                                                 PROCESSOR_UNI_INSTRUMENTS,
                                                 SENDER_BUFFER>
                                              ::FullSync(GenericSocketServer::<CONFIG,
                                                                               $remote_messages,
                                                                               $local_messages,
                                                                               UniZeroCopyFullSync<RemoteMessages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                                                               ReactiveMessagingSender<{CONFIG as usize}, LocalMessages, ChannelUniMoveFullSync<LocalMessages, SENDER_BUFFER, 1>> >
                                                                            ::new() ),
            Channels::Crossbeam => SocketServer::<CONFIG,
                                                  $remote_messages,
                                                  $local_messages,
                                                  PROCESSOR_BUFFER,
                                                  PROCESSOR_UNI_INSTRUMENTS,
                                                  SENDER_BUFFER>
                                               ::Crossbeam(GenericSocketServer::<CONFIG,
                                                                                 $remote_messages,
                                                                                 $local_messages,
                                                                                 UniZeroCopyCrossbeam<RemoteMessages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                                                                 ReactiveMessagingSender<{CONFIG as usize}, LocalMessages, ChannelUniMoveCrossbeam<LocalMessages, SENDER_BUFFER, 1>> >
                                                                              ::new() ),
        }
    }}
}


pub enum SocketServer<const CONFIG:                    u64,
                      RemoteMessages:                  ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug           + 'static,
                      LocalMessages:                   ReactiveMessagingSerializer<LocalMessages>    + Send + Sync + PartialEq + Debug + Default + 'static,
                      const PROCESSOR_BUFFER:          usize,
                      const PROCESSOR_UNI_INSTRUMENTS: usize,
                      const SENDER_BUFFER:             usize> {

    Atomic(GenericSocketServer::<CONFIG,
                                 RemoteMessages,
                                 LocalMessages,
                                 UniZeroCopyAtomic<RemoteMessages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                 ReactiveMessagingSender<CONFIG, LocalMessages, ChannelUniMoveAtomic<LocalMessages, SENDER_BUFFER, 1>> >),

    FullSync(GenericSocketServer::<CONFIG,
                                   RemoteMessages,
                                   LocalMessages,
                                   UniZeroCopyFullSync<RemoteMessages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                   ReactiveMessagingSender<CONFIG, LocalMessages, ChannelUniMoveFullSync<LocalMessages, SENDER_BUFFER, 1>> >),

    Crossbeam(GenericSocketServer::<CONFIG,
                                    RemoteMessages,
                                    LocalMessages,
                                    UniMoveCrossbeam<RemoteMessages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                    ReactiveMessagingSender<CONFIG, LocalMessages, ChannelUniMoveCrossbeam<LocalMessages, SENDER_BUFFER, 1>> >),
    BadConfig {
        config:   u64,
        _phantom: PhantomData<(RemoteMessages, LocalMessages)>
    },
}


// impl SocketServer {
//
// }


pub struct GenericSocketServer<const CONFIG: u64,
                               RemoteMessages:                  ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug           + 'static,
                               LocalMessages:                   ReactiveMessagingSerializer<LocalMessages>    + Send + Sync + PartialEq + Debug + Default + 'static,
                               Uni,
                               Sender> {

    _phantom: PhantomData<(RemoteMessages, LocalMessages, Uni, Sender)>
}