//! The simplest possible client/server implementation using the `reactive-messaging` library.
//!
//! # The (single) Protocol:
//!
//! The server accepts new connections quietly until the client says `Hello`.
//! Then the server answers with `World("name")`, where `name` comes from a constant value
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
#[derive(Debug, PartialEq, Serialize, Deserialize, Default, rkyv::Serialize, rkyv::Deserialize, rkyv::Archive)]
#[archive_attr(derive(Debug, PartialEq))]
enum ClientMessages {
    #[default]
    Hello,
    Error(String)
}

/// The messages issued by the server
#[derive(Debug, PartialEq, Serialize, Deserialize, Default, rkyv::Serialize, rkyv::Deserialize, rkyv::Archive)]
#[archive_attr(derive(Debug, PartialEq))]
enum ServerMessages {
    World(String),
    #[default]
    NoAnswer,
    Error(String),
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    println!("Welcome to the `reactive-messaging` Hello World example");
    let possible_options = vec!["server-only", "client-only"];
    println!("Usage: hello_world [{}]", possible_options.iter().fold(String::new(), |mut acc, item| { if acc.len() > 0 {acc.push('|')}; acc.push_str(item); acc } ));

    // command line options
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

    // start
    logic(start_server, start_client).await
}

async fn logic(start_server: bool, start_client: bool) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut server = None;
    if start_server {
        println!("==> starting the server");
        let server = server.insert(new_socket_server!(CONFIG, LISTENING_INTERFACE, PORT));
        type DerivedClientMessages = ArchivedClientMessages;    // uncomment if you'll use "VariableBinary"
        // type DerivedClientMessages = ClientMessages;    // uncomment you will use "Textual"
        let server_processor_handler = spawn_server_processor!(CONFIG, VariableBinary, FullSync, server, ClientMessages, ServerMessages,
            |_| future::ready(()),
            |_, _, peer, client_stream| client_stream
                .inspect(|client_message| println!(">>> {:?}", client_message.deref().deref()))
                .map(|client_message| match client_message.deref().deref() {
                    DerivedClientMessages::Hello => ServerMessages::World(WORLD.to_string()),
                    _ => ServerMessages::NoAnswer,
                })
                .to_responsive_stream(peer, |_, _| ())
        )?;
        server.start_single_protocol(server_processor_handler).await?;
    }

    if start_client {
        println!("==> starting the client");
        let mut client = new_socket_client!(CONFIG, LISTENING_INTERFACE, PORT);
        start_client_processor!(CONFIG, VariableBinary, FullSync, client, ServerMessages, ClientMessages,
            |connection_event| async {
                if let ProtocolEvent::PeerArrived { peer } = connection_event {
                    // sends a message as soon as the connection is established
                    peer.send_async(ClientMessages::Hello).await
                        .expect("The client couldn't send a message");
                }
            },
            |_, _, peer, server_stream| server_stream.inspect(move |server_message| {
                // on any received message from the server, disconnects without answering
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
impl ReactiveMessagingConfig<ClientMessages> for ClientMessages {
    fn processor_error_message(err: String) -> Option<ClientMessages> {
        Some(ClientMessages::Error(err))
    }
    fn input_error_message(err: String) -> Option<ClientMessages> {
        Some(ClientMessages::Error(err))
    }
}


// ServerMessages SerDe
///////////////////////
impl ReactiveMessagingConfig<ServerMessages> for ServerMessages {
    fn processor_error_message(err: String) -> Option<ServerMessages> {
        Some(ServerMessages::Error(err))
    }
    fn input_error_message(err: String) -> Option<ServerMessages> {
        Some(ServerMessages::Error(err))
    }
}