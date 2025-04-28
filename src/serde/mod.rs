//! SERializers & DEserializers (traits & implementations) for our [SocketServer]

mod mmap_serde;
pub use mmap_serde::*;

mod variable_binary_serde;
pub use variable_binary_serde::*;

mod textual_serde;
pub use textual_serde::*;
