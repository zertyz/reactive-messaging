use std::any::type_name;
use std::fmt::Debug;

/// Serializer for `LocalMessages` in binary blobs using the `rkyv` crate
pub struct ReactiveMessagingRkyvSerializer {}
impl<LocalMessages> ReactiveMessagingSerializer<LocalMessages> for ReactiveMessagingRkyvSerializer
where
    LocalMessages: for<'r> rkyv::Serialize<WriteSerializer<&'r mut Vec<u8>>> {

    #[inline(always)]
    fn serialize(local_message: &LocalMessages, buffer: &mut Vec<u8>) {
        rkyv_serializer(local_message, buffer)
            .unwrap_or_else(|err| panic!("`reactive_messaging::ReactiveMessagingRkyvSerializer<{}>::serialize()` Failed to serialize with RKYV: {err}",
                                                                type_name::<LocalMessages>()))
    }
}

/// Deserializer for `RemoteMessages` in binary blobs using the `rkyv` crate -- bringing back objects of type `RemoteMessages::Archived`
pub struct ReactiveMessagingRkyvFastDeserializer {}
impl<RemoteMessages> ReactiveMessagingDeserializer<RemoteMessages> for ReactiveMessagingRkyvFastDeserializer
where
    RemoteMessages: rkyv::Archive,
    RemoteMessages::Archived: Debug + PartialEq + Send + Sync + 'static /* NOTE: to satisfy this condition, use '#[archive_attr(derive(Debug, PartialEq))]' in your type */ {
    
    type DeserializedRemoteMessages = RemoteMessages::Archived;

    fn validate(_remote_message: &[u8]) -> Result<(), Error> {
        // the RKYV fast deserializer doesn't validate the input
        Ok(())
    }

    /// IMPORTANT: this implementation may crash if `remote_messages` contains invalid data.
    /// Use [ReactiveMessagingRkyvSafeDeserializer<>] instead if the remote party is not trusty.
    #[inline(always)]
    fn deserialize(_remote_message: &[u8]) -> Result<Self::DeserializedRemoteMessages, crate::prelude::Error> {
        unreachable!("`reactive-messaging::variable_binary_serde::ReactiveMessagingRkyvFastDeserializer<{}>::deserialize()`: BUG! `deserialize()` should never be called for `rkyv` deserializer", type_name::<RemoteMessages>())
    }

    fn deserialize_as_ref(remote_message: &[u8]) -> Result<&Self::DeserializedRemoteMessages, Error> {
        Ok(rkyv_deserializer::<RemoteMessages>(remote_message))
    }
}


// RKYV SerDe helpers
/////////////////////

use rkyv::{
    archived_root,
    ser::Serializer as RkyvSerializer,
};
use rkyv::ser::serializers::WriteSerializer;
use crate::prelude::Error;
use crate::serde::{ReactiveMessagingConfig, ReactiveMessagingDeserializer, ReactiveMessagingSerializer};

/// RKYV serializer
#[inline(always)]
fn rkyv_serializer<'a, T: rkyv::Serialize<WriteSerializer<&'a mut Vec<u8>>>>
                  (message: &'a T,
                   buffer: &'a mut Vec<u8>)
                  -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    buffer.clear();
    let mut serializer = WriteSerializer::new(buffer);
    serializer.serialize_value(message)?;
    Ok(())
}

/// RKYV deserializer
#[inline(always)]
fn rkyv_deserializer<T: rkyv::Archive>
                    (message: &[u8])
                    -> &<T as rkyv::Archive>::Archived {
    unsafe {
        archived_root::<T>(message)
    }
}


/// Unit tests for [super::super]
#[cfg(any(test,doc))]
mod tests {
    use super::*;

    /// assures RKYV serialization / deserialization works for all client / server messages
    #[cfg_attr(not(doc),test)]
    fn rkyv_serde_for_server_only() {

        #[derive(Clone, Debug, PartialEq, rkyv::Serialize, rkyv::Deserialize, rkyv::Archive)]
        pub enum Messages {
            Echo(String),
            TheEnd
        }

        let mut buffer = Vec::<u8>::with_capacity(64);
        let expected_value = String::from("This is a message tha will go through RKYV");
        let message = Messages::Echo(expected_value.clone());
        rkyv_serializer(&message, &mut buffer).expect("calling `rkyv_serializer()`");
        let observed = rkyv_deserializer::<Messages>(&buffer);
        // assert
        if let ArchivedMessages::Echo(observed_value) = observed {
            assert_eq!(observed_value.as_str(), expected_value.as_str(), "RKYV serialization is not looking good");
        } else {
            panic!("RON serialization failed -- the variant is wrong");
        }
    }

}