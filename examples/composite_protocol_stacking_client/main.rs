#[path = "../composite_protocol_stacking_common/mod.rs"] mod composite_protocol_stacking_common;

mod protocol_processor;

use composite_protocol_stacking_common::protocol_model::{GameServerMessages, GameClientMessages};
use crate::protocol_processor::ClientProtocolProcessor;
use std::{
    future,
    sync::Arc,
    time::Duration,
};
use reactive_messaging::prelude::*;
use futures::StreamExt;
use log::warn;
use crate::composite_protocol_stacking_common::protocol_model::{PreGameClientMessages, PreGameServerMessages, ProtocolStates};


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
        let client_processor_ref3 = Arc::clone(&client_processor_ref1);
        let client_processor_ref4 = Arc::clone(&client_processor_ref1);

        let mut socket_client = new_composite_socket_client!(NETWORK_CONFIG, SERVER_IP, PORT, ProtocolStates);
        // pre-game protocol processor
        let pre_game_processor = spawn_client_processor!(NETWORK_CONFIG, Atomic, socket_client, PreGameServerMessages, PreGameClientMessages,
            move |connection_event| {
                client_processor_ref1.pre_game_connection_events_handler(connection_event);
                future::ready(())
            },
            move |client_addr, port, peer, server_messages_stream| {
                let mut debug_serializer_buffer = Vec::<u8>::with_capacity(2048);
                let server_messages_stream = server_messages_stream
                    .inspect(move |server_message| {
                        if DEBUG {
                            ron_serializer(server_message.as_ref(), &mut debug_serializer_buffer)
                                .expect("`ron_serializer()` of our `ServerMessages`");
                            println!("<<<< (PRE-GAME) {}", String::from_utf8(debug_serializer_buffer.clone()).expect("Ron should be utf-8"))
                        }
                    });
                let mut debug_serializer_buffer = Vec::<u8>::with_capacity(2048);
                // processor stream
                client_processor_ref2.pre_game_dialog_processor(client_addr.clone(), port, peer.clone(), server_messages_stream)
                    .inspect(move |client_message| {
                        if DEBUG {
                            ron_serializer(client_message, &mut debug_serializer_buffer)
                                .expect("`ron_serializer()` of the received `ClientMessages`");
                            println!(">>>> (PRE-GAME) {}", String::from_utf8(debug_serializer_buffer.clone()).expect("Ron should be utf-8"))
                        }
                    })
            }
        )?;
        // game protocol processor
        let game_processor = spawn_client_processor!(NETWORK_CONFIG, Atomic, socket_client, GameServerMessages, GameClientMessages,
            move |connection_event| {
                client_processor_ref3.game_connection_events_handler(connection_event);
                future::ready(())
            },
            move |client_addr, port, peer, server_messages_stream| {
                let mut debug_serializer_buffer = Vec::<u8>::with_capacity(2048);
                let server_messages_stream = server_messages_stream
                    .inspect(move |server_message| {
                        if DEBUG {
                            ron_serializer(server_message.as_ref(), &mut debug_serializer_buffer)
                                .expect("`ron_serializer()` of our `ServerMessages`");
                            println!("<<<< (GAME) {}", String::from_utf8(debug_serializer_buffer.clone()).expect("Ron should be utf-8"))
                        }
                    });
                let mut debug_serializer_buffer = Vec::<u8>::with_capacity(2048);
                // processor stream
                client_processor_ref4.game_dialog_processor(client_addr.clone(), port, peer.clone(), server_messages_stream)
                    .inspect(move |client_message| {
                        if DEBUG {
                            ron_serializer(client_message, &mut debug_serializer_buffer)
                                .expect("`ron_serializer()` of the received `ClientMessages`");
                            println!(">>>> (GAME) {}", String::from_utf8(debug_serializer_buffer.clone()).expect("Ron should be utf-8"))
                        }
                    })
            }
        )?;
        socket_client.start_multi_protocol(ProtocolStates::PreGame, move |socket_connection: &SocketConnection<ProtocolStates>, _|
            match socket_connection.state() {
                ProtocolStates::PreGame    => Some(pre_game_processor.clone_sender()),
                ProtocolStates::Game       => Some(game_processor.clone_sender()),
                ProtocolStates::Disconnect => None,
            },
           |_| future::ready(())
        ).await?;

        socket_clients.push(socket_client);
    }

    tokio::time::sleep(Duration::from_secs(290)).await;
    for socket_client in socket_clients.into_iter() {
        socket_client.terminate().await.expect("FAILED TO SHUTDOWN THE CLIENT")
    }

    Ok(())
}
