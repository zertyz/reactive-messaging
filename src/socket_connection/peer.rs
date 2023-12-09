//! Resting place for [Peer], representing the remote part on a TCP/IP socket connection

use crate::{
    ReactiveMessagingSerializer,
    socket_connection::common::{
        ReactiveMessagingSender,
    },
};
use std::{
    fmt::{Debug, Formatter},
    sync::atomic::{AtomicU32, Ordering::Relaxed},
    net::SocketAddr,
    time::Duration,
};
use reactive_mutiny::prelude::advanced::{
    MutinyStream,
    FullDuplexUniChannel,
};
use tokio::sync::Mutex;


static PEER_COUNTER: AtomicU32 = AtomicU32::new(0);
pub type PeerId = u32;



/// Represents a reactive channel connected to a remote peer, through which we're able to send out "local messages" of type `RetryableSenderImpl::LocalMessages`.\
/// the [Self::send()] method honors whatever retrying config is specified in [RetryableSenderImpl::CONST_CONFIG].
/// IMPLEMENTATION NOTE: GAT traits (to reduce the number of generic parameters) couldn't be used here -- even after applying this compiler bug workaround https://github.com/rust-lang/rust/issues/102211#issuecomment-1513931928
///                      -- the "error: implementation of `std::marker::Send` is not general enough" bug kept on popping up in user provided closures that called other async functions.
pub struct Peer<const CONFIG:     u64,
                LocalMessages:    ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
                SenderChannel:    FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync,
                StateType:                                                                                      Send + Sync                     + 'static = ()> {
    pub peer_id:          PeerId,
    pub peer_address:     SocketAddr,
    pub state:            Mutex<Option<StateType>>,
        retryable_sender: ReactiveMessagingSender<CONFIG, LocalMessages, SenderChannel>,
}

impl<const CONFIG:  u64,
     LocalMessages: ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
     SenderChannel: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync,
     StateType:                                                                                   Send + Sync>
Peer<CONFIG, LocalMessages, SenderChannel, StateType> {

    pub fn new(retryable_sender: ReactiveMessagingSender<CONFIG, LocalMessages, SenderChannel>, peer_address: SocketAddr, initial_state: Option<StateType>) -> Self {
        Self {
            peer_id: PEER_COUNTER.fetch_add(1, Relaxed),
            peer_address,
            state: Mutex::new(initial_state),
            retryable_sender,
        }
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

    /// Sets this object to a user-provided state, to facilitate communications between producers and senders of messages
    /// (a requirement to allow the "Composite Protocol Stacking" pattern).\
    /// See also [Self::take_state()]
    pub async fn set_state(&self, state: Option<StateType>) {
        *self.state.lock().await = state;
    }

    /// "Takes" this object's user-provided state, previously set by [Self::set_state()] -- used to facilitate communications between producers and senders
    /// (a requirement to allow the "Composite Protocol Stacking" pattern)
    pub async fn take_state(&self) -> Option<StateType> {
        self.state.lock().await.take()
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
     LocalMessages: ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
     SenderChannel: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync,
     StateType:                                                                                   Send + Sync>
Debug for
Peer<CONFIG, LocalMessages, SenderChannel, StateType> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Peer {{peer_id: {}, peer_address: '{}', sender: {}/{} pending messages}}",
               self.peer_id, self.peer_address, self.retryable_sender.pending_items_count(), self.retryable_sender.buffer_size())
    }
}