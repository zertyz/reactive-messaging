//! Common types used across this crate

/// The fastest channel for `Stream`s -- see `benches/streamable_channels.rs`
pub(crate) type AtomicChannel<ItemType, const BUFFER_SIZE: usize> = reactive_mutiny::uni::channels::movable::atomic::Atomic::<'static, ItemType, BUFFER_SIZE, 1>;