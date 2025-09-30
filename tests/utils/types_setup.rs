//! Types setup for tests

use reactive_messaging::prelude::{Error, ReactiveMessagingConfig, ReactiveMessagingDeserializer, ReactiveMessagingSerializer};
use std::fmt::{Display, Formatter};
use serde::{Deserialize, Serialize};

#[derive(Default, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TestString(pub String);
impl Display for TestString {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}


/// Test implementation for text-only protocols as used across all integration tests
impl ReactiveMessagingConfig<TestString> for TestString {
    #[inline(always)]
    fn processor_error_message(err: String) -> Option<TestString> {
        let msg = format!("ServiceBug! Please, fix! Error: {}", err);
        panic!("ReactiveMessagingSerializer<TestString>::processor_error_message(): {}", msg);
        // msg
    }

    fn input_error_message(err: String) -> Option<TestString> {
        let msg = format!("ServiceBug! Please, fix! Error: {}", err);
        panic!("ReactiveMessagingSerializer<TestString>::parsing_error_message(): {}", msg);
        // msg
    }
}

/// Custom serializer to be used on the HTTP client test
pub struct PlainTextSerializer;
impl ReactiveMessagingSerializer<TestString> for PlainTextSerializer {
    #[inline(always)]
    fn serialize(message: &TestString, buffer: &mut Vec<u8>) {
        buffer.clear();
        buffer.extend_from_slice(message.0.as_bytes());
    }
}

/// Custom deserializer for the HTTP client test
pub struct PlainTextDeserializer;
impl ReactiveMessagingDeserializer<TestString> for PlainTextDeserializer {
    type DeserializedRemoteMessages = TestString;

    fn validate(_remote_message: &[u8]) -> Result<(), Error> {
        unreachable!()
    }

    #[inline(always)]
    fn deserialize(message: &[u8]) -> Result<TestString, Error> {
        Ok(TestString(String::from_utf8_lossy(message).to_string()))
    }

    fn deserialize_as_ref(_remote_message: &[u8]) -> Result<&Self::DeserializedRemoteMessages, Error> {
        unreachable!()
    }
}