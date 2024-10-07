pub mod protocol_model;
pub mod logic;


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