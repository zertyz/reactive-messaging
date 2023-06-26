#[path = "../common/mod.rs"] mod common;

use std::cell::UnsafeCell;
use std::future;
use std::ops::Deref;
use std::sync::Arc;
use std::time::Duration;
use common::protocol_model::{ClientMessages, ServerMessages};
use reactive_messaging;
use futures::{Stream, StreamExt};
use reactive_messaging::{ConnectionEvent, ron_serializer};
use crate::common::logic::ping_pong_models::{FaultEvents, GameOverStates, GameStates, MatchConfig, PingPongEvent, PlayerAction, Players, TurnFlipEvents};
use crate::common::logic::ping_pong_logic::{act, Umpire};
use dashmap::DashMap;
use tokio::sync::{Mutex, MutexGuard};
use log::{info,warn,error};
use reactive_messaging::prelude::ProcessorRemoteStreamType;
use crate::common::logic::protocol_processor::ServerProtocolProcessor;


const LISTENING_INTERFACE: &str = "0.0.0.0";
const LISTENING_PORT:      u16  = 1234;


#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    simple_logger::SimpleLogger::new().with_utc_timestamps().init().unwrap_or_else(|_| eprintln!("--> LOGGER WAS ALREADY STARTED"));

    println!("Ping-Pong server starting at {LISTENING_INTERFACE}:{LISTENING_PORT}");
    println!("Try pasting this into nc -vvvv localhost 9758:");
    let client_message_config = ClientMessages::Config(MatchConfig {
        score_limit:            3,
        rally_timeout_millis:   1000,
        no_bounce_probability:  0.01,
        no_rebate_probability:  0.02,
        mishit_probability:     0.03,
        pre_bounce_probability: 0.04,
        net_touch_probability:  0.05,
        net_block_probability:  0.06,
        ball_out_probability:   0.07,
    });
    print!("{}", ron_serializer(&client_message_config));
    let service = ClientMessages::PingPongEvent(PingPongEvent::TurnFlip { player_action: PlayerAction { lucky_number: 0.15 }, resulting_event: TurnFlipEvents::SuccessfulService });
    print!("{}", ron_serializer(&service));
    println!();

    let server_processor_ref1 = Arc::new(ServerProtocolProcessor::new());
    let server_processor_ref2 = Arc::clone(&server_processor_ref1);

    let mut socket_server = reactive_messaging::SocketServer::new(LISTENING_INTERFACE.to_string(), LISTENING_PORT);

    let start_server = socket_server.responsive_starter(
        move |connection_events| {
            server_processor_ref1.server_events_callback(connection_events);
            future::ready(())
        },
        move |client_addr, port, peer, client_messages_stream: ProcessorRemoteStreamType<ClientMessages>| {
            server_processor_ref2.dialog_processor(client_addr, port, peer, client_messages_stream)
        }
    );
    start_server().await?;

    let wait_for_shutdown = socket_server.shutdown_waiter();
tokio::spawn( async move {
    tokio::time::sleep(Duration::from_secs(300)).await;
    socket_server.shutdown().expect("FAILED TO SHUTDOWN");
});
    wait_for_shutdown().await?;

    Ok(())
    //let (processor_stream, stream_producer, stream_closer) = socket_server.set_processor()

    // socket server should allow me to inquire some executor/stream metrics (from reactive-mutiny) and should allow for clean shutdowns
    // conversely, there should be a SocketClient class, also receiving a host, port, workers? a processor and a sender?? Yes, a sending facility, through the `SocketClient` instance

    // the socket server uses a Uni to create a producer/stream pair -- the socket handling code will read from the socket and "produce" to the Uni; the executor will consume from the Stream
    // (all connected clients on a single Uni? Yes! Add more listeners for distribution, if that makes sense)

    // this library might consider: stateless server & stateful server -- on the stateful server processor, one stream processor will be created for each client (and will allow custom "authentication" routines to create a "session")
    // This "session" object is, then, passed to the processor creating pipeline builder. So, this internal manager is expected to hold the Stream instances in a container that is faster than anything else (is there an atomic Hashmap available?)

    // --------------------

    // today a task is already spawned for each client, at which point the state may be created -- so we don't need that stateful / stateless distinction: we just need a refactoring:
    // we must pass in a function that will generate a "producer" when requested -- and that producer may hold the state
    // the producer is any struct implementing a trait (from reactive-mutiny?)

    // but the state need to be on the stream processing function -- and not on the stream producer...
    // so we must devise a "builder" function that will return a producer & stream processor fn, which (the fn) holds the state. Can this work? Integration test? Example?

    // fn pipeline_builder(client_addr: String, connected_port: u16) -> impl Stream<Item=ItemType>

    // fn pipeline_builder(client_addr: String, connected_port: u16) -> MutinyStream<ItemType>
    // the `MutinyStream`, itself, will be created from a pipeline builder itself, which receives impl Stream<Item=Input> -> impl Stream<Item=Output>

    // therefore:
    // fn pipeline_builder(client_addr: String, _connected_port: u16, incoming_messages_stream: impl Stream<Item=InputItemType>) -> impl Stream<Item=OutputItemType>
    // and the SocketServer::new() receives it



}