//! Contains common functions used across all the integration tests


use std::time::{SystemTime, UNIX_EPOCH};


/// Measures the current time, returning the elapsed µs since epoch.\
/// This function enables storing the current clock in an atomic `u128`, to avoid the need for a `Mutex<SystemTime>` when working with multiple threads
pub fn now_as_micros() -> u64 {
    let start_time = SystemTime::now();
    start_time.duration_since(UNIX_EPOCH).expect("Time went backwards?!?!").as_micros() as u64
}