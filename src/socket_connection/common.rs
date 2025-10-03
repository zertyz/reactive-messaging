//! Contains some functions and other goodies used across this module


use crate::{
    config::{
        ConstConfig,
        RetryingStrategies,
    },
};
use reactive_mutiny::prelude::{GenericUni, MutinyStream,FullDuplexUniChannel};
use std::{
    fmt::Debug,
    future::{self},
    sync::Arc,
    time::{Duration, SystemTime},
};
use futures::{Stream, StreamExt};
use keen_retry::ExponentialJitter;
use log::{trace, warn};
use crate::prelude::Peer;
use crate::serde::ReactiveMessagingConfig;
use crate::types::ResponsiveStream;

/// Upgrades a standard `GenericUni` to a version able to retry, as dictated by `CONFIG`
pub fn upgrade_processor_uni_retrying_logic<const CONFIG: u64,
                                            ItemType:        Send + Sync + Debug + 'static,
                                            DerivedItemType: Send + Sync + Debug + 'static,
                                            OriginalUni:     GenericUni<ItemType=ItemType, DerivedItemType=DerivedItemType> + Send + Sync>
                                           (running_uni: Arc<OriginalUni>)
                                           -> ReactiveMessagingUniSender<CONFIG, ItemType, DerivedItemType, OriginalUni> {
    ReactiveMessagingUniSender::<CONFIG,
                                 ItemType,
                                 DerivedItemType,
                                 OriginalUni>::new(running_uni)
}

/// Our special sender over a [Uni], adding
/// retrying logic & connection control return values
/// -- used to "send" messages from the remote peer to the local processor `Stream`
pub struct ReactiveMessagingUniSender<const CONFIG: u64,
                                      DeserializedRemoteMessages: Send + Sync + Debug + 'static,
                                      ConsumedRemoteMessages:     Send + Sync + Debug + 'static,
                                      OriginalUni:                GenericUni<ItemType=DeserializedRemoteMessages, DerivedItemType=ConsumedRemoteMessages> + Send + Sync> {
    uni: Arc<OriginalUni>,
}
impl<const CONFIG: u64,
    DeserializedRemoteMessages: Send + Sync + Debug + 'static,
     ConsumedRemoteMessages:    Send + Sync + Debug + 'static,
     OriginalUni:               GenericUni<ItemType=DeserializedRemoteMessages, DerivedItemType=ConsumedRemoteMessages> + Send + Sync>
