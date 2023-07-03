#[path = "../common/mod.rs"] mod common;

pub mod protocol_processor;

use protocol_processor::ServerProtocolProcessor;
use common::protocol_model::ClientMessages;
use std::{future, sync::Arc, time::Duration};
use reactive_messaging::prelude::ProcessorRemoteStreamType;
use log::warn;


const LISTENING_INTERFACE: &str = "0.0.0.0";
const LISTENING_PORT:      u16  = 1234;


#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    simple_logger::SimpleLogger::new().with_utc_timestamps().init().unwrap_or_else(|_| eprintln!("--> LOGGER WAS ALREADY STARTED"));

    warn!("Ping-Pong server starting at {LISTENING_INTERFACE}:{LISTENING_PORT}");

    let server_processor_ref1 = Arc::new(ServerProtocolProcessor::new());
    let server_processor_ref2 = Arc::clone(&server_processor_ref1);

    let mut socket_server = reactive_messaging::SocketServer::new(LISTENING_INTERFACE.to_string(), LISTENING_PORT);

    socket_server.spawn_responsive_processor(
        move |connection_events| {
            server_processor_ref1.server_events_callback(connection_events);
            future::ready(())
        },
        move |client_addr, port, peer, client_messages_stream: ProcessorRemoteStreamType<ClientMessages>| {
            server_processor_ref2.dialog_processor(client_addr, port, peer, client_messages_stream)
        }
    ).await?;

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