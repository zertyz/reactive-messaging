//! Types used for reading and writing messages over socket connections

use std::fmt::Debug;
use std::future::Future;
use std::sync::Arc;
use reactive_mutiny::prelude::{FullDuplexUniChannel, GenericUni};
use crate::prelude::{Peer, SocketConnection};
use crate::serde::ReactiveMessagingConfig;
use crate::socket_connection::common::ReactiveMessagingUniSender;

pub trait SocketDialog<const CONFIG: u64>: Sync + Send + Default {

    type RemoteMessages:                                                                                           Send + Sync + PartialEq + Debug + 'static;
    type DeserializedRemoteMessages:                                                                               Send + Sync + PartialEq + Debug + 'static;
    type LocalMessages:  ReactiveMessagingConfig<Self::LocalMessages>                                            + Send + Sync + PartialEq + Debug + 'static;
    type ProcessorUni:   GenericUni<ItemType=Self::DeserializedRemoteMessages>                                   + Send + Sync                     + 'static;
    type SenderChannel:  FullDuplexUniChannel<ItemType=Self::LocalMessages, DerivedItemType=Self::LocalMessages> + Send + Sync                     + 'static;
    type State:                                                                                                    Send + Sync + Clone + Debug     + 'static;
    
    /// IMPLEMENTORS: #[inline(always)]
    fn dialog_loop(self,
                   socket_connection:     &mut SocketConnection<Self::State>,
                   peer:                  &Arc<Peer<CONFIG, Self::LocalMessages, Self::SenderChannel, Self::State>>,
                   processor_sender:      &ReactiveMessagingUniSender<CONFIG, Self::DeserializedRemoteMessages, <<Self as SocketDialog<CONFIG>>::ProcessorUni as GenericUni>::DerivedItemType, Self::ProcessorUni>)

                   -> impl Future < Output = Result<(), Box<dyn std::error::Error + Sync + Send>> >  + Send;
}