ReactiveMessagingUniSender<CONFIG, DeserializedRemoteMessages, ConsumedRemoteMessages, OriginalUni> {

    const CONST_CONFIG: ConstConfig = ConstConfig::from(CONFIG);

    /// Takes in a [Uni] (already configured and under execution) and wrap it to allow
    /// our special [Self::send()] to operate on it
    pub fn new(running_uni: Arc<OriginalUni>) -> Self {
        Self {
            uni: running_uni,
        }
    }

    /// mapper for eventual first-time-being retrying attempts -- or for fatal errors that might happen during retrying
    fn retry_error_mapper(abort: bool, error_msg: String) -> ((), (bool, String) ) {
        ( (), (abort, error_msg) )
    }
    /// mapper for any fatal errors that happens on the first attempt (which should not happen in the current `reactive-mutiny` Uni Channel API)
    fn first_attempt_error_mapper<T>(_: T, _: ()) -> ((), (bool, String) ) {
        panic!("reactive-messaging: send_to_local_processor(): BUG! `Uni` channel is expected never to fail fatably. Please, fix!")
    }

    /// Routes a received `message` (from a remote peer) to the local processor, honoring the configured retrying options.\
    /// Returns `Ok` if sent, `Err(details)` if sending was not possible, where `details` contain:
    ///   - `(abort?, error_message, unsent_message)`
    #[inline(always)]
    pub async fn send(&self,
                      message: DeserializedRemoteMessages)
                     -> Result<(), (/*abort?*/bool, /*error_message: */String)> {

        let retryable = self.uni.send(message);
        match Self::CONST_CONFIG.retrying_strategy {
            RetryingStrategies::DoNotRetry => {
                retryable
                    .map_input_and_errors(
                        Self::first_attempt_error_mapper,
                        |message, _err|
                            Self::retry_error_mapper(false, format!("Relaying received message '{:?}' to the internal processor failed. Won't retry (upstreaming the error) due to retrying config {:?}",
                                                                                    message, Self::CONST_CONFIG.retrying_strategy)) )
                    .into_result()
            },
            RetryingStrategies::EndCommunications => {
                retryable
                    .map_input_and_errors(
                        Self::first_attempt_error_mapper,
                        |message, _err|
                            Self::retry_error_mapper(true, format!("Relaying received message '{:?}' to the internal processor failed. Connection will be aborted due to retrying config {:?}",
                                                                                  message, Self::CONST_CONFIG.retrying_strategy)) )
                    .into_result()
            },
            RetryingStrategies::RetryWithBackoffUpTo(attempts) => {
                retryable
                    .map_input(|message| ( message, SystemTime::now()) )
                    .retry_with_async(|(message, retry_start)| future::ready(
                        self.uni.send(message)
                            .map_input(|message| (message, retry_start) )
                    ))
                    .with_exponential_jitter(|| ExponentialJitter::FromBackoffRange {
                        backoff_range_millis: 0..=(1.468935_f32.powi(attempts as i32 - 1) as u32),
                        re_attempts: attempts,
                        jitter_ratio: 0.2,
                    })
                    .await
                    .map_input_and_errors(
                        |(message, retry_start), _fatal_err|
                            Self::retry_error_mapper(true, format!("Relaying received message '{:?}' to the internal processor failed. Connection will be aborted (after exhausting all retries in {:?}) due to retrying config {:?}",
                                                                                   message, retry_start.elapsed(), Self::CONST_CONFIG.retrying_strategy)),
                        |_| (false, String::new()) )
                    .into()
            },
            RetryingStrategies::RetryYieldingForUpToMillis(millis) => {
                retryable
                    .map_input(|message| ( message, SystemTime::now()) )
                    .retry_with_async(|(message, retry_start)| future::ready(
                        self.uni.send(message)
                            .map_input(|message| (message, retry_start) )
                    ))
                    .yielding_until_timeout(Duration::from_millis(millis as u64), || ())
                    .await
                    .map_input_and_errors(
                        |(message, retry_start), _fatal_err|
                            Self::retry_error_mapper(true, format!("Relaying received message '{:?}' to the internal processor failed. Connection will be aborted (after exhausting all retries in {:?}) due to retrying config {:?}",
                                                                                   message, retry_start.elapsed(), Self::CONST_CONFIG.retrying_strategy)),
                        |_| (false, String::new()) )
                    .into()
            },
            RetryingStrategies::RetrySpinningForUpToMillis(_millis) => {
                // this option is deprecated
                unreachable!()
            },
        }
    }

    #[inline(always)]
    pub async fn send_derived(&self) {
        // TODO 2024-02-26: self.uni.send_derived... search for other 2024-02-26 TODOs
    }

    /// Reserves a slot for a new `message` to be received (from a remote peer), for later [Self::try_send_reserved()] to the local processor.
    /// Here, the configured retrying options is honored.\
    /// Returns `Ok` if reserved, `Err(details)` if reserving was not possible, where `details` contain:
    ///   - `(abort?, error_message)`
    #[inline(always)]
    pub async fn reserve_slot(&self) -> Result<&mut DeserializedRemoteMessages, (/*abort?*/bool, /*error_message: */String)> {

        // TODO: 2025-10-02: `reactive-mutiny` is missing some retry logic... adding here for now
        let retryable_reserve_slot = || -> keen_retry::RetryProducerResult<&mut DeserializedRemoteMessages, ()> {
            match self.uni.reserve_slot() {
                Some(slot) => keen_retry::RetryProducerResult::Ok { reported_input: (), output: slot },
                None => keen_retry::RetryProducerResult::Transient { input: (), error: () },
            }
        };

        let retryable = retryable_reserve_slot();
        match Self::CONST_CONFIG.retrying_strategy {
            RetryingStrategies::DoNotRetry => {
                retryable
                    .map_input_and_errors(
                        Self::first_attempt_error_mapper,
                        |(), _err|
                            Self::retry_error_mapper(false, format!("Reserving a slot to receive & relay the next message to the internal processor failed. Won't retry (upstreaming the error) due to retrying config {:?}",
                                                                           Self::CONST_CONFIG.retrying_strategy)) )
                    .into_result()
            },
            RetryingStrategies::EndCommunications => {
                retryable
                    .map_input_and_errors(
                        Self::first_attempt_error_mapper,
                        |(), _err|
                            Self::retry_error_mapper(true, format!("Reserving a slot to receive & relay the next message to the internal processor failed. Connection will be aborted due to retrying config {:?}",
                                                                                  Self::CONST_CONFIG.retrying_strategy)) )
                    .into_result()
            },
            RetryingStrategies::RetryWithBackoffUpTo(attempts) => {
                retryable
                    .map_input(|()| ( (), SystemTime::now()) )
                    .retry_with_async(|((), retry_start)| future::ready(
                        retryable_reserve_slot()
                            .map_input(|()| ((), retry_start) )
                    ))
                    .with_exponential_jitter(|| ExponentialJitter::FromBackoffRange {
                        backoff_range_millis: 0..=(1.468935_f32.powi(attempts as i32 - 1) as u32),
                        re_attempts: attempts,
                        jitter_ratio: 0.2,
                    })
                    .await
                    .map_input_and_errors(
                        |((), retry_start), _fatal_err|
                            Self::retry_error_mapper(true, format!("Reserving a slot to receive & relay the next message to the internal processor failed. Connection will be aborted (after exhausting all retries in {:?}) due to retrying config {:?}",
                                                                                  retry_start.elapsed(), Self::CONST_CONFIG.retrying_strategy)),
                        |_| (false, String::new()) )
                    .into()
            },
            RetryingStrategies::RetryYieldingForUpToMillis(millis) => {
                retryable
                    .map_input(|()| ( (), SystemTime::now()) )
                    .retry_with_async(|((), retry_start)| future::ready(
                        retryable_reserve_slot()
                            .map_input(|()| ((), retry_start) )
                    ))
                    .yielding_until_timeout(Duration::from_millis(millis as u64), || ())
                    .await
                    .map_input_and_errors(
                        |((), retry_start), _fatal_err|
                            Self::retry_error_mapper(true, format!("Reserving a slot to receive & relay the next message to the internal processor failed. Connection will be aborted (after exhausting all retries in {:?}) due to retrying config {:?}",
                                                                                  retry_start.elapsed(), Self::CONST_CONFIG.retrying_strategy)),
                        |_| (false, String::new()) )
                    .into()
            },
            RetryingStrategies::RetrySpinningForUpToMillis(_millis) => {
                // this option is deprecated
                unreachable!()
            },
        }
    }

    #[inline(always)]
    pub fn send_reserved(&self, slot: &mut DeserializedRemoteMessages) {
        // As of now, our understanding is that this method can never return false.
        // Even though `reactive-mutiny` says false may be returned as part of the
        // normal operation (in which case you may retry), sending might only fail
        // for `zero copy` channels, which -- internally -- uses 2 queues:
        // * One for the bounded allocator (zero-copy)
        // * Another for publishing entries from the bounded allocator (movable).
        //
        // Since our Uni channels are defined to have both the allocator and
        // publishing queues to have the same size, there is no possibility
        // this method will fail and, therefore, we can safely bail out from
        // retrying.
        let result = self.uni.try_send_reserved(slot);
        debug_assert!(result, "`reactive-messaging`: bug in the `.try_send_reserver()` logic. Please inspect the comments at this location to fix it");
    }

    #[inline(always)]
    pub fn try_cancel_reservation(&self, slot: &mut DeserializedRemoteMessages) -> bool {
        self.uni.try_cancel_slot_reserve(slot)
    }

    /// See [GenericUni::pending_items_count()]
    #[inline(always)]
    pub fn pending_items_count(&self) -> u32 {
        self.uni.pending_items_count()
    }

    /// See [GenericUni::buffer_size()]
    #[inline(always)]
    pub fn buffer_size(&self) -> u32 {
        self.uni.buffer_size()
    }

    /// See [GenericUni::close()]
    pub async fn close(&self, timeout: Duration) -> bool {
        self.uni.close(timeout).await
    }
}

