//! Types setup for tests

use reactive_messaging::prelude::{ReactiveMessagingTextualDeserializer, ReactiveMessagingTextualSerializer};
use std::fmt::{Display, Formatter};


#[derive(Default, Debug, PartialEq)]
pub struct TestString(pub String);
impl Display for TestString {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Test implementation for text-only protocols as used across all integration tests
impl ReactiveMessagingTextualSerializer<TestString> for TestString {
    #[inline(always)]
    fn serialize(message: &TestString, buffer: &mut Vec<u8>) {
        buffer.clear();
        buffer.extend_from_slice(message.0.as_bytes());
    }
    #[inline(always)]
    fn processor_error_message(err: String) -> Option<TestString> {
        let msg = format!("ServiceBug! Please, fix! Error: {}", err);
        panic!("ReactiveMessagingSerializer<TestString>::processor_error_message(): {}", msg);
        // msg
    }

    fn parsing_error_message(err: String) -> Option<TestString> {
        let msg = format!("ServiceBug! Please, fix! Error: {}", err);
        panic!("ReactiveMessagingSerializer<TestString>::parsing_error_message(): {}", msg);
        // msg
    }
}

/// Test implementation for our text-only protocols as used across all integration tests
impl ReactiveMessagingTextualDeserializer<TestString> for TestString {
    #[inline(always)]
    fn deserialize(message: &[u8]) -> Result<TestString, Box<dyn std::error::Error + Sync + Send + 'static>> {
        Ok(TestString(String::from_utf8_lossy(message).to_string()))
    }
}