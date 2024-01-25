//! Resting place for the socket based [client] and [server] submodules, as well as their common [types]

pub(crate) mod socket_server;
pub use socket_server::*;

pub(crate) mod socket_client;

pub use socket_client::*;

pub(crate) mod types;
