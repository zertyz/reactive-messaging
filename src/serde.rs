//! SERializers & DEserializers (traits & implementations) for our [SocketServer]


use serde::{Serialize, Deserialize};
use once_cell::sync::Lazy;


/// Trait that should be implemented by enums that model the "local messages" to be sent to the remote peer --
/// "local messages" may either be messages generated by the server or by the client, depending on if you're building a server or client.\
/// This trait, therefore, specifies how to:
///   * `serialize()` enum variants into a String (like RON, for textual protocols) to be sent to the remote peer
///   * inform the peer if any wrong input was sent -- the network processor will handle that special case.
pub trait ReactiveMessagingSerializer<LocalPeerMessages> {

    /// Local messages serializer: transforms a strong typed `message` into a sequence of bytes, put in `buffer`\
    /// -- not appending any '\n'.
    /// IMPLEMENTORS: #[inline(always)]
    fn serialize(local_message: &LocalPeerMessages, buffer: &mut Vec<u8>);

    /// Called whenever the local processor found an error -- the returned message should be as descriptive as possible.\
    /// IMPLEMENTORS: #[inline(always)]
    fn processor_error_message(err: String) -> LocalPeerMessages;
}

/// Trait that should be implemented by enums that model the "remote messages" to be consumed by a "Responsive Processor" --
/// "remote messages" may either be messages produced by the remote server or by the remote client (when we are implementing the opposite peer).\
/// This trait, therefore, specifies how to `deserialize()` enum variants received by the remote peer (like RON, for textual protocols)
pub trait ReactiveMessagingDeserializer<T> {

    /// Local messages deserializer: transform a textual `message` into a string typed value.\
    /// IMPLEMENTORS: #[inline(always)]
    fn deserialize(remote_message: &[u8]) -> Result<T, Box<dyn std::error::Error + Sync + Send>>;
}


// RON SERDE
////////////

static RON_DESERIALIZER_CONFIG: Lazy<ron::Options> = Lazy::new(ron::Options::default);

/// RON serializer
#[inline(always)]
pub fn ron_serializer<T: ?Sized + Serialize>(message: &T, buffer: &mut Vec<u8>) -> Result<(), ron::Error> {
    buffer.clear();
    let mut serializer = ron::Serializer::with_options(buffer, None, ron::Options::default())?;
    message.serialize(&mut serializer)?;
    Ok(())
}

/// RON deserializer
#[inline(always)]
pub fn ron_deserializer<T: for<'r> Deserialize<'r>>(message: &[u8]) -> Result<T, Box<dyn std::error::Error + Sync + Send>> {
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
        let mut buffer = Vec::<u8>::with_capacity(16);
        let message = Messages::Echo(String::from("This is an error message"));
        let expected = "Echo(\"This is an error message\")";
        ron_serializer(&message, &mut buffer).expect("calling `ron_serializer()`");
        let observed = String::from_utf8(buffer).expect("Ron should be utf-8");
        assert_eq!(observed, expected, "RON serialization is not good");

        let message = "Recursive(Some(Recursive(Some(Echo(\"here it is\")))))".as_bytes();
        let expected = Messages::Recursive(Some(Box::new(Messages::Recursive(Some(Box::new(Messages::Echo(String::from("here it is"))))))));
        let observed = ron_deserializer::<Messages>(message)
            .expect("RON deserialization failed");
        assert_eq!(observed, expected, "RON deserialization is not good");
    }
}