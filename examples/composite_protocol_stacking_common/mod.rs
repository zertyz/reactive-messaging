
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