#[path = "../composite_protocol_stacking_common/mod.rs"] mod composite_protocol_stacking_common;

mod protocol_processor;

use composite_protocol_stacking_common::protocol_model::{ServerMessages,ClientMessages};
use crate::protocol_processor::ClientProtocolProcessor;
use std::{
    future,
    sync::Arc,
    time::Duration,
};
use reactive_messaging::prelude::*;
use futures::StreamExt;
use log::warn;


const SERVER_IP:      &str        = "127.0.0.1";
const PORT:           u16         = 1234;
const INSTANCES:      u16         = 2;
const NETWORK_CONFIG: ConstConfig = ConstConfig {
    ..ConstConfig::default()
};


#[cfg(debug_assertions)]
const DEBUG: bool = true;
#[cfg(not(debug_assertions))]
const DEBUG: bool = false;


#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

    simple_logger::SimpleLogger::new().with_utc_timestamps().init().unwrap_or_else(|_| eprintln!("--> LOGGER WAS ALREADY STARTED"));

    warn!("{INSTANCES} Ping-Pong client(s) starting... connecting to {SERVER_IP}:{PORT}");

    let mut socket_clients = vec![];
    for _ in 1..=INSTANCES {

        let client_processor_ref1 = Arc::new(ClientProtocolProcessor::new());
        let client_processor_ref2 = Arc::clone(&client_processor_ref1);

        let mut socket_client = new_socket_client!(NETWORK_CONFIG, SERVER_IP, PORT, ServerMessages, ClientMessages);
        start_responsive_client_processor!(socket_client,
            move |connection_event| {
                client_processor_ref1.client_events_callback(connection_event);
                future::ready(())
            },
            move |client_addr, port, peer, server_messages_stream| {
                let mut debug_serializer_buffer = Vec::<u8>::with_capacity(2048);
                let server_messages_stream = server_messages_stream
                    .inspect(move |server_message| {
                        if DEBUG {
                            ron_serializer(server_message.as_ref(), &mut debug_serializer_buffer)
                                .expect("`ron_serializer()` of our `ServerMessages`");
                            println!("<<<< {}", String::from_utf8(debug_serializer_buffer.clone()).expect("Ron should be utf-8"))
                        }
                    });
                let mut debug_serializer_buffer = Vec::<u8>::with_capacity(2048);
                // processor stream
                client_processor_ref2.dialog_processor(client_addr, port, peer, server_messages_stream)
                    .inspect(move |client_message| {
                        if DEBUG {
                            ron_serializer(client_message, &mut debug_serializer_buffer)
                                .expect("`ron_serializer()` of the received `ClientMessages`");
                            println!(">>>> {}", String::from_utf8(debug_serializer_buffer.clone()).expect("Ron should be utf-8"))
                        }
                    })
            }
        )?;

        socket_clients.push(socket_client);
    }

    tokio::time::sleep(Duration::from_secs(290)).await;
    socket_clients.into_iter().for_each(|socket_client| socket_client.terminate().expect("FAILED TO SHUTDOWN THE CLIENT"));

    Ok(())
}
