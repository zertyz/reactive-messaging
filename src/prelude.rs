//! Re-exports of types useful for users of this crate

pub use crate::config::*;
pub use crate::types::*;
pub use crate::socket_server::*;
pub use crate::socket_client::*;
pub use super::socket_connection::Peer;
pub use crate::{ron_serializer, ron_deserializer};

// from `reactive-mutiny`:
pub use reactive_mutiny::prelude::advanced::*;