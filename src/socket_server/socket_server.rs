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
use crate::{ReactiveMessagingDeserializer, ReactiveMessagingSerializer};
use crate::socket_connection::common::{ReactiveMessagingSender, RetryableSender};
use crate::config::{Channels, ConstConfig};
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




#[macro_export]
macro_rules! new_socket_server {
    ($const_config:    expr,
     $interface_ip:    expr,
     $port:            expr,
     $remote_messages: ty,
     $local_messages:  ty) => {{
        use crate::socket_server::SocketServer;
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
                                                                           UniZeroCopyAtomic<$remote_messages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                                                           ReactiveMessagingSender<CONFIG, $local_messages, ChannelUniMoveAtomic<$local_messages, SENDER_BUFFER, 1>> >
                                                                        ::new($interface_ip, $port) ),
            Channels::FullSync => SocketServer::<CONFIG,
                                                 $remote_messages,
                                                 $local_messages,
                                                 PROCESSOR_BUFFER,
                                                 PROCESSOR_UNI_INSTRUMENTS,
                                                 SENDER_BUFFER>
                                              ::FullSync(GenericSocketServer::<CONFIG,
                                                                               $remote_messages,
                                                                               $local_messages,
                                                                               UniZeroCopyFullSync<$remote_messages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                                                               ReactiveMessagingSender<CONFIG, $local_messages, ChannelUniMoveFullSync<$local_messages, SENDER_BUFFER, 1>> >
                                                                            ::new($interface_ip, $port) ),
            Channels::Crossbeam => SocketServer::<CONFIG,
                                                  $remote_messages,
                                                  $local_messages,
                                                  PROCESSOR_BUFFER,
                                                  PROCESSOR_UNI_INSTRUMENTS,
                                                  SENDER_BUFFER>
                                               ::Crossbeam(GenericSocketServer::<CONFIG,
                                                                                 $remote_messages,
                                                                                 $local_messages,
                                                                                 UniMoveCrossbeam<$remote_messages, PROCESSOR_BUFFER, 1, PROCESSOR_UNI_INSTRUMENTS>,
                                                                                 ReactiveMessagingSender<CONFIG, $local_messages, ChannelUniMoveCrossbeam<$local_messages, SENDER_BUFFER, 1>> >
                                                                              ::new($interface_ip, $port) ),
        }
    }}
}
pub use new_socket_server;


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


pub struct GenericSocketServer<const CONFIG:        u64,
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

impl<const CONFIG:        u64,
     RemoteMessages:      ReactiveMessagingDeserializer<RemoteMessages> + Send + Sync + PartialEq + Debug + 'static,
     LocalMessages:       ReactiveMessagingSerializer<LocalMessages>    + Send + Sync + PartialEq + Debug + 'static,
     ProcessorUniType:    GenericUni<ItemType=RemoteMessages>           + Send + Sync                     + 'static,
     RetryableSenderImpl: RetryableSender<LocalMessages=LocalMessages>  + Send + Sync                     + 'static>
GenericSocketServer<CONFIG, RemoteMessages, LocalMessages, ProcessorUniType, RetryableSenderImpl> {

    /// Creates a new server instance listening on TCP/IP:
    ///   `interface_ip`:         the interface's IP to listen to -- 0.0.0.0 will cause listening to all network interfaces
    ///   `port`:                 what port to listen to
    pub fn new<IntoString: Into<String>>
              (interface_ip: IntoString,
               port: u16)
              -> Self {
        Self {
            interface_ip:             interface_ip.into(),
            port,
            server_shutdown_signaler: None,
            local_shutdown_receiver:  None,
            _phantom:                 PhantomData,
        }
    }

}


/// Unit tests the [socket_server](self) module
#[cfg(any(test,doc))]
mod tests {
    use super::*;

    /// Test that our instantiation macro is able to produce all types
    #[cfg_attr(not(doc),test)]
    fn instantiation() {
        let atomic_server = new_socket_server!(
            ConstConfig {
                channel: Channels::Atomic,
                ..ConstConfig::default()
            },
            "127.0.0.1", 8040, String, String);
        assert!(matches!(atomic_server, SocketServer::Atomic(_)), "an Atomic Server couldn't be instantiated");

        let fullsync_server  = new_socket_server!(
            ConstConfig {
                channel: Channels::Atomic,
                ..ConstConfig::default()
            },
            "127.0.0.1", 8041, String, String);
        assert!(matches!(fullsync_server, SocketServer::FullSync(_)), "a FullSync Server couldn't be instantiated");

        let crossbeam_server = new_socket_server!(
            ConstConfig {
                channel: Channels::Atomic,
                ..ConstConfig::default()
            },
            "127.0.0.1", 8042, String, String);
        assert!(matches!(crossbeam_server, SocketServer::Crossbeam(_)), "a Crossbeam Server couldn't be instantiated");
    }

}