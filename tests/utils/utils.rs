//! Contains common functions used across all the integration tests


use std::fmt::Debug;
use reactive_messaging::prelude::*;
use std::future;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::atomic::Ordering::Relaxed;
use std::time::{SystemTime, UNIX_EPOCH};
use futures::{FutureExt, Stream};
use futures::StreamExt;


/// Measures the current time, returning the elapsed µs since epoch.\
/// This function enables storing the current clock in an atomic `u128`, to avoid the need for a `Mutex<SystemTime>` when working with multiple threads
pub fn now_as_micros() -> u64 {
    let start_time = SystemTime::now();
    start_time.duration_since(UNIX_EPOCH).expect("Time went backwards?!?!").as_micros() as u64
}

/// Considering a `protocol_events_handler` callback (to be used in [MessagingService::spawn_responsive_processor()] && [MessagingService::spawn_unresponsive_processor()]),
/// returns a probed implementation (along with its probes) able to track the microseconds when the last events were received.
///
/// Returns a tuple containing the following elements:
/// 1) The closure implementation
/// 2) The probe for the µs time of the last [ProtocolEvent::PeerArrived] event
/// 3) The probe for the µs time of the last [ProtocolEvent::PeerLeft] event
/// 4) The probe for the µs time of the last [ProtocolEvent::LocalServiceTermination] event
///
/// For non-fired events, the probes will have the value of 0.
///
/// Usage:
/// ```nocompile
///     let (probed_protocol_events_handler,
///          last_peer_arrived_notification_micros,
///          last_peer_left_notification_micros,
///          last_local_service_termination_notification_micros) = last_micros_probed_protocol_events_handler();
pub fn last_micros_probed_protocol_events_handler<const CONFIG:  u64,
                                                  LocalMessages: ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
                                                  SenderChannel: FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync,
                                                  StateType:                                                                                   Send + Sync + Clone     + Debug + 'static>
                                                 ()
                                                 -> (/*probed_protocol_events_handler*/                     impl Fn(/*event: */ProtocolEvent<CONFIG, LocalMessages, SenderChannel, StateType>) -> Pin<Box<dyn Future<Output=()> + Send + Sync>> + Send + Sync,
                                                     /*last_peer_arrived_notification_micros*/              Arc<AtomicU64>,
                                                     /*last_peer_left_notification_micros*/                 Arc<AtomicU64>,
                                                     /*last_local_service_termination_notification_micros*/ Arc<AtomicU64>  ) {

    // probes
    let last_peer_arrived_notification_micros = Arc::new(AtomicU64::new(0));
    let last_peer_left_notification_micros = Arc::new(AtomicU64::new(0));
    let last_local_service_termination_notification_micros = Arc::new(AtomicU64::new(0));

    let last_peer_arrived_notification_micros_ref = Arc::clone(&last_peer_arrived_notification_micros);
    let last_peer_left_notification_micros_ref = Arc::clone(&last_peer_left_notification_micros);
    let last_local_service_termination_notification_micros_ref = Arc::clone(&last_local_service_termination_notification_micros);
    (
        move |event| {
            match event {
                ProtocolEvent::PeerArrived { peer }                                            => last_peer_arrived_notification_micros_ref.store(now_as_micros(), Relaxed),
                ProtocolEvent::PeerLeft { peer, stream_stats } => last_peer_left_notification_micros_ref.store(now_as_micros(), Relaxed),
                ProtocolEvent::LocalServiceTermination                                                      => last_local_service_termination_notification_micros_ref.store(now_as_micros(), Relaxed),
            }
            Box::pin(future::ready(()))
        },
        last_peer_arrived_notification_micros,
        last_peer_left_notification_micros,
        last_local_service_termination_notification_micros
    )

}

/// Considering a `protocol_processor_builder` function (to be used in [MessagingService::spawn_responsive_processor()] && [MessagingService::spawn_unresponsive_processor()]),
/// returns a probed implementation (along with its probe) able to produce streams that will track the microseconds when the last message was received.
///
/// Returns a tuple containing the following elements:
/// 1) The closure implementation
/// 2) The probe for the µs time of the last received remote message
///
/// While no messages are received, the probe will have the value of 0.
///
/// Usage:
/// ```nocompile
///     let (probed_protocol_processor_builder,
///          last_remote_message_micros) = last_micros_probed_protocol_processor_builder();
pub fn last_micros_probed_protocol_processor_builder<StreamItemType,
                                                     InputStreamType: Stream<Item=StreamItemType> + Send + 'static>
                                                    ()
                                                    -> (/*probed_protocol_processor_builder*/ impl Fn(/*remote_messages_stream: */InputStreamType) -> Pin<Box<dyn Stream<Item=StreamItemType> + Send>> + Send + Sync,
                                                        /*last_remote_message_micros*/        Arc<AtomicU64> ) {

    // probes
    let last_remote_message_micros = Arc::new(AtomicU64::new(0));

    let last_remote_message_micros_ref = Arc::clone(&last_remote_message_micros);
    (
        move |remote_messages_stream| {
            let last_remote_message_micros_ref = Arc::clone(&last_remote_message_micros_ref);
            Box::pin( remote_messages_stream.inspect(move |_server_message| {
                last_remote_message_micros_ref.store(now_as_micros(), Relaxed);
            }) )
        },
        last_remote_message_micros
    )

}