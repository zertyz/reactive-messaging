//! The simplest possible client/server implementation using the `reactive-messaging` library.
//!
//! # The (single) Protocol:
//!
//! The server accepts new connections quietly until the client says `Hello`.
//! Then the server answers with `World(name)`, where `name` comes from a constant value
//! and the client disconnects after printing it loud.

use reactive_messaging::prelude::*;
use futures::stream::StreamExt;
use std::ops::Deref;
use std::error::Error;
use std::{env, future};
use std::time::Duration;
use serde::{Deserialize, Serialize};


const LISTENING_INTERFACE: &str = "127.0.0.1";
const PORT: u16 = 1234;
const CONFIG: ConstConfig = ConstConfig {
    retrying_strategy: RetryingStrategies::DoNotRetry,
    ..ConstConfig::default()
};
const WORLD: &str = "Earth";


/// The messages issued by the client
#[derive(Debug,PartialEq,Serialize,Deserialize,Default)]
enum ClientMessages {
    #[default]
    Hello,
    Error(String)
}

/// The messages issued by the server
#[derive(Debug,PartialEq,Serialize,Deserialize,Default)]
enum ServerMessages {
    World(String),
    #[default]
    NoAnswer,
    Error(String),
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("Welcome to the `reactive-messaging` Hello World example");
    let possible_options = vec!["server-only", "client-only"];
    println!("Usage: hello_world [{}]", possible_options.iter().fold(String::new(), |mut acc, item| { if acc.len() > 0 {acc.push('|')}; acc.push_str(item); acc } ));

    let args: Vec<String> = env::args().collect();
    if args.len() > 2 {
        return Err(Box::from(String::from("This program takes a single optional argument")))
    }
    let (start_server, start_client) = args.get(1)
        .map(|arg| if arg == possible_options[0] {
            (true, false)
        } else if arg == possible_options[1] {
            (false, true)
        } else {
            panic!("Unknown argument '{arg}'")
        })
        .unwrap_or((true, true));
    logic(start_server, start_client).await
}

async fn logic(start_server: bool, start_client: bool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut server = None;
    if start_server {
        println!("==> starting the server");
        let server = server.insert(new_socket_server!(CONFIG, LISTENING_INTERFACE, PORT));
        let server_processor_handler = spawn_responsive_server_processor!(CONFIG, Atomic, server, ClientMessages, ServerMessages,
            |_| future::ready(()),
            |_, _, _, client_stream| client_stream
                .inspect(|client_message| println!(">>> {:?}", client_message.deref()))
                .map(|client_message| match *client_message {
                    ClientMessages::Hello => ServerMessages::World(WORLD.to_string()),
                    _ => ServerMessages::NoAnswer,
                })
        )?;
        server.start_with_single_protocol(server_processor_handler).await?;
    }

    if start_client {
        println!("==> starting the client");
        let mut client = new_socket_client!(CONFIG, LISTENING_INTERFACE, PORT);
        start_unresponsive_client_processor!(CONFIG, Atomic, client, ServerMessages, ClientMessages,
            |connection_event| async {
                match connection_event {
                    ConnectionEvent::PeerConnected { peer } => {
                        // sends a message as soon as the connection is established
                        peer.send_async(ClientMessages::Hello).await
                            .expect("The client couldn't send a message");
                    }
                    _ => {},
                }
            },
            |_, _, peer, server_stream| server_stream.inspect(move |server_message| {
                // on any received message from the server, disconnects without answering nothing
                println!("<<< {:?}", server_message.deref());
                peer.cancel_and_close();
            })
        ).map_err(|err| format!("{err}. Did you start a local server (on another window) with `nc -vvvv -l -p 1234`?"))?;
        let client_waiter = client.termination_waiter();

        println!("==> waiting for the client to communicate");
        // wait for the client to perform its duty
        client_waiter().await?;

        if let Some(mut server) = server {
            println!("==> client is done; asking the server to terminate");
            let server_waiter = server.termination_waiter();
            server.terminate().await?;
            server_waiter().await?;
            println!("==> server ended. Goodbye");
        }
    } else if let Some(server) = server {
        println!("Server is running without a client. Wait 3 minutes or press CTRL-C. On another terminal, run `nc -vvvv localhost 1234` and act as a client");
        tokio::time::sleep(Duration::from_secs(60*3)).await;
        server.terminate().await?
    }
    Ok(())
}


// ClientMessages SerDe
///////////////////////
impl ReactiveMessagingSerializer<ClientMessages> for ClientMessages {
    fn serialize(local_message: &ClientMessages, buffer: &mut Vec<u8>) {
        ron_serializer(local_message, buffer)
            .expect("BUG in `ReactiveMessagingSerializer<ClientMessages>`. Is the buffer too small?");
    }
    fn processor_error_message(err: String) -> ClientMessages {
        ClientMessages::Error(err)
    }
}
impl ReactiveMessagingDeserializer<ClientMessages> for ClientMessages {
    fn deserialize(remote_message: &[u8]) -> Result<ClientMessages, Box<dyn Error + Sync + Send>> {
        ron_deserializer(remote_message)
    }
}


// ServerMessages SerDe
///////////////////////
impl ReactiveMessagingSerializer<ServerMessages> for ServerMessages {
    fn serialize(local_message: &ServerMessages, buffer: &mut Vec<u8>) {
        ron_serializer(local_message, buffer)
            .expect("BUG in `ReactiveMessagingSerializer<ServerMessages>`. Is the buffer too small?");
    }
    fn processor_error_message(err: String) -> ServerMessages {
        ServerMessages::Error(err)
    }
}
impl ReactiveMessagingDeserializer<ServerMessages> for ServerMessages {
    fn deserialize(remote_message: &[u8]) -> Result<ServerMessages, Box<dyn Error + Sync + Send>> {
        ron_deserializer(remote_message)
    }
}
impl ResponsiveMessages<ServerMessages> for ServerMessages {
    fn is_disconnect_message(_processor_answer: &ServerMessages) -> bool {
        false
    }
    fn is_no_answer_message(_processor_answer: &ServerMessages) -> bool {
        false
    }
}