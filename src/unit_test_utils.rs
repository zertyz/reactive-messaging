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
    
