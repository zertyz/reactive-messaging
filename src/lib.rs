pub(crate) mod config;

mod socket_connection_handler;
pub use crate::socket_connection_handler::ResponsiveMessages;

mod serde;
pub use crate::serde::{
    ReactiveMessagingSerializer,
    ReactiveMessagingDeserializer,
    ron_serializer,
    ron_deserializer,
};

mod types;

pub mod prelude;

mod socket_server;
pub use socket_server::*;
//mod old_socket_server;  // TODO REMOVE THIS ONCE THE ABOVE IS COMPLETE

mod socket_client;
pub use socket_client::*;