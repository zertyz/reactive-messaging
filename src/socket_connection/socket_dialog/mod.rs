//! Contains abstractions and implementations for the various [SocketDialog]s
//! we support:
//!  * [MmapBinarySocketDialog]
//!  * [SerializedBinarySocketDialog]
//!  * [TextualSocketDialog]
//!
//! While developing, unit testing, integration testing, e2e testing... for debug builds, in general,
//! prefer [TextualSocketDialog] -- as they offer a great debugging & testing ability.
//!
//! For production, use [MmapBinarySocketDialog] if possible, as they offer the best performance.
//!
//! Use [SerializeBinarySocketDialog] only when your data cannot be m-mapped -- for instance, when
//! you need to send `HashMaps`, free sized Strings (like in `VARCHAR`), etc.

pub mod dialog_types;
pub mod mmap_binary_dialog;
pub mod serialized_binary_dialog;
pub mod textual_dialog;