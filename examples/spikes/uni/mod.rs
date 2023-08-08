//! This module contains models for the parts of the `reactive-mutiny` library that we care about.
//! The mentioned library gives us "reactive programming", by allowing us to use a `Channel` to
//! send events that will be received by a `Stream` -- which will be executed to completion.

mod uni;
pub use uni::*;

pub mod channel;