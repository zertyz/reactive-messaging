//! Tests the server API contracts, in regard to how the usage is intended to be done -- what models
//! are accepted, how to use it with closures, methods, etc.
//!
//! NOTE: many "tests" here have no real assertions, as they are used only to verify the API is able to represent certain models;
//!       actually, many are not even executed, as their futures may simply be dropped.

#[cfg_attr(not(doc), test)]
fn test() {}