//! Models what the server API should be like
//! so to allow us full control of the [Uni] and [Channel]s being used.

use std::{marker::PhantomData, sync::Arc};

use crate::uni::{GenericUni, MessagingMutinyStream, channel::{ChannelZeroCopy, ChannelMove}, Uni};

// All possible Channels & Uni variations for our server
////////////////////////////////////////////////////////

/// A zero-cost abstraction type allowing to use `reactive-mutiny` Zero-Copy channels
/// (incoming & outgoing messages are guaranteed to be zero-copied)
pub type ZeroCopySocketServer<const CONFIG:                    usize,
                              const PROCESSOR_UNI_INSTRUMENTS: usize,
                              const PROCESSOR_BUFFER_SIZE:     usize,
                              const SENDER_BUFFER_SIZE:        usize,
                              RemoteMessages,
                              LocalMessages /* BUFFER SIZE, etc */>
    = SocketServer<CONFIG,
                   PROCESSOR_UNI_INSTRUMENTS,
                   PROCESSOR_BUFFER_SIZE,
                   RemoteMessages,
                   LocalMessages,
                   Uni<PROCESSOR_UNI_INSTRUMENTS,
                       PROCESSOR_BUFFER_SIZE,
                       RemoteMessages,
                       ChannelZeroCopy<PROCESSOR_BUFFER_SIZE, RemoteMessages>>,
                   ChannelZeroCopy<SENDER_BUFFER_SIZE, LocalMessages>>;

/// A zero-cost abstraction type ready to use `reactive-mutiny' Movable channels
/// (incoming & outgoing messages might be copied, if the compiler can't optimize those operations)
pub type MovableSocketServer<const CONFIG:                    usize,
                             const PROCESSOR_UNI_INSTRUMENTS: usize,
                             const PROCESSOR_BUFFER_SIZE:     usize,
                             const SENDER_BUFFER_SIZE:        usize,
                             RemoteMessages,
                             LocalMessages /* BUFFER SIZE, etc */>
    = SocketServer<CONFIG,
                   PROCESSOR_UNI_INSTRUMENTS,
                   PROCESSOR_BUFFER_SIZE,
                   RemoteMessages,
                   LocalMessages,
                   Uni<PROCESSOR_UNI_INSTRUMENTS,
                       PROCESSOR_BUFFER_SIZE,
                       RemoteMessages,
                       ChannelMove<PROCESSOR_BUFFER_SIZE, RemoteMessages>>,
                   ChannelMove<SENDER_BUFFER_SIZE, LocalMessages>>;

pub struct SocketServer<const CONFIG: usize,
                        const PROCESSOR_UNI_INSTRUMENTS: usize,
                        const PROCESSOR_BUFFER_SIZE: usize,
                        RemoteMessages,
                        LocalMessages,
                        ProcessorUniType: GenericUni<PROCESSOR_UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE>,
                        SenderChannelType> {
    pub _phantom: PhantomData<(RemoteMessages,LocalMessages,ProcessorUniType,SenderChannelType)>
}

/// Helps to infer some types:
/// ```nocompile
///     type THE_TYPE_I_WANT = <SocketServer<...> as GenericSocketServer>::THE_TYPE_YOU_WANT
pub trait GenericSocketServer<const PROCESSOR_UNI_INSTRUMENTS: usize,
                              const PROCESSOR_BUFFER_SIZE: usize> {
    const PROCESSOR_UNI_INSTRUMENTS: usize;
    const CONFIG: usize;
    type RemoteMessages;
    type LocalMessages;
    type ProcessorUniType: GenericUni<PROCESSOR_UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE>;
    type SenderChannelType;
    type ConnectionEventType;
    type StreamItemType;
    type StreamType;

    fn start<LocalMessagesIteratorType:  Iterator<Item=Self::LocalMessages>>
            (self,
             connection_events_handler: impl FnMut(ConnectionEvents<Self::SenderChannelType>),
             processor:                 impl FnOnce(MessagingMutinyStream<PROCESSOR_UNI_INSTRUMENTS,
                                                                          PROCESSOR_BUFFER_SIZE,
                                                                          Self::ProcessorUniType>)
                                                   -> LocalMessagesIteratorType)
            -> Arc<Self>;
}
impl<const CONFIG: usize,
     const PROCESSOR_UNI_INSTRUMENTS: usize,
     const PROCESSOR_BUFFER_SIZE: usize,
     RemoteMessages,
     LocalMessages,
     ProcessorUniType: GenericUni<PROCESSOR_UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE>,
     SenderChannelType>
GenericSocketServer<PROCESSOR_UNI_INSTRUMENTS,
                    PROCESSOR_BUFFER_SIZE> for
SocketServer<CONFIG,
             PROCESSOR_UNI_INSTRUMENTS,
             PROCESSOR_BUFFER_SIZE,
             RemoteMessages,
             LocalMessages,
             ProcessorUniType,
             SenderChannelType> {

    const CONFIG: usize                    = CONFIG;
    const PROCESSOR_UNI_INSTRUMENTS: usize = PROCESSOR_UNI_INSTRUMENTS;
    type RemoteMessages                    = RemoteMessages;
    type LocalMessages                     = LocalMessages;
    type ProcessorUniType                  = ProcessorUniType;
    type SenderChannelType                 = SenderChannelType;
    type ConnectionEventType               = SenderChannelType;
    type StreamItemType                    = ProcessorUniType::DerivedItemType;
    type StreamType                        = MessagingMutinyStream<PROCESSOR_UNI_INSTRUMENTS,
                                                                   PROCESSOR_BUFFER_SIZE,
                                                                   ProcessorUniType>;

    /// Starts the server, returning an `Arc<Self>` so it may still be shutdown
    fn start<LocalMessagesIteratorType:  Iterator<Item=LocalMessages>>
            (self,
             _connection_events_handler: impl FnMut(ConnectionEvents<SenderChannelType>),
             _processor:                 impl FnOnce(MessagingMutinyStream<PROCESSOR_UNI_INSTRUMENTS,
                                                                           PROCESSOR_BUFFER_SIZE,
                                                                           ProcessorUniType>)
                                                    -> LocalMessagesIteratorType)
            -> Arc<Self> {
        Arc::new(self)
    }
}


/// The possible connection events
pub enum ConnectionEvents<SenderChannelType> {
    /// Happens when the client connects
    _Connected(SenderChannelType),
    /// Happens when the client disconnects
    _Disconnected(SenderChannelType),
    /// Happens when there was an error when reading/writing data
    _IOError(SenderChannelType),
}