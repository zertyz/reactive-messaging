//! Contains the functional requirements (+ tests) for the client

use std::error::Error;
use std::future;
use std::io::BufWriter;
use crate::utils::*;
use reactive_messaging::prelude::*;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;
use futures::stream::StreamExt;
use tokio::io::{AsyncWriteExt, BufReader, AsyncBufReadExt};
use tokio::net::TcpListener;
use tokio::task::yield_now;


/// The compile-time configs used by the tests here
const CONFIG: ConstConfig = ConstConfig {
    retrying_strategy: RetryingStrategies::DoNotRetry,
    ..ConstConfig::default()
};


/// Multi-protocol clients -- those adhering to the "Composite Protocol Stacking" design pattern, were implemented in a way in which
/// the logic processors may "believe" that connections that are "arriving" may mean a "fresh connection" and, conversely, connections
/// that are "leaving" (possibly due to an upgrade to another logic processor) may mean a "closed connection".
///
/// Obviously, neither is necessarily true, and having a fine control over the connections is necessary for user code to be able to
/// correctly maintain "sessions": failure to register a new session (upon a new connection) may break the logic; on the other hand,
/// failure to remove a session (upon disconnection) may lead to resource exhaustion and memory leaks.
///
/// With a carefully designed "logic processor", "protocol event handler" and "routing function", it is indeed possible to detect --
/// from the logic processor's point of view -- that a connection is really new (instead of an upgraded one). Unfortunately, detecting
/// the opposite case (when the connection is "leaving") is not so easy: was the connection upgraded to another logic processor, as
/// part of one of the possible branches of the protocol? Or the other end disconnected?
///
/// With the sole purpose to make it easier (for user code) to handle sessions, the Multi-protocol use case of the [CompositeSocketClient]
/// introduces a second event kind, not seen in the Single-protocol use case: the [ConnectionEvent] -- which comes in addition to the
/// [ProtocolEvent], shared among both use cases.
///
/// The setup for the test of this requirement is:
///   * A server that, when receives a number `i`, answers with `i+1`. The server may be configured to disconnect when receiving a particular `i`;
///   * Start a "Composite Protocol Stacking" client with `n` number of states/protocols, starting from 0. Each protocol logic is
///     simple: upgrades the state to whatever number the server says when we send our `id`, being the last id reserved for a "graceful
///     disconnection" **made by the client logic**.
///
/// The proof of completeness of this requirement is, therefore:
///   a) Graceful disconnections by the local party (the client): For `n` in `1..10`, with the server configured to never close the connection,
///     1) assert that the connection event happened once, before the first protocol processor realizes that a connection is arriving;
///     2) assert that every protocol processor was informed that a connection has arrived;
///     3) assert that the last protocol processor receives the event that the connection is leaving -- and no other;
///     4) assert that the disconnection event happens after the protocol's "connection is leaving" event.
///   b) Forceful disconnections by the remote party (the server): For `n` in `1..10`, with the server configured to close the connection at `n`,
///     1) idem
///     2) idem
///     3) idem -- but notice the client's logic should not disconnect
///     4) idem -- ibidem.
#[cfg_attr(not(doc), tokio::test(flavor = "multi_thread"))]
async fn distinct_connection_and_protocol_events() {

    const PORT: u16 = 8768;

    composite_protocol_client(26).await.expect("Couldn't call `composite_protocol_client(26)`");

    async fn composite_protocol_client(n_protocols: usize) -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut server = multi_protocol_server(PORT, None).await?;
        let mut client = new_composite_socket_client!(CONFIG, "127.0.0.1", PORT, usize);
        let mut handles = vec![];
        for protocol_id in 0..n_protocols {
            let handle = spawn_unresponsive_client_processor!(CONFIG, Atomic, client, TestString, TestString,
                move |event| future::ready(match event {
                    ProtocolEvent::PeerArrived { peer } => {
                        println!("PROTOCOL EVENT #{protocol_id}: arrived -- {peer:?}");
                        peer.send(TestString(format!("{protocol_id}"))).expect("Couldn't send");
                    },
                    ProtocolEvent::PeerLeft{ peer, stream_stats  } => {
                        println!("PROTOCOL EVENT #{protocol_id}: left -- {peer:?}");
                    },
                    ProtocolEvent::LocalServiceTermination => {
                        println!("PROTOCOL EVENT #{protocol_id}: Local Service Termination request reported");
                    },
                }),
//                 move |_, _, peer, server_stream| {
//                     server_stream
//                         .then(move |message| {
//                             let protocol_id = protocol_id.clone();
//                             let peer = peer.clone();
//                             async move {
// println!("  cc Client's protocol #{protocol_id} is reacting to the server message '{message}' and will progress to the suggested protocol");
//                                 let suggested_protocol = message.0.parse::<usize>().expect("Couldn't parse server response");
//                                 peer.set_state(suggested_protocol).await;
//                                 peer.send(TestString(message.0.clone())).expect("Couldn't send");
//                                 assert_eq!(peer.flush_and_close(Duration::from_secs(1)).await, 0, "Couldn't flush peer for protocol #{protocol_id} within 1 sec");
//                             }
//                     })
//                 }
                move |_, _, peer, server_stream| server_stream
                    // .take(1)
                    .map(move |message| {
println!("  cc Client's protocol #{protocol_id} is reacting to the server message '{message}' and will progress to the suggested protocol");
                        let suggested_protocol = message.0.parse::<usize>().expect("Couldn't parse server response");
                        assert!(peer.try_set_state(suggested_protocol), "State was locked for setting it");
                        peer.cancel_and_close();
                    })
            )?;
            handles.push(handle);
        }
        client.start_multi_protocol(0,
                                    move |socket_connection, is_reused| {
                                        let state = socket_connection.state();
                                        let handle = handles.get(*state).map(ConnectionChannel::clone_sender);
                                        println!("  .. <<routing closure>>: asked to route Connection for protocol #{state}. Is it present? {}", handle.is_some());
                                        handle
                                    },
                                    |event| future::ready(match event {
                                        ConnectionEvent::Connected(socket_connection) => println!("CONNECTION EVENT: Client is connected!"),
                                        ConnectionEvent::Disconnected(socket_connection) => println!("CONNECTION EVENT: Client is disconnected!"),
                                        ConnectionEvent::LocalServiceTermination => println!("CONNECTION EVENT: Local Service Termination request reported"),
                                    })).await
            .expect("Couldn't start the composite client");

        // shutdown client & server
        let wait_for_client_termination = client.termination_waiter();
        let wait_for_server_termination = server.termination_waiter();
        wait_for_client_termination().await.expect("Couldn't wait for the client to finish");
        server.terminate().await.expect("Couldn't terminate the server");
        wait_for_server_termination().await.expect("Couldn't wait for the server to finish");

        // assert
        todo!();

        Ok(())
    }

}


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
    let (probed_protocol_events_handler,
         _last_peer_arrived_notification_micros,
         last_peer_left_notification_micros,
         last_local_service_termination_notification_micros) = last_micros_probed_protocol_events_handler();
    let (probed_protocol_processor_builder,
         last_server_message_micros) = last_micros_probed_protocol_processor_builder();

    start_unresponsive_client_processor!(CONFIG, Atomic, client, TestString, TestString,
        move |event| {
            if let ProtocolEvent::PeerArrived { peer } = &event {
                let result = peer.send(TestString(String::from("GET / HTTP/1.0\n\n")));
                assert!(result.is_ok(), "Unexpected error sending: {result:?}");
            }
            probed_protocol_events_handler(event)
        },
        move |remote_addr, port, peer, server_messages_stream| {
            probed_protocol_processor_builder(server_messages_stream)
                .inspect(move |_server_message| {
                    peer.cancel_and_close();
                })
        }
    );

    client.termination_waiter()().await.expect("Error waiting for the client to finish");
    let termination_reported_time_diff = last_server_message_micros.load(Relaxed).abs_diff(now_as_micros());

    assert!(termination_reported_time_diff <= threshold_micros,                    "Client code took too long to report it has terminated its duties -- {termination_reported_time_diff}µs, above the acceptable threshold of {threshold_micros}µs");
    assert!(last_peer_left_notification_micros.load(Relaxed) > 0,                  "`ProtocolEvent::PeerLeft` event wasn't fired");
    assert!(last_local_service_termination_notification_micros.load(Relaxed) <= 0, "`ProtocolEvent::LocalServiceTermination` event was wrongly fired: no local code commanded a service termination");
}


