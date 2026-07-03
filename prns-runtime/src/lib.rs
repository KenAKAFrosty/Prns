#![cfg_attr(not(any(feature = "std", test)), no_std)]
// Same compiler-enforced guarantee as `prns-core` (rationale there): the runtime is
// channel plumbing and async orchestration over the pure engine, so it too contains
// *zero* `unsafe` across every feature combination.
#![forbid(unsafe_code)]
#![doc = "The high-level Personal Reticulum runtime over the pure `prns-core` engine"]
#![deny(rustdoc::broken_intra_doc_links)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub use prns_core::{crypto, engine, identity, interfaces, routing, storage, units, wire};

pub mod reactor;
pub mod runtime;
