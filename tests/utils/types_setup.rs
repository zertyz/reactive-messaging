//! Types setup for tests

use reactive_messaging::prelude::{ReactiveMessagingDeserializer, ReactiveMessagingSerializer};
use std::fmt::{Display, Formatter};


#[derive(Default, Debug, PartialEq)]
pub struct TestString(pub String);
impl Display for TestString {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Test implementation for text-only protocols as used across all integration tests
impl ReactiveMessagingSerializer<TestString> for TestString {
    #[inline(always)]
    fn serialize_textual(message: &TestString, buffer: &mut Vec<u8>) {
        buffer.clear();
        buffer.extend_from_slice(message.0.as_bytes());
    }
    #[inline(always)]
    fn processor_error_message(err: String) -> TestString {
        let msg = format!("ServiceBug! Please, fix! Error: {}", err);
        panic!("ReactiveMessagingSerializer<TestString>::processor_error_message(): {}", msg);
        // msg
    }
}

/// Test implementation for our text-only protocols as used across all integration tests
impl ReactiveMessagingDeserializer<TestString> for TestString {
    #[inline(always)]
    fn deserialize_textual(message: &[u8]) -> Result<TestString, Box<dyn std::error::Error + Sync + Send + 'static>> {
        Ok(TestString(String::from_utf8_lossy(message).to_string()))
    }
}