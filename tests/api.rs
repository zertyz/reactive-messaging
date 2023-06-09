//! Tests the API contracts.
//!
//! NOTE: many "tests" here have no real assertions, as they are used only to verify the API is able to represent certain models;
//!       actually, many are not even executed, as their futures are simply dropped.

use reactive_messaging::{
    SocketServer,
    SocketClient,
    prelude::{ConnectionEvent, ProcessorRemoteStreamType},
    ResponsiveMessages,
    ReactiveMessagingSerializer,
    ReactiveMessagingDeserializer,
};
use futures::stream::StreamExt;


/// Proves we are able to create a server whose processor outputs answers -- using a "Responsive Processor".
/// Note that special messages mean "don't send anything" (as defined by the serializer) and that 
/// answers may still be sent explicitly (using [Peer]).
#[cfg_attr(not(doc), test)]
fn server_with_responsive_processor() {
    let mut server = SocketServer::new("0.0.0.0".to_string(), 0);
    let _unused_future = server.spawn_responsive_processor(
        |_: ConnectionEvent<DummyResponsiveServerMessages>| async {},
        |_, _, _, client_messages: ProcessorRemoteStreamType<DummyResponsiveClientMessages>| client_messages.map(|_| DummyResponsiveServerMessages::ProducedByTheServer)
    );
}

/// Proves we are able to create a server whose processor doesn't output any answers -- using a "Unresponsive Processor".
/// Note that answers may still be sent, but they should be done explicitly
#[cfg_attr(not(doc), test)]
fn server_with_unresponsive_processor() {
    let mut server = SocketServer::new("0.0.0.0".to_string(), 0);
    let _unused_future = server.spawn_unresponsive_processor(
        |_: ConnectionEvent<DummyUnresponsiveServerMessages>| async {},
        |_, _, _, client_messages: ProcessorRemoteStreamType<DummyUnresponsiveClientMessages>| client_messages.map(|_| "anything")
    );
}

/// Proves we are able to create a client whose processor outputs answers -- using a "Responsive Processor".
/// Note that special messages mean "don't send anything" (as defined by the serializer) and that 
/// answers may still be sent explicitly (using [Peer]).
#[cfg_attr(not(doc), test)]
fn client_with_responsive_processor() {
    let _unused_future = SocketClient::spawn_responsive_processor(
        "0.0.0.0".to_string(),
        0,
        |_: ConnectionEvent<DummyResponsiveClientMessages>| async {},
        |_, _, _, server_messages: ProcessorRemoteStreamType<DummyResponsiveServerMessages>| server_messages.map(|_| DummyResponsiveClientMessages::ProducedByTheClient)
    );
}

/// Proves we are able to create a client whose processor doesn't output any answers -- using a "Unresponsive Processor".
/// Note that answers may still be sent, but they should be done explicitly
#[cfg_attr(not(doc), test)]
fn client_with_unresponsive_processor() {
    let _unused_future = SocketClient::spawn_unresponsive_processor(
        "0.0.0.0".to_string(),
        0,
        |_: ConnectionEvent<DummyUnresponsiveClientMessages>| async {},
        |_, _, _, server_messages: ProcessorRemoteStreamType<DummyResponsiveServerMessages>| server_messages.map(|_| "anything")
    );
}

/// The pretended messages produced by the client's "Responsive Processor"
#[derive(Debug,PartialEq)]
enum DummyResponsiveClientMessages {
    ProducedByTheClient,
}
impl ReactiveMessagingSerializer<DummyResponsiveClientMessages> for DummyResponsiveClientMessages {
    fn serialize(_remote_message: &DummyResponsiveClientMessages, _buffer: &mut Vec<u8>) {
        todo!()
    }
    fn processor_error_message(_err: String) -> DummyResponsiveClientMessages {
        todo!()
    }
}
impl ResponsiveMessages<DummyResponsiveClientMessages> for DummyResponsiveClientMessages {
    fn is_disconnect_message(_processor_answer: &DummyResponsiveClientMessages) -> bool {
        todo!()
    }
    fn is_no_answer_message(_processor_answer: &DummyResponsiveClientMessages) -> bool {
        todo!()
    }
}
impl ReactiveMessagingDeserializer<DummyResponsiveClientMessages> for DummyResponsiveClientMessages {
    fn deserialize(_local_message: &[u8]) -> Result<DummyResponsiveClientMessages, Box<dyn std::error::Error + Sync + Send>> {
        todo!()
    }
}

/// The pretended messages produced by the server's "Responsive Processor"
#[derive(Debug,PartialEq)]
enum DummyResponsiveServerMessages {
    ProducedByTheServer,
}
impl ReactiveMessagingSerializer<DummyResponsiveServerMessages> for DummyResponsiveServerMessages {
    fn serialize(_remote_message: &DummyResponsiveServerMessages, _buffer: &mut Vec<u8>) {
        todo!()
    }
    fn processor_error_message(_err: String) -> DummyResponsiveServerMessages {
        todo!()
    }
}
impl ResponsiveMessages<DummyResponsiveServerMessages> for DummyResponsiveServerMessages {
    fn is_disconnect_message(_processor_answer: &DummyResponsiveServerMessages) -> bool {
        todo!()
    }
    fn is_no_answer_message(_processor_answer: &DummyResponsiveServerMessages) -> bool {
        todo!()
    }
}
impl ReactiveMessagingDeserializer<DummyResponsiveServerMessages> for DummyResponsiveServerMessages {
    fn deserialize(_local_message: &[u8]) -> Result<DummyResponsiveServerMessages, Box<dyn std::error::Error + Sync + Send>> {
        todo!()
    }
}

/// The pretended messages produced by the client's "Unresponsive Processor"
#[derive(Debug,PartialEq)]
enum DummyUnresponsiveClientMessages {}
impl ReactiveMessagingSerializer<DummyUnresponsiveClientMessages> for DummyUnresponsiveClientMessages {
    fn serialize(_remote_message: &DummyUnresponsiveClientMessages, _buffer: &mut Vec<u8>) {
        todo!()
    }
    fn processor_error_message(_err: String) -> DummyUnresponsiveClientMessages {
        todo!()
    }
}
impl ReactiveMessagingDeserializer<DummyUnresponsiveClientMessages> for DummyUnresponsiveClientMessages {
    fn deserialize(_local_message: &[u8]) -> Result<DummyUnresponsiveClientMessages, Box<dyn std::error::Error + Sync + Send>> {
        todo!()
    }
}

/// The pretended messages produced by the server's "Unresponsive Processor"
#[derive(Debug,PartialEq)]
enum DummyUnresponsiveServerMessages {}
impl ReactiveMessagingSerializer<DummyUnresponsiveServerMessages> for DummyUnresponsiveServerMessages {
    fn serialize(_remote_message: &DummyUnresponsiveServerMessages, _buffer: &mut Vec<u8>) {
        todo!()
    }
    fn processor_error_message(_err: String) -> DummyUnresponsiveServerMessages {
        todo!()
    }
}
impl ReactiveMessagingDeserializer<DummyUnresponsiveServerMessages> for DummyUnresponsiveServerMessages {
    fn deserialize(_local_message: &[u8]) -> Result<DummyUnresponsiveServerMessages, Box<dyn std::error::Error + Sync + Send>> {
        todo!()
    }
}