/// Our special "sender of messages to the remote peer" over a `reactive-mutiny`s [FullDuplexUniChannel], adding
/// retrying logic & connection control return values
/// -- used to send messages to the remote peer
pub struct ReactiveMessagingSender<const CONFIG:    u64,
                                   LocalMessages:                                                                                 Send + Sync + PartialEq + Debug + 'static,
                                   OriginalChannel: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync> {
    channel: Arc<OriginalChannel>,
}
impl<const CONFIG: u64,
     LocalMessages:                                                                                 Send + Sync + PartialEq + Debug,
     OriginalChannel: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync>
ReactiveMessagingSender<CONFIG, LocalMessages, OriginalChannel> {

    /// The instance config this generic implementation adheres to
    pub const CONST_CONFIG: ConstConfig = ConstConfig::from(CONFIG);

    /// Instantiates a new `channel` (from `reactive-mutiny`, with type `Self::SenderChannelType`) and wrap in a way to allow
    /// our special [Self::send()] to operate on
    pub fn new<IntoString: Into<String>>(channel_name: IntoString) -> Self {
        Self {
            channel: OriginalChannel::new(channel_name.into()),
        }
    }

    pub fn create_stream(&self) -> (MutinyStream<'static, LocalMessages, OriginalChannel, LocalMessages>, u32) {
        self.channel.create_stream()
    }

    #[inline(always)]
    pub fn pending_items_count(&self) -> u32 {
        self.channel.pending_items_count()
    }

    #[inline(always)]
    pub fn buffer_size(&self) -> u32 {
        self.channel.buffer_size()
    }

    pub async fn flush_and_close(&self, timeout: Duration) -> u32 {
        self.channel.gracefully_end_all_streams(timeout).await
    }

    pub fn cancel_and_close(&self) {
        self.channel.cancel_all_streams();
    }

    /// Routes `message` to the remote peer,
    /// honoring the configured retrying options.
    /// On error, returns whether the connection should be dropped or not.\
    /// Returns `Ok` if sent successfully, `Err(details)` if sending was not possible, where `details` contain:
    ///   - `(abort_the_connection?, error_message)`
    /// 
    /// See [Self::send_async_trait()] if your retrying strategy sleeps, and you are calling this from an async context.
    #[inline(always)]
    pub fn send(&self,
                message: LocalMessages)
               -> Result<(), (/*abort_the_connection?*/bool, /*error_message: */String)> {

        let retryable = self.channel.send(message);
        match Self::CONST_CONFIG.retrying_strategy {
            RetryingStrategies::DoNotRetry => {
                retryable
                    .map_input_and_errors(
                        Self::first_attempt_error_mapper,
                        |message, _err|
                            Self::retry_error_mapper(false, format!("sync-Sending '{:?}' failed. Won't retry (upstreaming the error) due to retrying config {:?}",
                                                                                    message, Self::CONST_CONFIG.retrying_strategy)) )
                    .into_result()
            },
            RetryingStrategies::EndCommunications => {
                retryable
                    .map_input_and_errors(
                        Self::first_attempt_error_mapper,
                        |message, _err|
                            Self::retry_error_mapper(true, format!("sync-Sending '{:?}' failed. Connection will be aborted due to retrying config {:?}",
                                                                                   message, Self::CONST_CONFIG.retrying_strategy)) )
                    .into_result()
            },
            RetryingStrategies::RetryWithBackoffUpTo(attempts) => {
                retryable
                    .map_input(|message| ( message, SystemTime::now()) )
                    .retry_with(|(message, retry_start)|
                        self.channel.send(message)
                            .map_input(|message| (message, retry_start) )
                    )
                    .with_exponential_jitter(|| ExponentialJitter::FromBackoffRange {
                        backoff_range_millis: 0..=(1.468935_f32.powi(attempts as i32 - 1) as u32),
                        re_attempts: attempts,
                        jitter_ratio: 0.2,
                    })
                    .map_input_and_errors(
                        |(message, retry_start), _fatal_err|
                            Self::retry_error_mapper(true, format!("sync-Sending '{:?}' failed. Connection will be aborted (after exhausting all retries in {:?}) due to retrying config {:?}",
                                                                                   message, retry_start.elapsed(), Self::CONST_CONFIG.retrying_strategy)),
                        |_| (false, String::with_capacity(0)) )
                    .into()
            },
            RetryingStrategies::RetryYieldingForUpToMillis(millis) => {
                // note: since we are in a synchronous method, here yielding = spinning
                retryable
                    .map_input(|message| ( message, SystemTime::now()) )
                    .retry_with(|(message, retry_start)|
                        self.channel.send(message)
                            .map_input(|message| (message, retry_start) )
                    )
                    .spinning_until_timeout(Duration::from_millis(millis as u64), ())
                    .map_input_and_errors(
                        |(message, retry_start), _fatal_err|
                            Self::retry_error_mapper(true, format!("sync-Sending '{:?}' failed. Connection will be aborted (after exhausting all retries in {:?}) due to retrying config {:?}",
                                                                                   message, retry_start.elapsed(), Self::CONST_CONFIG.retrying_strategy)),
                        |_| (false, String::with_capacity(0)) )
                    .into()
            },
            RetryingStrategies::RetrySpinningForUpToMillis(_millis) => {
                // this option is deprecated
                unreachable!()
            },
        }
    }

    /// Similar to [Self::send()], but async.
    /// The name contains `async_trait` to emphasize that there is a performance loss when calling this function through the trait:
    /// A boxing + dynamic dispatch -- this cost is not charged when calling this function from the implementing type directly.
    /// Depending on your retrying strategy, it might be preferred to use [Self::send()] instead -- knowing it will cause the whole thread to sleep,
    /// when retrying, instead of causing only the task to sleep (as done here).
    #[inline(always)]
    pub async fn send_async_trait(&self,
                                  message: LocalMessages)
                                 -> Result<(), (/*abort_the_connection?*/bool, /*error_message: */String)> {

        let retryable = self.channel.send(message);
        match Self::CONST_CONFIG.retrying_strategy {
            RetryingStrategies::DoNotRetry => {
                retryable
                    .map_input_and_errors(
                        Self::first_attempt_error_mapper,
                        |message, _err|
                            Self::retry_error_mapper(false, format!("async-Sending '{:?}' failed. Won't retry (upstreaming the error) due to retrying config {:?}",
                                                                                    message, Self::CONST_CONFIG.retrying_strategy)) )
                    .into_result()
            },
            RetryingStrategies::EndCommunications => {
                retryable
                    .map_input_and_errors(
                        Self::first_attempt_error_mapper,
                        |message, _err|
                            Self::retry_error_mapper(true, format!("async-Sending '{:?}' failed. Connection will be aborted (without retrying) due to retrying config {:?}",
                                                                                   message, Self::CONST_CONFIG.retrying_strategy)) )
                    .into_result()
            },
            RetryingStrategies::RetryWithBackoffUpTo(attempts) => {
                retryable
                    .map_input(|message| ( message, SystemTime::now()) )
                    .retry_with_async(|(message, retry_start)| future::ready(
                        self.channel.send(message)
                            .map_input(|message| (message, retry_start) )
                    ))
                    .with_exponential_jitter(|| ExponentialJitter::FromBackoffRange {
                        backoff_range_millis: 0..=(1.468935_f32.powi(attempts as i32 - 1) as u32),
                        re_attempts: attempts,
                        jitter_ratio: 0.2,
                    })
                    .await
                    .map_input_and_errors(
                        |(message, retry_start), _fatal_err|
                            Self::retry_error_mapper(true, format!("async-Sending '{:?}' failed. Connection will be aborted (after exhausting all retries in {:?}) due to retrying config {:?}",
                                                                                   message, retry_start.elapsed(), Self::CONST_CONFIG.retrying_strategy)),
                        |_| (false, String::new()) )
                    .into()
            },
            RetryingStrategies::RetryYieldingForUpToMillis(millis) => {
                retryable
                    .map_input(|message| ( message, SystemTime::now()) )
                    .retry_with_async(|(message, retry_start)| future::ready(
                        self.channel.send(message)
                            .map_input(|message| (message, retry_start) )
                    ))
                    .yielding_until_timeout(Duration::from_millis(millis as u64), || ())
                    .await
                    .map_input_and_errors(
                        |(message, retry_start), _fatal_err|
                            Self::retry_error_mapper(true, format!("async-Sending '{:?}' failed. Connection will be aborted (after exhausting all retries in {:?}) due to retrying config {:?}",
                                                                                   message, retry_start.elapsed(), Self::CONST_CONFIG.retrying_strategy)),
                        |_| (false, String::new()) )
                    .into()
            },
            RetryingStrategies::RetrySpinningForUpToMillis(_millis) => {
                // this option is deprecated
                unreachable!()
            },
        }
    }

    /// mapper for eventual first-time-being retrying attempts -- or for fatal errors that might happen during retrying
    fn retry_error_mapper(abort: bool, error_msg: String) -> ((), (bool, String) ) {
        ( (), (abort, error_msg) )
    }
    /// mapper for any fatal errors that happens on the first attempt (which should not happen in the current `reactive-mutiny` Uni Channel API)
    fn first_attempt_error_mapper<T>(_: T, _: ()) -> ((), (bool, String) ) {
        panic!("reactive-messaging: send_to_remote_peer(): BUG! `Uni` channel is expected never to fail fatably. Please, fix!")
    }

}

