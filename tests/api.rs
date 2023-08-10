//! Tests the API contracts.
//!
//! NOTE: many "tests" here have no real assertions, as they are used only to verify the API is able to represent certain models;
//!       actually, many are not even executed, as their futures are simply dropped.
/*
use reactive_messaging::{
    SocketServer,
    SocketClient,
    prelude::ConnectionEvent,
    ResponsiveMessages,
    ReactiveMessagingSerializer,
    ReactiveMessagingDeserializer,
};
use futures::stream::StreamExt;
use reactive_messaging::prelude::{MessagingAtomicUniType, MessagingAtomicDerivedType, MessagingMutinyStream};


const BUFFERED_MESSAGES_PER_PEER_COUNT: usize = 2048;


/// Proves we are able to create a server whose processor outputs answers -- using a "Responsive Processor".
/// Note that special messages mean "don't send anything" (as defined by the serializer) and that 
/// answers may still be sent explicitly (using [Peer]).
#[cfg_attr(not(doc), test)]
fn server_with_responsive_processor() {
    type SocketServerType = SocketServer::<BUFFERED_MESSAGES_PER_PEER_COUNT>;
    const UNI_INSTRUMENTS: usize = SocketServerType::uni_instruments();
    let mut server = SocketServerType::new("0.0.0.0", 0);
    let _unused_future = server.spawn_responsive_processor(
        |_| async {},
        |_, _, _, client_messages| client_messages.map(|_: MessagingAtomicDerivedType<1024, DummyResponsiveClientMessages>| DummyResponsiveServerMessages::ProducedByTheServer)
    );
}

/// Proves we are able to create a server whose processor doesn't output any answers -- using a "Unresponsive Processor".
/// Note that answers may still be sent, but they should be done explicitly
#[cfg_attr(not(doc), test)]
fn server_with_unresponsive_processor() {
    let mut server = SocketServer::<BUFFERED_MESSAGES_PER_PEER_COUNT>::new("0.0.0.0", 0);
    let _unused_future = server.spawn_unresponsive_processor(
        |_| async {},
        |_, _, _, client_messages: MessagingMutinyStream<MESSAGING_UNI_INSTRUMENTS, MessagingAtomicUniType<1024, MESSAGING_UNI_INSTRUMENTS, DummyUnresponsiveClientMessages>>| client_messages.map(|_| "anything")
    );
}

/// Proves we are able to create a client whose processor outputs answers -- using a "Responsive Processor".
/// Note that special messages mean "don't send anything" (as defined by the serializer) and that 
/// answers may still be sent explicitly (using [Peer]).
#[cfg_attr(not(doc), test)]
fn client_with_responsive_processor() {
    let _unused_future = SocketClient::<BUFFERED_MESSAGES_PER_PEER_COUNT>::spawn_responsive_processor(
        "0.0.0.0",
        0,
        |_: ConnectionEvent<BUFFERED_MESSAGES_PER_PEER_COUNT, DummyResponsiveClientMessages>| async {},
        |_, _, _, server_messages: ProcessorRemoteStreamType<BUFFERED_MESSAGES_PER_PEER_COUNT, DummyResponsiveServerMessages>| server_messages.map(|_| DummyResponsiveClientMessages::ProducedByTheClient)
    );
}

/// Proves we are able to create a client whose processor doesn't output any answers -- using a "Unresponsive Processor".
/// Note that answers may still be sent, but they should be done explicitly
#[cfg_attr(not(doc), test)]
fn client_with_unresponsive_processor() {
    let _unused_future = SocketClient::<BUFFERED_MESSAGES_PER_PEER_COUNT>::spawn_unresponsive_processor(
        "0.0.0.0",
        0,
        |_: ConnectionEvent<BUFFERED_MESSAGES_PER_PEER_COUNT, DummyUnresponsiveClientMessages>| async {},
        |_, _, _, server_messages: ProcessorRemoteStreamType<BUFFERED_MESSAGES_PER_PEER_COUNT, DummyResponsiveServerMessages>| server_messages.map(|_| "anything")
    );
}

/// The pretended messages produced by the client's "Responsive Processor"
#[derive(Debug,PartialEq)]
enum DummyResponsiveClientMessages {
    ProducedByTheClient,
}
impl ReactiveMessagingSerializer<DummyResponsiveClientMessages> for DummyResponsiveClientMessages {
    #[inline(always)]
    fn serialize(_remote_message: &DummyResponsiveClientMessages, _buffer: &mut Vec<u8>) {
        todo!()
    }
    #[inline(always)]
    fn processor_error_message(_err: String) -> DummyResponsiveClientMessages {
        todo!()
    }
}
impl ResponsiveMessages<DummyResponsiveClientMessages> for DummyResponsiveClientMessages {
    #[inline(always)]
    fn is_disconnect_message(_processor_answer: &DummyResponsiveClientMessages) -> bool {
        todo!()
    }
    #[inline(always)]
    fn is_no_answer_message(_processor_answer: &DummyResponsiveClientMessages) -> bool {
        todo!()
    }
}
impl ReactiveMessagingDeserializer<DummyResponsiveClientMessages> for DummyResponsiveClientMessages {
    #[inline(always)]
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
    #[inline(always)]
    fn serialize(_remote_message: &DummyResponsiveServerMessages, _buffer: &mut Vec<u8>) {
        todo!()
    }
    #[inline(always)]
    fn processor_error_message(_err: String) -> DummyResponsiveServerMessages {
        todo!()
    }
}
impl ResponsiveMessages<DummyResponsiveServerMessages> for DummyResponsiveServerMessages {
    #[inline(always)]
    fn is_disconnect_message(_processor_answer: &DummyResponsiveServerMessages) -> bool {
        todo!()
    }
    #[inline(always)]
    fn is_no_answer_message(_processor_answer: &DummyResponsiveServerMessages) -> bool {
        todo!()
    }
}
impl ReactiveMessagingDeserializer<DummyResponsiveServerMessages> for DummyResponsiveServerMessages {
    #[inline(always)]
    fn deserialize(_local_message: &[u8]) -> Result<DummyResponsiveServerMessages, Box<dyn std::error::Error + Sync + Send>> {
        todo!()
    }
}

/// The pretended messages produced by the client's "Unresponsive Processor"
#[derive(Debug,PartialEq)]
enum DummyUnresponsiveClientMessages {}
impl ReactiveMessagingSerializer<DummyUnresponsiveClientMessages> for DummyUnresponsiveClientMessages {
    #[inline(always)]
    fn serialize(_remote_message: &DummyUnresponsiveClientMessages, _buffer: &mut Vec<u8>) {
        todo!()
    }
    #[inline(always)]
    fn processor_error_message(_err: String) -> DummyUnresponsiveClientMessages {
        todo!()
    }
}
impl ReactiveMessagingDeserializer<DummyUnresponsiveClientMessages> for DummyUnresponsiveClientMessages {
    #[inline(always)]
    fn deserialize(_local_message: &[u8]) -> Result<DummyUnresponsiveClientMessages, Box<dyn std::error::Error + Sync + Send>> {
        todo!()
    }
}

/// The pretended messages produced by the server's "Unresponsive Processor"
#[derive(Debug,PartialEq)]
enum DummyUnresponsiveServerMessages {}
impl ReactiveMessagingSerializer<DummyUnresponsiveServerMessages> for DummyUnresponsiveServerMessages {
    #[inline(always)]
    fn serialize(_remote_message: &DummyUnresponsiveServerMessages, _buffer: &mut Vec<u8>) {
        todo!()
    }
    #[inline(always)]
    fn processor_error_message(_err: String) -> DummyUnresponsiveServerMessages {
        todo!()
    }
}
impl ReactiveMessagingDeserializer<DummyUnresponsiveServerMessages> for DummyUnresponsiveServerMessages {
    #[inline(always)]
    fn deserialize(_local_message: &[u8]) -> Result<DummyUnresponsiveServerMessages, Box<dyn std::error::Error + Sync + Send>> {
        todo!()
    }
}*/