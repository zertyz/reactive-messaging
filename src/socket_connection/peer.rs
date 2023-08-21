//! Resting place for [Peer], representing the remote part on a TCP/IP socket connection

use std::fmt::{Debug, Formatter};
use std::future::Future;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering::Relaxed;
use std::net::SocketAddr;
use std::time::Duration;
use reactive_mutiny::prelude::{ChannelCommon, ChannelUni, MutinyStream};
use reactive_mutiny::types::FullDuplexUniChannel;
use crate::ReactiveMessagingSerializer;
use crate::socket_connection::common::{RetryableSender};


static PEER_COUNTER: AtomicU32 = AtomicU32::new(0);
pub type PeerId = u32;



/// Represents a reactive channel connected to a remote peer, through which we're able to send out "local messages" of type `RetryableSenderImpl::LocalMessages`.\
/// the [Self::send()] method honors whatever retrying config is specified in [RetryableSenderImpl::CONST_CONFIG].
pub struct Peer<RetryableSenderImpl: RetryableSender<LocalMessages=LocalMessages, SenderChannelType=SenderChannelType> + Send + Sync,
                // Rust BUG ahead! The unusual & complicated generic notations ahead (instead of simply using RetryableSenderImpl::Type everywhere)
                // are to overcome a silly Rust Compiler bug that seems hard to fix, as it has been biting rustaceans around for ~1.5 years now,
                // which happens when the compiler tries to infer the `Send` auto-trait for types of Futures returned by structs that use traits that has GATs
                // (generic associated types) in them, pooping out messages like "implementation of Send is not general enough" & "Cannot prove the returned Future is Send"
                // See: https://github.com/rust-lang/rust/issues/96865
                //      https://github.com/rust-lang/rust/issues/102211#issuecomment-1513931928
                //      Also see those other zero-cost fixes, for different incarnations of the same bug:
                //        https://play.rust-lang.org/?version=nightly&mode=debug&edition=2021&gist=554fb0f6c23beadeb4d239fcf5b7d433 -- zero-cost fix for bad type inference in the returned future of GAT structs
                //        https://play.rust-lang.org/?version=stable&mode=debug&edition=2021&gist=a4f501590c778b0dbe78ca620af23b4d  -- zero-cost fix bad type inferences in closures (but only fixes Fn, not FnOnce nor FnMut) -- this happens even when GAT is not involved
                LocalMessages      : ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug = <RetryableSenderImpl as RetryableSender>::LocalMessages,
                SenderChannelType  : FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync                     = <RetryableSenderImpl as RetryableSender>::SenderChannelType,
> {
    pub peer_id:          PeerId,
    pub peer_address:     SocketAddr,
        retryable_sender: Box<RetryableSenderImpl>,
}

impl<RetryableSenderImpl: RetryableSender<LocalMessages=LocalMessages, SenderChannelType=SenderChannelType> + Send + Sync,
     LocalMessages:       ReactiveMessagingSerializer<LocalMessages>                                        + Send + Sync + PartialEq + Debug,
     SenderChannelType:   FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages>       + Send + Sync>
Peer<RetryableSenderImpl, LocalMessages, SenderChannelType> {

    pub fn new(retryable_sender: Box<RetryableSenderImpl>, peer_address: SocketAddr) -> Self {
        Self {
            peer_id: PEER_COUNTER.fetch_add(1, Relaxed),
            retryable_sender,
            peer_address,
        }
    }

    /// Asks the underlying channel to revert to Stream-mode (rather than Execution-mode), returning the `Stream`
    pub fn create_stream(&self) -> (MutinyStream<'static, LocalMessages, SenderChannelType, LocalMessages>, u32) {
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

        return self.retryable_sender.send_async_trait(message).await;
    }

    pub async fn flush_and_close(&self, timeout: Duration) -> u32 {
        self.retryable_sender.flush_and_close(timeout).await
    }

    pub fn cancel_and_close(&self) {
        self.retryable_sender.channel().cancel_all_streams();
    }
}


impl<RetryableSenderImpl: RetryableSender + Send + Sync>
Debug for
Peer<RetryableSenderImpl> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Peer {{peer_id: {}, peer_address: '{}', sender: {}/{} pending messages}}",
               self.peer_id, self.peer_address, self.retryable_sender.channel().pending_items_count(), self.retryable_sender.channel().buffer_size())
    }
}