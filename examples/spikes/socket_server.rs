//! Models what the server API should be like
//! so to allow us full control of the [Uni] and [Channel]s being used.

use std::{marker::PhantomData, sync::Arc};

use crate::uni::{GenericUni, ConcreteIterator, channel::{ChannelAtomic, ChannelFullSync}, Uni};

// All possible Channels & Uni variations for our server
////////////////////////////////////////////////////////

pub type AtomicSocketServer<const CONFIG: usize, RemoteMessages, LocalMessages /* BUFFER SIZE, etc */>
    = SocketServer<CONFIG,
                   RemoteMessages,
                   LocalMessages,
                   Uni<RemoteMessages, ChannelAtomic<RemoteMessages>>,
                   ChannelAtomic<LocalMessages>>;

pub type FullSyncSocketServer<const CONFIG: usize, RemoteMessages, LocalMessages /* BUFFER SIZE, etc */>
    = SocketServer<CONFIG,
                   RemoteMessages,
                   LocalMessages,
                   Uni<RemoteMessages, ChannelFullSync<RemoteMessages>>,
                   ChannelFullSync<LocalMessages>>;

pub struct SocketServer<const CONFIG: usize,
                        RemoteMessages,
                        LocalMessages,
                        ProcessorUniType: GenericUni,
                        SenderChannelType> {
    pub _phantom: PhantomData<(RemoteMessages,LocalMessages,ProcessorUniType,SenderChannelType)>
}

/// Helps to infer some types:
/// ```nocompile
///     type THE_TYPE_I_WANT = <SocketServer<...> as GenericSocketServer>::THE_TYPE_YOU_WANT
pub trait GenericSocketServer {
    const CONFIG: usize;
    type RemoteMessages;
    type LocalMessages;
    type ProcessorUniType: GenericUni;
    type SenderChannelType;
    type ConnectionEventType;
    type StreamItemType;
    type StreamType;

    fn start<LocalMessagesIteratorType:  Iterator<Item=Self::LocalMessages>>
            (self,
             connection_events_handler: impl FnOnce(Self::SenderChannelType),
             processor:                 impl FnOnce(ConcreteIterator<<Self::ProcessorUniType as GenericUni>::DerivedType>) -> LocalMessagesIteratorType)
            -> Arc<Self>;
}
impl<const CONFIG: usize,
     RemoteMessages,
     LocalMessages,
     ProcessorUniType: GenericUni,
     SenderChannelType>
GenericSocketServer for
SocketServer<CONFIG,
             RemoteMessages,
             LocalMessages,
             ProcessorUniType,
             SenderChannelType> {

    const CONFIG: usize      = CONFIG;
    type RemoteMessages      = RemoteMessages;
    type LocalMessages       = LocalMessages;
    type ProcessorUniType    = ProcessorUniType;
    type SenderChannelType   = SenderChannelType;
    type ConnectionEventType = SenderChannelType;
    type StreamItemType      = ProcessorUniType::DerivedType;
    type StreamType          = ConcreteIterator<ProcessorUniType::DerivedType>;

    /// Starts the server, returning an `Arc<Self>` so it may still be shutdown
    fn start<LocalMessagesIteratorType:  Iterator<Item=LocalMessages>>
            (self,
             _connection_events_handler: impl FnOnce(SenderChannelType),
             _processor:                 impl FnOnce(ConcreteIterator<ProcessorUniType::DerivedType>) -> LocalMessagesIteratorType)
            -> Arc<Self> {
        Arc::new(self)
    }
}
