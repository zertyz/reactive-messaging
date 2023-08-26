//! Contains some functions and other goodies used across this module


use std::fmt::Debug;
use std::future;
use std::future::Future;
use std::process::Output;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use async_trait::async_trait;
use reactive_mutiny::prelude::{GenericUni, MutinyStream};
use reactive_mutiny::types::FullDuplexUniChannel;
use crate::config::{ConstConfig, RetryingStrategies};
use crate::ReactiveMessagingSerializer;

/// Upgrades a standard `GenericUni` to a version able to retry, as dictated by `COFNIG_USIZE`
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

/// Upgrades a standard "sender" ([FullDuplexUniChannel]) to a version able to retry, as dictated by `CONFIG_USIZE`
/*pub fn upgrade_sender_retrying_logic<const CONFIG: usize,
                                     ItemType:          ReactiveMessagingSerializer<ItemType>                             + Send + Sync + Debug + PartialEq + 'static,
                                     SenderChannelType: FullDuplexUniChannel<ItemType=ItemType, DerivedItemType=ItemType> + Send + Sync                     + 'static>
                                    (channel:  Arc<SenderChannelType>)
                                    -> impl RetryableSender<LocalMessages=ItemType, SenderChannelType=SenderChannelType> {
    ReactiveMessagingSender::<CONFIG, ItemType, SenderChannelType, > { channel }
}
*/
#[async_trait]
/// Our contract for applying the retrying logic to [FullDuplexUniChannel]s when
/// sending them out to the remote peer
pub trait RetryableSender {

    /// The instance config the implementation adheres to
    const CONST_CONFIG: ConstConfig;
    /// The type of the local messages to be delivered to the remote peer
    type LocalMessages:     ReactiveMessagingSerializer<Self::LocalMessages>                                        + Send + Sync + PartialEq + Debug;
    type SenderChannelType: FullDuplexUniChannel<ItemType=Self::LocalMessages, DerivedItemType=Self::LocalMessages> + Send + Sync;

    /// Instantiates a new `channel` (from `reactive-mutiny`, with type `Self::SenderChannelType`) and wrap in a way to allow
    /// our special [Self::send()] to operate on
    fn new<IntoString: Into<String>>(channel_name: IntoString) -> Self where Self: Sized;

