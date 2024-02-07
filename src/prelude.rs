//! Re-exports of types -- out of the internal directory and module structure -- useful for users of this crate

pub use crate::config::*;
pub use crate::types::*;
pub use crate::socket_services::{
    types::*,
    socket_server::*,
    socket_client::*,
};
pub use crate::socket_connection::{
    connection::*,
    connection_provider::*,
    peer::Peer,
};
pub use crate::serde::{
    ReactiveMessagingSerializer,
    ReactiveMessagingDeserializer,
    ron_serializer,
    ron_deserializer,
};

// from `reactive-mutiny`:
pub use reactive_mutiny::prelude::advanced::*;