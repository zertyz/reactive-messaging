//! Contains the functional requirements (+ tests) for the client

use crate::utils::*;
use reactive_messaging::prelude::*;
use std::future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::atomic::Ordering::Relaxed;
use std::time::SystemTime;
use futures::stream::StreamExt;


/// The compile-time configs used by the tests here
const CONFIG: ConstConfig = ConstConfig {
    retrying_strategy: RetryingStrategies::DoNotRetry,
    ..ConstConfig::default()
};


/// The [CompositeSocketClient] may be used not only for long-running interactions, but for short ones as well.
/// For the long-runner usage, the methods [CompositeSocketClient::termination_waiter()] & [CompositeSocketClient::terminate()]
/// are provided -- for when the user code commands the client to terminate its operations, where any waiter is notified.
///
/// For the short-lived clients, we should be able to detect, as fast as possible, that the client ceased its activities --
/// for instance, when the server drops the connection or when the client processor code drops it.
///
/// Since the client logic is spawned in the background, the existing [CompositeSocketClient::termination_waiter()] also fits
/// this usage case.
///
/// The proof of completeness is:
///   1) Create a simple HTTP-talking processor, recording the time the answer was given
///   2) Connect to google.com and wait for termination, recording the time the client said it was done
///   3) Compare the times, ensuring they are in a narrow threshold -- something within 1ms
#[cfg_attr(not(doc), tokio::test)]
async fn terminates_immediately_when_done() {

    let threshold_micros = 100;  // maximum acceptable time, in µs, the client code has to inform the caller that the client completed its duties

    let mut client = new_socket_client!(CONFIG, "google.com", 80);

    // probes
    let client_reported_server_answer_time = Arc::new(AtomicU64::new(0));
    let client_reported_server_answer_time_ref = Arc::clone(&client_reported_server_answer_time);
    let peer_left_notification = Arc::new(AtomicBool::new(false));
    let peer_left_notification_ref = Arc::clone(&peer_left_notification);
    let local_service_termination_notification = Arc::new(AtomicBool::new(false));
    let local_service_termination_notification_ref = Arc::clone(&local_service_termination_notification);

    start_unresponsive_client_processor!(CONFIG, Atomic, client, TestString, TestString,
        move |event| {
            match event {
                ProtocolEvent::PeerArrived { peer } => {
                    let result = peer.send(TestString(String::from("GET / HTTP/1.0\n\n")));
                    assert!(result.is_ok(), "Unexpected error sending: {result:?}");
                },
                ProtocolEvent::PeerLeft{ peer: _, stream_stats: _ } => peer_left_notification_ref.store(true, Relaxed),
                ProtocolEvent::LocalServiceTermination => local_service_termination_notification_ref.store(true, Relaxed),
            }
            future::ready(())
        },
        move |_peer_addr, _port, peer, server_messages_stream| {
            let client_reported_server_answer_time_ref = Arc::clone(&client_reported_server_answer_time_ref);
            server_messages_stream.inspect(move |_server_message| {
                peer.cancel_and_close();
                client_reported_server_answer_time_ref.store(now_as_micros(), Relaxed);
            })
        }
    );

    client.termination_waiter()().await.expect("Error waiting for the client to finish");
    let termination_reported_time_diff = client_reported_server_answer_time.load(Relaxed).abs_diff(now_as_micros());

    assert!(termination_reported_time_diff <= threshold_micros,    "Client code took too long to report it has terminated its duties -- {termination_reported_time_diff}µs, above the acceptable threshold of {threshold_micros}µs");
    assert!(peer_left_notification.load(Relaxed),                  "`ProtocolEvent::PeerLeft` event wasn't fired");
    assert!(!local_service_termination_notification.load(Relaxed), "`ProtocolEvent::LocalServiceTermination` event was wrongly fired: no local code commanded a service termination");
}