    fn create_stream(&self) -> (MutinyStream<'static, Self::LocalMessages, Self::SenderChannelType, Self::LocalMessages>, u32);

    /// IMPLEMENTORS: #[inline(always)]
    fn pending_items_count(&self) -> u32;

    /// IMPLEMENTORS: #[inline(always)]
    fn buffer_size(&self) -> u32;

    async fn flush_and_close(&self, timeout: Duration) -> u32;

    fn cancel_and_close(&self);

    /// Routes `message` to the remote peer,
    /// honoring the configured retrying options.
    /// On error, returns whether the connection should be dropped or not.\
    /// Returns `Ok` if sent successfully, `Err(details)` if sending was not possible, where `details` contain:
    ///   - `(abort_the_connection?, error_message)`
    /// See [Self::send_async_trait()] if your retrying strategy sleeps and you are calling this from an async context.
    /// IMPLEMENTORS: #[inline(always)]
    fn send(&self,
            message: Self::LocalMessages)
           -> Result<(), (/*abort_the_connection?*/bool, /*error_message: */String)>;

    /// Similar to [Self::send()], but async.
    /// The name contains `async_trait` to emphasize that there is a performance loss when calling this function through the trait:
    /// A boxing + dynamic dispatch -- this cost is not charged when calling this function from the implementing type directly.
    /// Depending on your retrying strategy, it might be preferred to use [Self::send()] instead -- knowing it will cause the whole thread to sleep,
    /// when retrying, instead of causing only the task to sleep (as done here).
    /// IMPLEMENTORS: #[inline(always)]
    async fn send_async_trait(&self,
                              message: Self::LocalMessages)
                             -> Result<(), (/*abort_the_connection?*/bool, /*error_message: */String)>;

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
            RetryingStrategies::RetrySleepingArithmetically(steps) => {
                retryable
                    .map_input(|message| ( message, SystemTime::now()) )
                    .retry_with_async(|(message, retry_start)| future::ready(
                        self.uni.send(message)
                            .map_input(|message| (message, retry_start) )
                    ))
                    .with_delays((10..=(10*steps as u64)).step_by(10).map(|millis| Duration::from_millis(millis)))
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
            RetryingStrategies::RetrySpinningForUpToMillis(millis) => {
                /// this option is deprecated
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

    /// mapper for eventual first-time-being retrying attempts -- or for fatal errors that might happen during retrying
    fn retry_error_mapper(abort: bool, error_msg: String) -> ((), (bool, String) ) {
        ( (), (abort, error_msg) )
    }
    /// mapper for any fatal errors that happens on the first attempt (which should not happen in the current `reactive-mutiny` Uni Channel API)
    fn first_attempt_error_mapper<T>(_: T, _: ()) -> ((), (bool, String) ) {
        panic!("reactive-messaging: send_to_remote_peer(): BUG! `Uni` channel is expected never to fail fatably. Please, fix!")
    }
}

#[async_trait]
impl<const CONFIG: u64,
     LocalMessages:   ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug,
     OriginalChannel: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync>
RetryableSender for
ReactiveMessagingSender<CONFIG, LocalMessages, OriginalChannel> {
    const CONST_CONFIG: ConstConfig = ConstConfig::from(CONFIG);
    type LocalMessages = LocalMessages;
    type SenderChannelType = OriginalChannel;

    fn new<IntoString: Into<String>>(channel_name: IntoString) -> Self {
        Self {
            channel: OriginalChannel::new(channel_name.into()),
        }
    }

    fn create_stream(&self) -> (MutinyStream<'static, LocalMessages, OriginalChannel, LocalMessages>, u32) {
        self.channel.create_stream()
    }

    #[inline(always)]
    fn pending_items_count(&self) -> u32 {
        self.channel.pending_items_count()
    }

    #[inline(always)]
    fn buffer_size(&self) -> u32 {
        self.channel.buffer_size()
    }

    async fn flush_and_close(&self, timeout: Duration) -> u32 {
        self.channel.gracefully_end_all_streams(timeout).await
    }

    fn cancel_and_close(&self) {
        self.channel.cancel_all_streams();
    }

    #[inline(always)]
    fn send(&self,
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
            RetryingStrategies::RetrySleepingArithmetically(steps) => {
                retryable
                    .map_input(|message| ( message, SystemTime::now()) )
                    .retry_with(|(message, retry_start)|
                        self.channel.send(message)
                            .map_input(|message| (message, retry_start) )
                    )
                    .with_delays((10..=(10*steps as u64)).step_by(10).map(|millis| Duration::from_millis(millis)))
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
            RetryingStrategies::RetrySpinningForUpToMillis(millis) => {
                panic!("deprecated")
            },
        }
    }

    #[inline(always)]
    async fn send_async_trait(&self,
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
            RetryingStrategies::RetrySleepingArithmetically(steps) => {
                retryable
                    .map_input(|message| ( message, SystemTime::now()) )
                    .retry_with_async(|(message, retry_start)| future::ready(
                        self.channel.send(message)
                            .map_input(|message| (message, retry_start) )
                    ))
                    .with_delays((10..=(10*steps as u64)).step_by(10).map(|millis| Duration::from_millis(millis)))
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
            RetryingStrategies::RetrySpinningForUpToMillis(millis) => {
                panic!("deprecated")
            },
        }
    }
}

/// Common test code for this module
#[cfg(any(test,doc))]
mod tests {
    use crate::{ReactiveMessagingDeserializer, ReactiveMessagingSerializer};
    use crate::types::ResponsiveMessages;

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

    /// Our test text-only protocol's messages may also be used by "Responsive Processors"
    impl ResponsiveMessages<String> for String {
        #[inline(always)]
        fn is_disconnect_message(processor_answer: &String) -> bool {
            // for String communications, an empty line sent by the messages processor signals that the connection should be closed
            processor_answer.is_empty()
        }
        #[inline(always)]
        fn is_no_answer_message(processor_answer: &String) -> bool {
            processor_answer == "."
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