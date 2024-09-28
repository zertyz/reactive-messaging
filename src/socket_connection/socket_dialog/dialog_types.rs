//! Types used for reading and writing messages over socket connections

use std::fmt::Debug;
use std::future::Future;
use std::ops::RangeInclusive;
use std::sync::Arc;
use reactive_mutiny::prelude::{FullDuplexUniChannel, GenericUni};
use crate::prelude::{Peer, SocketConnection};
use crate::socket_connection::common::ReactiveMessagingUniSender;

pub trait SocketDialog<const CONFIG: u64>: Default {

    type RemoteMessagesConstrainedType: Send + Sync + PartialEq + Debug + 'static;
    type LocalMessagesConstrainedType:  Send + Sync + PartialEq + Debug + 'static;
    
    /// IMPLEMENTORS: #[inline(always)]
    fn dialog_loop<ProcessorUniType:   GenericUni<ItemType=Self::RemoteMessagesConstrainedType>                                                              + Send + Sync                     + 'static,
                   SenderChannel:      FullDuplexUniChannel<ItemType=Self::LocalMessagesConstrainedType, DerivedItemType=Self::LocalMessagesConstrainedType> + Send + Sync                     + 'static,
                   StateType:                                                                                                                                  Send + Sync + Clone + Debug     + 'static,
                  >
                  (self,
                   socket_connection:     &mut SocketConnection<StateType>,
                   peer:                  &Arc<Peer<CONFIG, Self::LocalMessagesConstrainedType, SenderChannel, StateType>>,
                   processor_sender:      &ReactiveMessagingUniSender<CONFIG, Self::RemoteMessagesConstrainedType, ProcessorUniType::DerivedItemType, ProcessorUniType>,
                   payload_size_range:    RangeInclusive<u32>)

                  -> impl Future < Output = Result<(), Box<dyn std::error::Error + Sync + Send>> >  + Send;
}