// `no_std` for every shipping build; `std` whenever the `test` cfg is set, so
// the unit tests can use std helpers (and run under any feature set) while
// non-test builds still verify the core is no_std — see scripts/no-std-esp-build.sh.
#![cfg_attr(not(any(feature = "std", test)), no_std)]
#![doc = "Pure Reticulum engine and wire contract scaffold."]

pub mod announce;
pub mod crypto;
pub mod engine;
pub mod host;
pub mod outbox;
pub mod path;
mod payload_store;
pub mod runtime;
pub mod schedule;
pub mod wire;
