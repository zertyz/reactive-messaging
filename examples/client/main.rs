#[path = "../common/mod.rs"] mod common;

mod protocol_processor;

use common::protocol_model::{ClientMessages, ServerMessages};
use crate::common::logic::ping_pong_models::MatchConfig;
use crate::protocol_processor::ClientProtocolProcessor;
use reactive_messaging::prelude::ProcessorRemoteStreamType;
use reactive_messaging::ron_serializer;
use std::future;
use std::ops::Deref;
use std::sync::Arc;
use std::time::Duration;
use futures::{Stream, StreamExt};


const SERVER_IP: &str = "127.0.0.1";
const PORT:      u16  = 1234;
const INSTANCES: u16  = 1;

#[cfg(debug_assertions)]
const DEBUG: bool = true;
#[cfg(not(debug_assertions))]
const DEBUG: bool = false;



#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

    simple_logger::SimpleLogger::new().with_utc_timestamps().init().unwrap_or_else(|_| eprintln!("--> LOGGER WAS ALREADY STARTED"));

    println!("{INSTANCES} Ping-Pong client(s) starting... connecting to {SERVER_IP}:{PORT}");

    let mut socket_clients = vec![];
    for _ in 1..=INSTANCES {

        let client_processor_ref1 = Arc::new(ClientProtocolProcessor::new());
        let client_processor_ref2 = Arc::clone(&client_processor_ref1);

        let mut socket_client = reactive_messaging::SocketClient::spawn_responsive_processor(SERVER_IP.to_string(), PORT,
            move |connection_event| {
                client_processor_ref1.client_events_callback(connection_event);
                future::ready(())
            },
            move |client_addr, port, peer, server_messages_stream: ProcessorRemoteStreamType<ServerMessages>| {
                let mut debug_serializer_buffer = Vec::<u8>::with_capacity(2048);
                let server_messages_stream = server_messages_stream
                    .inspect(move |server_message| {
                        if DEBUG {
                            ron_serializer(server_message.deref(), &mut debug_serializer_buffer)
                                .expect("`ron_serializer()` of our `ServerMessages`");
                            print!("<<<< {}", String::from_utf8(debug_serializer_buffer.clone()).expect("Ron should be utf-8"))
                        }
                    });
                let mut debug_serializer_buffer = Vec::<u8>::with_capacity(2048);
                let processor_stream = client_processor_ref2.dialog_processor(client_addr, port, peer, server_messages_stream)
                    .inspect(move |client_message| {
                        if DEBUG {
                            ron_serializer(client_message, &mut debug_serializer_buffer)
                                .expect("`ron_serializer()` of the received `ClientMessages`");
                            print!(">>>> {}", String::from_utf8(debug_serializer_buffer.clone()).expect("Ron should be utf-8"))
                        }
                    });
                processor_stream
            }
        ).await?;

        socket_clients.push(socket_client);
    }

    tokio::time::sleep(Duration::from_secs(180)).await;
    socket_clients.into_iter().for_each(|socket_client| socket_client.shutdown().expect("FAILED TO SHUTDOWN THE CLIENT"));

    Ok(())
}
