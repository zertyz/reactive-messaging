#[path = "../composite_protocol_stacking_common/mod.rs"] mod composite_protocol_stacking_common;

pub mod server_protocol_processor;

use composite_protocol_stacking_common::{
    NETWORK_CONFIG,
    protocol_model::{GameClientMessages, GameServerMessages}
};
use server_protocol_processor::ServerProtocolProcessor;
use std::{
    future,
    sync::Arc,
    time::Duration,
};
use reactive_messaging::prelude::*;
use log::warn;
use crate::composite_protocol_stacking_common::protocol_model::{PreGameClientMessages, PreGameServerMessages, ProtocolStates};


const LISTENING_INTERFACE: &str = "0.0.0.0";
const LISTENING_PORT:      u16  = 443;


#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    simple_logger::SimpleLogger::new().with_utc_timestamps().init().unwrap_or_else(|_| eprintln!("--> LOGGER WAS ALREADY STARTED"));

    warn!("Ping-Pong server starting at {LISTENING_INTERFACE}:{LISTENING_PORT}");

    let server_processor_ref1 = Arc::new(ServerProtocolProcessor::new());
    let server_processor_ref2 = Arc::clone(&server_processor_ref1);
    let server_processor_ref3 = Arc::clone(&server_processor_ref1);
    let server_processor_ref4 = Arc::clone(&server_processor_ref1);

    let mut socket_server = new_composite_socket_server!(NETWORK_CONFIG, LISTENING_INTERFACE, LISTENING_PORT, ProtocolStates);
    // pre-game protocol processor
    let pre_game_processor = spawn_server_processor!(NETWORK_CONFIG, Textual, Atomic, socket_server, PreGameClientMessages, PreGameServerMessages,
        move |connection_event| {
            server_processor_ref1.pre_game_connection_events_handler(connection_event);
            future::ready(())
        },
        move |client_addr, port, peer, client_messages_stream| server_processor_ref2.pre_game_dialog_processor(client_addr, port, peer, client_messages_stream)
    )?;
    // game protocol processor
    let game_processor = spawn_server_processor!(NETWORK_CONFIG, MmapBinary, Atomic, socket_server, GameClientMessages, GameServerMessages,
        move |connection_event| {
            let server_processor = server_processor_ref3.clone();
            async move {
                server_processor.game_connection_events_handler(connection_event).await;
            }
        },
        move |client_addr, port, peer, client_messages_stream| server_processor_ref4.game_dialog_processor(client_addr, port, peer, client_messages_stream)
    )?;
    socket_server.start_multi_protocol(ProtocolStates::PreGame, move |socket_connection: &SocketConnection<ProtocolStates>, _|
        match socket_connection.state() {
            ProtocolStates::PreGame    => Some(pre_game_processor.clone_sender()),
            ProtocolStates::Game       => Some(game_processor.clone_sender()),
            ProtocolStates::Disconnect => None,
        },
        |_| future::ready(())
    ).await?;

    let wait_for_termination = socket_server.termination_waiter();
    tokio::spawn( async move {
        tokio::time::sleep(Duration::from_secs(86400)).await;
        socket_server.terminate().await.expect("Failed to Terminate the server");
    });
    wait_for_termination().await?;

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