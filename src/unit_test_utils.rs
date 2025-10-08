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

#[derive(Debug,Default,PartialEq,Serialize,Deserialize)]
pub struct TestString {
    #[serde(skip)]
    pub drop_count: u64,
    pub inner: String,
}
use std::fmt::{Display, Formatter};
use std::ops::Deref;
use serde::{Deserialize, Serialize};

impl Display for TestString {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.inner, f)
    }
}
impl From<&str> for TestString {
    fn from(value: &str) -> Self {
        Self { drop_count: 0, inner: String::from(value)}
    }
}
impl From<String> for TestString {
    fn from(value: String) -> Self {
        Self { drop_count: 0, inner: String::from(value)}
    }
}
impl PartialEq<String> for TestString {
    fn eq(&self, other: &String) -> bool {
        &self.inner == other
    }
}
impl Deref for TestString {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl AsRef<str> for TestString {
    fn as_ref(&self) -> &str {
        &self.inner
    }
}

impl Drop for TestString {
    fn drop(&mut self) {
        self.drop_count += 1;
        if self.drop_count > 1 {
            panic!("### WOULD BE DOUBLE-DROPPING (total of {} drops!) {:x}: {}", self.drop_count, std::ptr::addr_of!(self).addr(), self.inner);
        }
        // drop(self.inner);
    }
}


impl ReactiveMessagingConfig<TestString> for TestString {
    #[inline(always)]
    fn processor_error_message(err: String) -> Option<TestString> {
        let msg = format!("ServerBug! Please, fix! Error: {}", err);
        panic!("SocketServerSerializer<String>::processor_error_message(): {}", msg);
        // msg
    }

    #[inline(always)]
    fn input_error_message(err: String) -> Option<TestString> {
        let msg = format!("ServerBug! Please, fix! Error: {}", err);
        panic!("SocketServerSerializer<String>::parsing_error_message(): {}", msg);
        // msg
    }
}

/// Test implementation for our text-only protocol as used across unit tests
pub struct TestStringSerializer;
impl ReactiveMessagingSerializer<TestString> for TestStringSerializer {
    #[inline(always)]
    fn serialize(message: &TestString, buffer: &mut Vec<u8>) {
        buffer.clear();
        buffer.extend_from_slice(message.inner.as_bytes());
    }
}

/// Test implementation for our text-only protocol as used across unit tests
pub struct TestStringDeserializer;
impl ReactiveMessagingDeserializer<TestString> for TestStringDeserializer {
    type DeserializedRemoteMessages = TestString;

    fn validate(_remote_message: &[u8]) -> Result<(), Error> {
        unreachable!()
    }

    #[inline(always)]
    fn deserialize(message: &[u8]) -> Result<TestString, crate::types::Error> {
        Ok(TestString {
            drop_count: 0,
            inner: String::from_utf8_lossy(message).to_string(),
        })
    }

    fn deserialize_as_ref(_remote_message: &[u8]) -> Result<&Self::DeserializedRemoteMessages, Error> {
        unreachable!()
    }
}

