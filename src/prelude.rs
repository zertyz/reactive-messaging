//! Re-exports of types -- out of the internal directory and module structure -- useful for users of this crate

pub use crate::config::*;
pub use crate::types::*;
pub use crate::socket_server::*;
pub use crate::socket_client::*;
pub use crate::socket_connection::peer::Peer;
pub use crate::{ron_serializer, ron_deserializer};
pub use crate::ReactiveMessagingSerializer;
pub use crate::ReactiveMessagingDeserializer;

// from `reactive-mutiny`:
pub use reactive_mutiny::prelude::advanced::*;