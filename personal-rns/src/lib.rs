// `no_std` for every shipping build; `std` whenever the `test` cfg is set, so
// the unit tests can use std helpers (and run under any feature set) while
// non-test builds still verify the core is no_std — see scripts/no-std-esp-build.sh.
#![cfg_attr(not(any(feature = "std", test)), no_std)]
#![doc = "Reticulum"]

// Make the `alloc` crate addressable as `alloc::*` whenever any module
// gated on the `alloc` feature is in scope (currently only
// `interfaces::loopback`). Under std the alloc crate is reachable via
// `std::*` paths too, but `alloc::*` paths only resolve when the crate
// is extern-declared explicitly.
#[cfg(feature = "alloc")]
extern crate alloc;

pub mod crypto;
pub mod engine;
pub mod identity;
pub mod interfaces;
pub mod routing;
pub mod wire;
