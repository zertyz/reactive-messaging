//! Resting place for [Peer], representing the remote party on a TCP/IP socket connection

use crate::{
    socket_connection::common::{
        ReactiveMessagingSender,
    },
};
use std::{
    fmt::{Debug, Formatter},
    net::SocketAddr,
    time::Duration,
};
use reactive_mutiny::prelude::advanced::{
    MutinyStream,
    FullDuplexUniChannel,
};
use tokio::sync::Mutex;
use crate::config::ConstConfig;
use crate::socket_connection::connection::{ConnectionId, SocketConnection};


/// Represents a reactive channel connected to a remote peer, through which we're able to send out "local messages" of type `RetryableSenderImpl::LocalMessages`.\
/// the [Self::send()] method honors whatever retrying config is specified in [RetryableSenderImpl::CONST_CONFIG].
/// IMPLEMENTATION NOTE: GAT traits (to reduce the number of generic parameters) couldn't be used here -- even after applying this compiler bug workaround https://github.com/rust-lang/rust/issues/102211#issuecomment-1513931928
///                      -- the "error: implementation of `std::marker::Send` is not general enough" bug kept on popping up in user provided closures that called other async functions.
pub struct Peer<const CONFIG:     u64,
                LocalMessages:                                                                                  Send + Sync + PartialEq + Debug + 'static,
                SenderChannel:    FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync,
                StateType:                                                                                      Send + Sync + Clone     + Debug + 'static = ()> {
    pub peer_id:          ConnectionId,
    pub peer_address:     SocketAddr,
    pub state:            Mutex<Option<StateType>>,
        retryable_sender: ReactiveMessagingSender<CONFIG, LocalMessages, SenderChannel>,
}

impl<const CONFIG:  u64,
     LocalMessages:                                                                               Send + Sync + PartialEq + Debug,
     SenderChannel: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync,
     StateType:                                                                                   Send + Sync + Clone     + Debug>
Peer<CONFIG, LocalMessages, SenderChannel, StateType> {

    pub fn new(retryable_sender: ReactiveMessagingSender<CONFIG, LocalMessages, SenderChannel>, peer_address: SocketAddr, connection: &SocketConnection<StateType>) -> Self {
        Self {
            peer_id: connection.id(),
            peer_address,
            state: Mutex::new(Some(connection.state().clone())),
            retryable_sender,
        }
    }

    pub fn config(&self) -> ConstConfig {
        ConstConfig::from(CONFIG)
    }

    /// Asks the underlying channel to revert to Stream-mode (rather than Execution-mode), returning the `Stream`
    pub fn create_stream(&self) -> (MutinyStream<'static, LocalMessages, SenderChannel, LocalMessages>, u32) {
        self.retryable_sender.create_stream()
    }


    #[inline(always)]
    pub fn send(&self,
                message: LocalMessages)
               -> Result<(), (/*abort_the_connection?*/bool, /*error_message: */String)> {
        self.retryable_sender.send(message)
    }

    #[inline(always)]
    pub async fn send_async(&self,
                            message: LocalMessages)
                           -> Result<(), (/*abort_the_connection?*/bool, /*error_message: */String)> {

        self.retryable_sender.send_async_trait(message).await
    }

    /// Sets this object to a user-provided state, to facilitate communications between protocol processors
    /// (a requirement to allow the "Composite Protocol Stacking" pattern).\
    /// See also [Self::take_state()]
    pub async fn set_state(&self, state: StateType) {
        *self.state.lock().await = Some(state);
    }

    /// Use [set_state()] (async) if possible
    pub fn try_set_state(&self, state: StateType) -> bool {
        if let Ok(mut locked_state) = self.state.try_lock() {
            *locked_state = Some(state);
            true
        } else {
            false
        }
    }

    /// "Takes" this object's user-provided state, previously set by [Self::set_state()] -- used to facilitate communications between protocol processors
    /// (a requirement to allow the "Composite Protocol Stacking" pattern)
    pub async fn take_state(&self) -> Option<StateType> {
        self.state.lock().await.take()
    }

    /// Use [Self::take_state()] (async) if possible
    pub fn try_take_state(&self) -> Option<Option<StateType>> {
        if let Ok(mut locked_state) = self.state.try_lock() {
            Some(locked_state.take())
        } else {
            None
        }
    }

    #[inline(always)]
    pub fn pending_items_count(&self) -> u32 {
        self.retryable_sender.pending_items_count()
    }

    #[inline(always)]
    pub fn buffer_size(&self) -> u32 {
        self.retryable_sender.buffer_size()
    }

    pub async fn flush_and_close(&self, timeout: Duration) -> u32 {
        self.retryable_sender.flush_and_close(timeout).await
    }

    pub fn cancel_and_close(&self) {
        self.retryable_sender.cancel_and_close();
    }
}


impl<const CONFIG:  u64,
     LocalMessages:                                                                               Send + Sync + PartialEq + Debug,
     SenderChannel: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync,
     StateType:                                                                                   Send + Sync + Clone     + Debug>
Debug for
Peer<CONFIG, LocalMessages, SenderChannel, StateType> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Peer {{peer_id: {}, peer_address: '{}', state: '{:?}', sender: {}/{} pending messages}}",
               self.peer_id, self.peer_address, self.state.try_lock(), self.retryable_sender.pending_items_count(), self.retryable_sender.buffer_size())
    }
}