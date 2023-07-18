//! Contains constants and other configuration information affecting default & fixed behaviors of this library

use std::time::Duration;
use reactive_mutiny::prelude::Instruments;


/// How many bytes to pre-allocate to each connected peer to receive their dialogues
pub const CHAT_MSG_SIZE_HINT: usize = 1024;
/// Default executor instruments for processing the server-side logic due to each client message
pub const SOCKET_PROCESSOR_INSTRUMENTS: usize = Instruments::NoInstruments.into();
/// Timeout to wait for any last messages to be sent to the peer when a disconnection was commanded
pub const GRACEFUL_STREAM_ENDING_TIMEOUT_DURATION: Duration = Duration::from_millis(100);

