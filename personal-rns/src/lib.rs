// `no_std` for every shipping build; `std` whenever the `test` cfg is set, so
// the unit tests can use std helpers (and run under any feature set) while
// non-test builds still verify the core is no_std — see scripts/no-std-esp-build.sh.
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
// Intra-doc links are load-bearing navigation; a broken one is a hard error so
// a rename can't silently rot the docs. Run `cargo doc` (the fmt-check gate's
// doc step) to enforce.
#![deny(rustdoc::broken_intra_doc_links)]

// Make the `alloc` crate addressable as `alloc::*` for any module gated on the
// `alloc` feature. Under std the alloc crate is reachable via `std::*` paths
// too, but `alloc::*` paths only resolve when the crate is extern-declared
// explicitly.
#[cfg(feature = "alloc")]
extern crate alloc;

pub mod crypto;
pub mod engine;
pub mod identity;
pub mod interfaces;
pub mod routing;
pub mod runtime;
pub mod wire;
