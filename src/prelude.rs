//! Re-exports of types useful for users of this crate

use crate::types::*;
use reactive_mutiny::prelude::advanced::{MutinyStream};


/// Type for the `Stream` of messages coming from the remote peer
pub type ProcessorRemoteStreamType<MessagesType> = MutinyStream<'static, MessagesType, SocketProcessorChannelType<MessagesType>, SocketProcessorDerivedType<MessagesType>>;

/// Types for the dialog processors
pub type Peer<MessageType> = super::socket_connection_handler::Peer<MessageType>;