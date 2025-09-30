use std::any::type_name;
use std::fmt::Debug;
use serde::{Serialize, Deserialize};
use once_cell::sync::Lazy;
use crate::prelude::Error;
use crate::serde::{ReactiveMessagingConfig, ReactiveMessagingDeserializer, ReactiveMessagingSerializer};

/// Serializer for `LocalMessages` in textual format using the `ron` crate
pub struct ReactiveMessagingRonSerializer/*<LocalMessages: ReactiveMessagingConfig<LocalMessages>> */{
    // _phantom: PhantomData<LocalMessages>,
}
impl<LocalMessages> ReactiveMessagingSerializer<LocalMessages> for ReactiveMessagingRonSerializer
where
    LocalMessages: Serialize + ReactiveMessagingConfig<LocalMessages> {

    #[inline(always)]
    fn serialize(local_message: &LocalMessages, buffer: &mut Vec<u8>) {
        ron_serializer(local_message, buffer)
            .unwrap_or_else(|err| panic!("`reactive_messaging::ReactiveMessagingRonSerializer<{}>::serialize()` Failed to serialize with RON: {err}",
                                                                type_name::<LocalMessages>()))
    }
}

/// Deserializer for `LocalMessages` in textual format using the `ron` crate
pub struct ReactiveMessagingRonDeserializer {}
impl<RemoteMessages> ReactiveMessagingDeserializer<RemoteMessages> for ReactiveMessagingRonDeserializer
where
    RemoteMessages: Send + Sync + PartialEq + Debug + for<'r> Deserialize<'r> + 'static {

    type DeserializedRemoteMessages = RemoteMessages;

    #[inline(always)]
    fn validate(_remote_message: &[u8]) -> Result<(), Error> {
        unreachable!("`reactive-messaging::textual_serde::ReactiveMessagingRonDeserializer<{}>::validate()`: BUG! `validate()` should never be called for any textual deserializer", type_name::<RemoteMessages>())
    }

    #[inline(always)]
    fn deserialize(remote_message: &[u8]) -> Result<Self::DeserializedRemoteMessages, crate::prelude::Error> {
        ron_deserializer(remote_message)
            .map_err(|err| crate::prelude::Error::TextualInputParsingError {
                msg: format!("`reactive_messaging::ReactiveMessagingRonDeserializer<{}>::deserialize()` Failed to deserialize with RON", type_name::<RemoteMessages>()),
                cause: err
            })
    }

    #[inline(always)]
    fn deserialize_as_ref(_remote_message: &[u8]) -> Result<&Self::DeserializedRemoteMessages, Error> {
        unreachable!("`reactive-messaging::textual_serde::ReactiveMessagingRonDeserializer<{}>::deserialize_as_ref()`: BUG! `deserialize_as_ref()` should never be called for any textual deserializer", type_name::<RemoteMessages>())
    }
}


// RON SERDE
////////////

static RON_DESERIALIZER_CONFIG: Lazy<ron::Options> = Lazy::new(ron::Options::default);

/// RON serializer
#[inline(always)]
fn ron_serializer<T: ?Sized + Serialize>(message: &T, buffer: &mut Vec<u8>) -> Result<(), ron::Error> {

    use std::fmt::{self, Write};
    pub struct VecWriteAdapter<'a>(&'a mut Vec<u8>);
    impl Write for VecWriteAdapter<'_> {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            self.0.extend_from_slice(s.as_bytes());
            Ok(())
        }
    }

    buffer.clear();
    let buffer = VecWriteAdapter(buffer);
    let mut serializer = ron::Serializer::with_options(buffer, None, &RON_DESERIALIZER_CONFIG)?;
    message.serialize(&mut serializer)?;
    Ok(())
}

/// RON deserializer
#[inline(always)]
fn ron_deserializer<T: for<'r> Deserialize<'r>>(message: &[u8]) -> Result<T, Box<dyn std::error::Error + Sync + Send>> {
    RON_DESERIALIZER_CONFIG.from_bytes(message)
        .map_err(|err| Box::from(format!("RON deserialization error for message '{:?}': {}", std::str::from_utf8(message), err)))
}


/// Unit tests for our socket server [serde](self) module
#[cfg(any(test,doc))]
mod tests {
    use super::*;


    /// assures RON serialization / deserialization works for all client / server messages
    #[cfg_attr(not(doc),test)]
    fn ron_serde_for_server_only() {

        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        pub enum Messages {
            Echo(String),
            Recursive(Option<Box<Messages>>)
        }

        let mut buffer = Vec::<u8>::with_capacity(64);
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