impl<const CONFIG:        u64,
     T:                   ?Sized,
     LocalMessagesType:   ReactiveMessagingConfig<LocalMessagesType>                                          + Send + Sync + PartialEq + Debug,
     SenderChannel:       FullDuplexUniChannel<ItemType=LocalMessagesType, DerivedItemType=LocalMessagesType> + Send + Sync,
     StateType:                                                                                                 Send + Sync + Clone     + Debug>
ResponsiveStream<CONFIG, LocalMessagesType, SenderChannel, StateType>
for T where T: Stream<Item=LocalMessagesType> {

    #[inline(always)]
    fn to_responsive_stream<YieldedItemType>

                           (self,
                            peer:            Arc<Peer<CONFIG, LocalMessagesType, SenderChannel, StateType>>,
                            mut item_mapper: impl FnMut(&LocalMessagesType, &Arc<Peer<CONFIG, LocalMessagesType, SenderChannel, StateType>>) -> YieldedItemType)

                           -> impl Stream<Item = YieldedItemType>

                           where Self: Sized + Stream<Item = LocalMessagesType> {

        let flush_timeout_millis = peer.config().flush_timeout_millis;

        // send back each message
        self.map(move |outgoing| {
            trace!("`to_responsive_stream()`: Sending Answer `{:?}` to {:?} (peer id {})", outgoing, peer.peer_address, peer.peer_id);
            let remapped_item = item_mapper(&outgoing, &peer);
            if let Err((abort, error_msg)) = peer.send(outgoing) {
                // peer is slow-reading -- and, possibly, fast sending
                warn!("`to_responsive_stream()`: Slow reader detected while sending to {peer:?}: {error_msg}");
                if abort {
                    std::thread::sleep(Duration::from_millis(flush_timeout_millis as u64));
                    peer.cancel_and_close();
                }
            }
            remapped_item
        })

    }
}

/// Common test code for this module
#[cfg(any(test,doc))]
mod tests {
}