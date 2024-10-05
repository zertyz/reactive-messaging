#![doc = include_str!("../README.md")]


pub(crate) mod types;
pub(crate) mod config;

pub mod socket_connection;

pub(crate) mod serde;
pub(crate) mod socket_services;
pub mod prelude;

#[cfg(any(test,doc))]
mod unit_test_utils;