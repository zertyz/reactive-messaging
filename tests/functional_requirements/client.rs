//! Contains the functional requirements (+ tests) for the client

use std::error::Error;
use std::future;
use std::io::BufWriter;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize};
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

    composite_protocol_client(26, None).await.expect("Couldn't perform `composite_protocol_client(26, None)` -- with disconnection started by the Client");
    composite_protocol_client(26, Some(25)).await.expect("Couldn't perform `composite_protocol_client(26, Some(25))` -- with disconnection started by the Server");

    async fn composite_protocol_client(n_client_protocols: usize, server_disconnect_at: Option<u16>) -> Result<(), Box<dyn Error + Send + Sync>> {

        let case_builder = move || format!("Client: {n_client_protocols} protocols; Server: disconnect at {server_disconnect_at:?}");
        let case = case_builder();

        /// When each Connection Event happens, the corresponding value will be `true`
        struct ClientConnectionDatum {
            connected: AtomicBool,
            disconnected: AtomicBool,
            termination: AtomicBool,
        }

        /// When each Protocol Event happens, the corresponding value will be incremented by `protocol_id`
        struct ClientProtocolDatum {
            arrived: AtomicUsize,
            left: AtomicUsize,
            termination: AtomicUsize,
            message: AtomicUsize,
        }

        let mut server = multi_protocol_server(PORT, server_disconnect_at).await?;
        let mut client = new_composite_socket_client!(CONFIG, "127.0.0.1", PORT, usize);
        let mut handles = vec![];
        let mut client_protocol_data = vec![];
        let routing_sum = Arc::new(AtomicUsize::new(0));
        let routing_sum_ref = Arc::clone(&routing_sum);
        let client_connection_datum = Arc::new(ClientConnectionDatum {
            connected: AtomicBool::new(false),
            disconnected: AtomicBool::new(false),
            termination: AtomicBool::new(false),
        });
        let client_connection_datum_ref = Arc::clone(&client_connection_datum);
        for protocol_id in 0..n_client_protocols {
            let client_protocol_datum_ref1 = Arc::new(ClientProtocolDatum {
                arrived: AtomicUsize::new(0),
                left: AtomicUsize::new(0),
                termination: AtomicUsize::new(0),
                message: AtomicUsize::new(0),
            });
            let client_protocol_datum_ref2 = Arc::clone(&client_protocol_datum_ref1);
            client_protocol_data.push(Arc::clone(&client_protocol_datum_ref1));
            let handle = spawn_unresponsive_client_processor!(CONFIG, Atomic, client, TestString, TestString,
                move |event| future::ready(match event {
                    ProtocolEvent::PeerArrived { peer } => {
                        client_protocol_datum_ref1.arrived.fetch_add(protocol_id+1, Relaxed);
                        peer.send(TestString(format!("{protocol_id}"))).expect("{case}: Couldn't send");
                        println!("PROTOCOL EVENT #{protocol_id}: arrived -- {peer:?}");
                    },
                    ProtocolEvent::PeerLeft{ peer, stream_stats  } => {
                        client_protocol_datum_ref1.left.fetch_add(protocol_id+1, Relaxed);
                        println!("PROTOCOL EVENT #{protocol_id}: left -- {peer:?}");
                    },
                    ProtocolEvent::LocalServiceTermination => {
                        client_protocol_datum_ref1.termination.fetch_add(protocol_id+1, Relaxed);
                        println!("PROTOCOL EVENT #{protocol_id}: Local Service Termination request reported");
                    },
                }),
                move |_, _, peer, mut server_stream| {
                    let client_protocol_datum_ref2 = Arc::clone(&client_protocol_datum_ref2);
                    let case = case_builder();
                    server_stream
                        .take(1)
                        .map(move |message| {
                            client_protocol_datum_ref2.message.fetch_add(protocol_id+1, Relaxed);
                            println!("  cc Client's protocol #{protocol_id} is reacting to the server message '{message}' and will progress to the suggested protocol");
                            let suggested_protocol = message.0.parse::<usize>().expect("Couldn't parse server response");
                            assert!(peer.try_set_state(suggested_protocol), "State was locked for setting it");
                        })
                    }
            )?;
            handles.push(handle);
        }
        client.start_multi_protocol(0,
                                    move |socket_connection, is_reused| {
                                        let state = socket_connection.state();
                                        routing_sum_ref.fetch_add(state+1, Relaxed);
                                        let handle = handles.get(*state).map(ConnectionChannel::clone_sender);
                                        println!("  .. <<routing closure>>: asked to route ({}) Connection for protocol #{state}. Is it present? {}",
                                                 if socket_connection.closed() { "closed" } else { "still opened" },
                                                 handle.is_some());
                                        handle
                                    },
                                    move |event| future::ready(match event {
                                        ConnectionEvent::Connected(_socket_connection) => {
                                            client_connection_datum_ref.connected.store(true, Relaxed);
                                            println!("CONNECTION EVENT: Client is connected!");
                                        },
                                        ConnectionEvent::Disconnected(_socket_connection) => {
                                            client_connection_datum_ref.disconnected.store(true, Relaxed);
                                            println!("CONNECTION EVENT: Client is disconnected!");
                                        },
                                        ConnectionEvent::LocalServiceTermination => {
                                            client_connection_datum_ref.termination.store(true, Relaxed);
                                            println!("CONNECTION EVENT: Local Service Termination request reported");
                                        },
                                    })).await
            .expect("{case}: Couldn't start the composite client");

        // shutdown client & server
        let wait_for_client_termination = client.termination_waiter();
        let wait_for_server_termination = server.termination_waiter();
        wait_for_client_termination().await.expect("{case}: Couldn't wait for the client to finish");
        server.terminate().await.expect("{case}: Couldn't terminate the server");
        wait_for_server_termination().await.expect("{case}: Couldn't wait for the server to finish");

        // connection routing assertions
        let expected_routing_sum = server_disconnect_at.map(|server_disconnect_at| server_disconnect_at as usize)
            // when the server commands the disconnection, the sum is of numbers from 1 to `server_disconnect_at+1`
            .map(|server_disconnect_at| (server_disconnect_at+1)*(server_disconnect_at+2)/2)
            // otherwise, the sum is of numbers from 1 to `n_protocols+1`, as the client routing will be called 1 more time (and should return None for the disconnection to take place)
            .unwrap_or((n_client_protocols+1)*(n_client_protocols+2)/2);
        assert_eq!(routing_sum.load(Relaxed), expected_routing_sum, "{case}: Some incorrect routing took place");
        // Connection event assertions
        assert!(client_connection_datum.connected.load(Relaxed), "{case}: `ConnectionEvent::Connected` didn't happen");
        assert!(client_connection_datum.disconnected.load(Relaxed), "{case}: `ConnectionEvent::Disconnected` didn't happen");
        assert!(!client_connection_datum.termination.load(Relaxed), "{case}: `ConnectionEvent::LocalServiceTermination` shouldn't have happened");
        // Protocol event assertions
        let n_protocols = server_disconnect_at.unwrap_or(n_client_protocols as u16);
        for protocol_id in 0..n_protocols as usize {
            assert_eq!(client_protocol_data[protocol_id].arrived.load(Relaxed),     protocol_id+1, "{case}: protocol #{protocol_id}: `ProtocolEvent::PeerArrived` didn't happen as expected");
            assert_eq!(client_protocol_data[protocol_id].left.load(Relaxed),        protocol_id+1, "{case}: protocol #{protocol_id}: `ProtocolEvent::PeerLeft` didn't happen as expected");
            assert_eq!(client_protocol_data[protocol_id].termination.load(Relaxed), 0,             "{case}: protocol #{protocol_id}: `ProtocolEvent::LocalServiceTermination` did happen -- and this was unexpected");
            assert_eq!(client_protocol_data[protocol_id].message.load(Relaxed),     protocol_id+1, "{case}: protocol #{protocol_id}: a message wasn't received (as expected)");
        }

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
                .inspect(move |(input_number, server_answer)| println!("  ss Server received {input_number:?} and will {}",
                                                                                            if input_number != &disconnect_at { format!("answer '{server_answer}'") } else { String::from("disconnect") } ))
                .take_while(move |(input_number, _server_answer)| future::ready(input_number != &disconnect_at))
                .map(|(input_number, server_answer)| TestString(server_answer))
        }
    )?;
    Ok(server)
}