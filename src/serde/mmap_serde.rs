use std::fmt::Debug;

/// Structs or Enums that implement this trait declares they can be serialized/deserialized
/// "as-is" in binary form. This leads to the best possible performance when compared to
/// the alternatives (to implement both [crate::prelude::ReactiveMessagingTextualSerializer] & [crate::prelude::ReactiveMessagingTextualDeserializer]).\
/// Usually, types that use only primitive types (such as the numbers, char, sub-types consisting only of
/// those, and arrays of those types) are safe to implement this trait. Note, however, that no endian,
/// floating point, nor any other conversions are made.\
/// An important property that arise from all that was exposed is that each payload has, inevitably,
/// a fixed size.\
/// IMPORTANT: implementing this trait for a type that is not memory mappable will lead to program crashing.
///            In the future, a proc-macro may be used to implement this trait while ensuring these constraints
///            and, specially, verifying that all subtypes also implement this trait properly.
/// CRITICALLY IMPORTANT: until the proc-macro is ready, bad use of this trait may break your program:
///                       example: you have a complex model with several reusable subtypes. Over time, a jr. Engineer
///                       add a new enum variant with a String field, unaware of this issue.
///                       Implications:
///                         1) The program will not break at first. Only when the new variant (with the new field) is used
///                            -- creating a "really hard to track" error;
///                         2) Any unit tests you may have written will not fail because of this new variant/field
///                            -- it would be required the jr. Engineer would write a test with their new variant/field
///                               so the serde error could be exposed;
///                         3) Nonetheless, even with these current limitations, proper code reviews and presence of
///                            automated tests can effectively block any issues from making it to production;
///                         4) This problem will be short-lived: issue **(n12)** will soon take care of it for good.
pub trait ReactiveMessagingMemoryMappable: PartialEq + Debug {}

