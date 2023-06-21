//! SERializers & DEserializers (traits & implementations) for our [SocketServer]


use crate::{
    socket_connection_handler::Peer,
};
use std::{
    sync::Arc,
    fmt::Write,
    fmt::Debug,
};
use futures::future::BoxFuture;
use ron::{
    Options,
    ser::PrettyConfig,
};
use serde::{Serialize, Deserialize};
use lazy_static::lazy_static;


/// Trait that should be implemented by enums that model the "local messages" to be handled by the [SocketServer] --
/// "local messages" are, typically, server messages (see [ServerMessages]) -- but may also be client messages if we're building a client (for tests?)\
/// This trait, therefore, specifies how to:
///   * `serialize()` enum variants into a String (like RON, for textual protocols) to be sent to the remote peer
///   * inform the peer if any wrong input was sent
///   * identify local messages that should cause a disconnection
pub trait SocketServerSerializer<LocalPeerMessages: SocketServerSerializer<LocalPeerMessages> + Send + PartialEq + Debug> {

    /// `SocketServer`s serializer: transforms a strong typed `message` into a `String`.\
    /// IMPLEMENTORS: #[inline(always)]
    fn ss_serialize(message: &LocalPeerMessages) -> String;

    /// Called whenever the socket server found an error -- the returned message should be as descriptive as possible.\
    /// IMPLEMENTORS: #[inline(always)]
    fn processor_error_message(err: String) -> LocalPeerMessages;

    /// Informs if the given internal `processor_answer` is a "disconnect" message (usually issued by the messages processor)\
    /// -- in which case, the socket server will send it and, immediately, close the connection.\
    /// IMPLEMENTORS: #[inline(always)]
    fn is_disconnect_message(processor_answer: &LocalPeerMessages) -> bool;

    /// Tells if the given `processor_answer` represents a "no message" -- a message that should produce no answer to the peer.\
    /// IMPLEMENTORS: #[inline(always)]
    fn is_no_answer_message(processor_answer: &LocalPeerMessages) -> bool;
}

/// Trait that should be implemented by enums that model the "remote messages" to be handled by the [SocketServer] --
/// "remote messages" are, typically, client messages (see [ClientMessages]) -- but may also be server messages if we're building a client (for tests?)\
/// This trait, therefore, specifies how to:
///   * `deserialize()` enum variants received by the remote peer (like RON, for textual protocols)
pub trait SocketServerDeserializer<T> {

    /// `SocketServer`s deserializer: transform a textual `message` into a string typed value.\
    /// IMPLEMENTORS: #[inline(always)]
    fn ss_deserialize(message: &[u8]) -> Result<T, Box<dyn std::error::Error + Sync + Send>>;
}


// RON SERDE
////////////

lazy_static! {

    static ref RON_EXTENSIONS: ron::extensions::Extensions = {
        let mut extensions = ron::extensions::Extensions::empty();
        extensions.insert(ron::extensions::Extensions::IMPLICIT_SOME);
        extensions.insert(ron::extensions::Extensions::UNWRAP_NEWTYPES);
        extensions.insert(ron::extensions::Extensions::UNWRAP_VARIANT_NEWTYPES);
        extensions
    };

    static ref RON_SERIALIZER_CONFIG: PrettyConfig = ron::ser::PrettyConfig::new()
        .depth_limit(10)
        .new_line(String::from(""))
        .indentor(String::from(""))
        .separate_tuple_members(true)
        .enumerate_arrays(false)
        .extensions(*RON_EXTENSIONS);

    static ref RON_DESERIALIZER_CONFIG: Options = ron::Options::default()
        ;//.with_default_extension(*RON_EXTENSIONS);

}

/// RON serializer
#[inline(always)]
pub fn ron_serializer<T: Serialize>(message: &T) -> String {
    let mut output_data = ron::ser::to_string(message).unwrap();
    write!(output_data, "\n").unwrap();
    output_data
}

/// RON deserializer
#[inline(always)]
pub fn ron_deserializer<T: for<'a> Deserialize<'a>>(message: &[u8]) -> Result<T, Box<dyn std::error::Error + Sync + Send>> {
    RON_DESERIALIZER_CONFIG.from_bytes(message)
        .map_err(|err| Box::from(format!("RON deserialization error for message '{:?}': {}", std::str::from_utf8(message), err)))
}


/// Unit tests for our socket server [serde](self) module
#[cfg(any(test,doc))]
mod tests {
    use super::*;


    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    pub enum Messages {
        Echo(String),
        Recursive(Option<Box<Messages>>)
    }


    /// assures RON serialization / deserialization works for all client / server messages
    #[cfg_attr(not(doc),test)]
    fn ron_serde_for_server_only() {
        let message = Messages::Echo(String::from("This is an error message"));
        let expected = "Echo(\"This is an error message\")\n";
        let observed = ron_serializer(&message);
        assert_eq!(observed, expected, "RON serialization is not good");

        let message = "Recursive(Some(Recursive(Some(Echo(\"here it is\")))))".as_bytes();
        let expected = Messages::Recursive(Some(Box::new(Messages::Recursive(Some(Box::new(Messages::Echo(String::from("here it is"))))))));
        let observed = ron_deserializer::<Messages>(message)
            .expect("RON deserialization failed");
        assert_eq!(observed, expected, "RON deserialization is not good");
    }
}