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
use crate::serde::ReactiveMessagingSerializer;
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
                                      RemoteMessages:         Send + Sync + Debug + 'static,
                                      ConsumedRemoteMessages: Send + Sync + Debug + 'static,
                                      OriginalUni:            GenericUni<ItemType=RemoteMessages, DerivedItemType=ConsumedRemoteMessages> + Send + Sync> {
    uni: Arc<OriginalUni>,
}
impl<const CONFIG: u64,
     RemoteMessages:         Send + Sync + Debug + 'static,
     ConsumedRemoteMessages: Send + Sync + Debug + 'static,
     OriginalUni:            GenericUni<ItemType=RemoteMessages, DerivedItemType=ConsumedRemoteMessages> + Send + Sync>
ReactiveMessagingUniSender<CONFIG, RemoteMessages, ConsumedRemoteMessages, OriginalUni> {

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
                      message: RemoteMessages)
                     -> Result<(), (/*abort?*/bool, /*error_message: */String)> {

        let retryable = self.uni.send(message);
        match Self::CONST_CONFIG.retrying_strategy {
            RetryingStrategies::DoNotRetry => {
                retryable
                    .map_input_and_errors(
                        Self::first_attempt_error_mapper,
                        |message, _err|
                            Self::retry_error_mapper(false, format!("Relaying received message '{:?}' to the internal processor failed. Won't retry (ignoring the error) due to retrying config {:?}",
                                                                                    message, Self::CONST_CONFIG.retrying_strategy)) )
                    .into_result()
            },
            RetryingStrategies::EndCommunications => {
                retryable
                    .map_input_and_errors(
                        Self::first_attempt_error_mapper,
                        |message, _err|
                            Self::retry_error_mapper(false, format!("Relaying received message '{:?}' to the internal processor failed. Connection will be aborted (without retrying) due to retrying config {:?}",
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
                        backoff_range_millis: 1..=(2.526_f32.powi(attempts as i32) as u32),
                        re_attempts: attempts,
                        jitter_ratio: 0.2,
                    })
                    .await
                    .map_input_and_errors(
                        |(message, retry_start), _fatal_err|
                            Self::retry_error_mapper(true, format!("Relaying received message '{:?}' to the internal processor failed. Connection will be aborted (after exhausting all retries in {:?}) due to retrying config {:?}",
                                                                                   message, retry_start.elapsed(), Self::CONST_CONFIG.retrying_strategy)),
                        |_| (false, String::with_capacity(0)) )
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
                        |_| (false, String::with_capacity(0)) )
                    .into()
            },
            RetryingStrategies::RetrySpinningForUpToMillis(_millis) => {
                // this option is deprecated
                unreachable!()
            },
        }
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
                                   LocalMessages:   ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
                                   OriginalChannel: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync> {
    channel: Arc<OriginalChannel>,
}
impl<const CONFIG: u64,
     LocalMessages:   ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
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
                            Self::retry_error_mapper(false, format!("sync-Sending '{:?}' failed. Won't retry (ignoring the error) due to retrying config {:?}",
                                                                                    message, Self::CONST_CONFIG.retrying_strategy)) )
                    .into_result()
            },
            RetryingStrategies::EndCommunications => {
                retryable
                    .map_input_and_errors(
                        Self::first_attempt_error_mapper,
                        |message, _err|
                            Self::retry_error_mapper(true, format!("sync-Sending '{:?}' failed. Connection will be aborted (without retrying) due to retrying config {:?}",
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
                        backoff_range_millis: 1..=(2.526_f32.powi(attempts as i32) as u32),
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
                            Self::retry_error_mapper(false, format!("async-Sending '{:?}' failed. Won't retry (ignoring the error) due to retrying config {:?}",
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
                        backoff_range_millis: 1..=(2.526_f32.powi(attempts as i32) as u32),
                        re_attempts: attempts,
                        jitter_ratio: 0.2,
                    })
                    .await
                    .map_input_and_errors(
                        |(message, retry_start), _fatal_err|
                            Self::retry_error_mapper(true, format!("async-Sending '{:?}' failed. Connection will be aborted (after exhausting all retries in {:?}) due to retrying config {:?}",
                                                                                   message, retry_start.elapsed(), Self::CONST_CONFIG.retrying_strategy)),
                        |_| (false, String::with_capacity(0)) )
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
                        |_| (false, String::with_capacity(0)) )
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
     LocalMessagesType:   ReactiveMessagingSerializer<LocalMessagesType>                                      + Send + Sync + PartialEq + Debug,
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
    use crate::serde::{ReactiveMessagingDeserializer, ReactiveMessagingSerializer};

    /// Test implementation for our text-only protocol as used across this module
    impl ReactiveMessagingSerializer<String> for String {
        #[inline(always)]
        fn serialize(message: &String, buffer: &mut Vec<u8>) {
            buffer.clear();
            buffer.extend_from_slice(message.as_bytes());
        }
        #[inline(always)]
        fn processor_error_message(err: String) -> String {
            let msg = format!("ServerBug! Please, fix! Error: {}", err);
            panic!("SocketServerSerializer<String>::processor_error_message(): {}", msg);
            // msg
        }
    }

    /// Testable implementation for our text-only protocol as used across this module
    impl ReactiveMessagingDeserializer<String> for String {
        #[inline(always)]
        fn deserialize(message: &[u8]) -> Result<String, Box<dyn std::error::Error + Sync + Send + 'static>> {
            Ok(String::from_utf8_lossy(message).to_string())
        }
    }

}