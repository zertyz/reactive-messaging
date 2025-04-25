
pub mod protocol_model; // Contains data models shared across protocol

pub mod logic; // Encapsulates server and client logic

use reactive_messaging::prelude::{ConstConfig, SocketOptions};

pub const NETWORK_CONFIG: ConstConfig = ConstConfig {
    // Maximum message size for sender and receiver
    receiver_max_msg_size: 1024,
    sender_max_msg_size: 1024,
    // Minimal queue sizes as this is a ping-pong game (and we don't buffer any messages)
    receiver_channel_size: 32,
    sender_channel_size: 32,
    socket_options: SocketOptions {
        hops_to_live:  None,          // Disable hop count limit
        linger_millis: None,          // No linger delay
        no_delay:      Some(true),    // TCP no-delay enabled
    },
    ..ConstConfig::default()
};


/// Wrapper macro for [reactive_messaging::spawn_server_processor!()], with:
///   1) Message type: MmapBinary (no serialization/deserialization = zero-copy).
///   2) Channel type: FullSync (reactive-mutiny).
///
/// Note that the same message kind must be used for the client (bellow) as well
#[macro_export]
macro_rules! spawn_server_processor {
    ($_1: expr, $_4: expr, $_5: ty, $_6: ty, $_7: expr, $_8: expr) => {
        reactive_messaging::spawn_server_processor!($_1, MmapBinary, FullSync, $_4, $_5, $_6, $_7, $_8)
        // reactive_messaging::spawn_server_processor!($_1, VariableBinary, FullSync, $_4, $_5, $_6, $_7, $_8)
    }
}
pub use spawn_server_processor;

/// Wrapper macro for [reactive_messaging::spawn_client_processor!()], with:
///   1) Message type: MmapBinary (no serialization/deserialization = zero-copy).
///   2) Channel type: FullSync (reactive-mutiny).
///
/// Note that the same message kind must be used for the server (above) as well
#[macro_export]
macro_rules! spawn_client_processor {
    ($_1: expr, $_4: expr, $_5: ty, $_6: ty, $_7: expr, $_8: expr) => {
        reactive_messaging::spawn_client_processor!($_1, MmapBinary, FullSync, $_4, $_5, $_6, $_7, $_8)
    }
}
pub use spawn_client_processor;