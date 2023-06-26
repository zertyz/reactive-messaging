//! Common types used across this crate

use super::config::*;
use std::time::Duration;
use reactive_mutiny::prelude::advanced::{MutinyStream, UniZeroCopyAtomic, ChannelCommon, ChannelUni, ChannelProducer, Instruments, ChannelUniZeroCopyAtomic, OgreUnique, AllocatorAtomicArray};


/// The fastest channel for sender `Stream`s -- see `benches/streamable_channels.rs`
pub(crate) type SenderChannel<ItemType, const BUFFER_SIZE: usize> = reactive_mutiny::uni::channels::movable::atomic::Atomic::<'static, ItemType, BUFFER_SIZE, 1>;

// Uni types for handling socket connections
pub(crate) type SocketProcessorUniType<MessagesType>     = UniZeroCopyAtomic<MessagesType, RECEIVER_BUFFER_SIZE, 1, SOCKET_PROCESSOR_INSTRUMENTS>;
pub(crate) type SocketProcessorChannelType<MessagesType> = ChannelUniZeroCopyAtomic<MessagesType, RECEIVER_BUFFER_SIZE, 1>;
pub(crate) type SocketProcessorDerivedType<MessagesType> = OgreUnique<MessagesType, AllocatorAtomicArray<MessagesType, RECEIVER_BUFFER_SIZE>>;
