//! Common code used across unit tests

use std::sync::atomic::AtomicU16;
use std::sync::atomic::Ordering::Relaxed;


/// Call this to always get a different port that can be used to start a test server
/// -- this way, many tests can be run in parallel without any port collision
pub fn next_server_port() -> u16 {
    static NEXT_SERVER_PORT: AtomicU16 = AtomicU16::new(8750);
    NEXT_SERVER_PORT.fetch_add(1, Relaxed)
}

/// Automatically executed one
/// (provided this module is accessed?)
#[ctor::ctor]
fn suite_setup() {
    simple_logger::SimpleLogger::new().with_utc_timestamps().init().unwrap_or_else(|_| eprintln!("--> LOGGER WAS ALREADY STARTED"));
}

use crate::prelude::{Error, ReactiveMessagingConfig};
use crate::serde::{ReactiveMessagingDeserializer, ReactiveMessagingSerializer};

impl ReactiveMessagingConfig<String> for String {
    #[inline(always)]
    fn processor_error_message(err: String) -> Option<String> {
        let msg = format!("ServerBug! Please, fix! Error: {}", err);
        panic!("SocketServerSerializer<String>::processor_error_message(): {}", msg);
        // msg
    }

    #[inline(always)]
    fn input_error_message(err: String) -> Option<String> {
        let msg = format!("ServerBug! Please, fix! Error: {}", err);
        panic!("SocketServerSerializer<String>::parsing_error_message(): {}", msg);
        // msg
    }
}

/// Test implementation for our text-only protocol as used across unit tests
pub struct StringSerializer;
impl ReactiveMessagingSerializer<String> for StringSerializer {
    #[inline(always)]
    fn serialize(message: &String, buffer: &mut Vec<u8>) {
        buffer.clear();
        buffer.extend_from_slice(message.as_bytes());
    }
}

/// Test implementation for our text-only protocol as used across unit tests
pub struct StringDeserializer;
impl ReactiveMessagingDeserializer<String> for StringDeserializer {
    type DeserializedRemoteMessages = String;

    fn validate(remote_message: &[u8]) -> Result<(), Error> {
        unreachable!()
    }

    #[inline(always)]
    fn deserialize(message: &[u8]) -> Result<String, crate::types::Error> {
        Ok(String::from_utf8_lossy(message).to_string())
    }

    fn deserialize_as_ref(remote_message: &[u8]) -> Result<&Self::DeserializedRemoteMessages, Error> {
        unreachable!()
    }
}

