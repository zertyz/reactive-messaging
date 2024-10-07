pub mod protocol_model;
pub mod logic;

use reactive_messaging::prelude::{ConstConfig, SocketOptions};

pub const NETWORK_CONFIG: ConstConfig = ConstConfig {
    // message size limits
    receiver_max_msg_size: 1024,
    sender_max_msg_size: 1024,
    // queue limits -- we use tiny numbers as we don't buffer anything -- ping-pong, remember...
    receiver_channel_size: 32,
    sender_channel_size: 32,
    socket_options: SocketOptions {
        hops_to_live:  None,
        linger_millis: None,
        no_delay:      Some(true),
    },
    ..ConstConfig::default()
};


/// Wrapper around the real [reactive_messaging::spawn_server_processor!()] macro, specifying
/// the fastest possible message kind & channel for our example:
///   1) MmapBinary -- where no serialization / deserialization is involved -- effectively, zero-copy
///   2) Using the FullSync reactive-mutiny channel.
///
/// Note that the same message kind must be used for the server as well
#[macro_export]
macro_rules! spawn_server_processor {
    ($_1: expr, $_4: expr, $_5: ty, $_6: ty, $_7: expr, $_8: expr) => {
        reactive_messaging::spawn_server_processor!($_1, MmapBinary, FullSync, $_4, $_5, $_6, $_7, $_8)
    }
}
pub use spawn_server_processor;

/// Wrapper around the real [reactive_messaging::spawn_client_processor!()] macro, specifying
/// the fastest possible message kind & channel for our example:
///   1) MmapBinary -- where no serialization / deserialization is involved -- effectively, zero-copy
///   2) Using the FullSync reactive-mutiny channel.
///
/// Note that the same message kind must be used for the server as well
#[macro_export]
macro_rules! spawn_client_processor {
    ($_1: expr, $_4: expr, $_5: ty, $_6: ty, $_7: expr, $_8: expr) => {
        reactive_messaging::spawn_client_processor!($_1, MmapBinary, FullSync, $_4, $_5, $_6, $_7, $_8)
    }
}
pub use spawn_client_processor;