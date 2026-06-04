#![cfg_attr(not(any(feature = "std", test)), no_std)]
// Compiler-enforced memory safety. The pure engine contains *zero* `unsafe`, and
// `forbid` (unlike `deny`) cannot be locally re-enabled with `#[allow]` — a future
// `unsafe {}` anywhere in this crate is a hard compile error, not a review judgment
// call. This is the load-bearing guarantee behind "100% safe Rust engine": it holds
// across every feature combination (including `stream-compression`, so compression
// can never reach for an FFI codec the way a typical port does). The only `unsafe`
// in the suite lives at the FFI boundary (personal-rns-ffi), by design.
#![forbid(unsafe_code)]
#![doc = "Reticulum"]
#![deny(rustdoc::broken_intra_doc_links)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod crypto;
pub mod engine;
pub mod identity;
pub mod interfaces;
pub mod routing;
pub mod runtime;
pub mod wire;