/// Our specially crafted text-based "mock" server suitable for Multi-protocol tests:
/// receives a number, followed by `\n` and answers with number+1, also followed by `\n`.\
/// If `disconnect_at` is given, it will drop the connection as soon as that number is received.
async fn multi_protocol_server(port: u16, mut disconnect_at: Option<u16>) -> Result<impl MessagingService<{CONFIG.into()}>, Box<dyn Error + Send + Sync>> {
    let mut server = new_socket_server!(CONFIG, "127.0.0.1", port);
    start_responsive_server_processor!(CONFIG, Atomic, server, TestString, TestString,
        |_| future::ready(()),
        move |_, _, peer, client_messages_stream| {
            let disconnect_at = disconnect_at.clone();
            client_messages_stream
                .map(|client_message| {
                    client_message.0.parse::<u16>()
                        .map(|number| (Some(number), format!("{}", number + 1)))
                        .unwrap_or_else(|err| (None, format!("ERROR: error parsing input '{client_message}' as a `u16` number: {err}")))
                })
.inspect(|(input_number, server_answer)| println!("  ss Server received {input_number:?} and will answer '{server_answer}'"))
                .take_while(move |(input_number, _server_answer)| future::ready(input_number != &disconnect_at))
                .map(|(input_number, server_answer)| TestString(server_answer))
        }
    )?;
    Ok(server)
}