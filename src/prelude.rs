//! Re-exports of types useful for users of this crate

pub use crate::types::{
    ConnectionEvent,
    SocketProcessorDerivedType,
};
use crate::types::*;

pub use super::socket_connection_handler::Peer;

use reactive_mutiny::prelude::advanced::MutinyStream;
/// Type for the `Stream` we create when reading from the remote peer.\
/// This type is intended to be used only for the first level of `dialog_processor_builder()`s you pass to
/// the [SocketClient] or [SocketServer], as Rust Generics isn't able to infer a generic `Stream` type
/// in this situation (in which the `Stream` is created inside the generic function itself).\
/// If your logic uses functions that receive `Stream`s, you'll want flexibility to do whatever you want
/// with the `Stream` (which would no longer be a `MutinyStream`), so declare such functions as:
/// ```no_compile
///     fn dialog_processor<RemoteStreamType: Stream<Item=SocketProcessorDerivedType<RemoteMessages>>>
///                        (remote_messages_stream: RemoteStreamType) -> impl Stream<Item=LocalMessages> { ... }
pub type ProcessorRemoteStreamType<const BUFFERED_MESSAGES_PER_PEER_COUNT: usize,
                                   RemoteMessagesType>
    = MutinyStream<'static, RemoteMessagesType,
                            SocketProcessorChannelType<BUFFERED_MESSAGES_PER_PEER_COUNT, RemoteMessagesType>,
                            SocketProcessorDerivedType<BUFFERED_MESSAGES_PER_PEER_COUNT, RemoteMessagesType>>;

