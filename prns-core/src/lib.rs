#![cfg_attr(not(any(feature = "std", test)), no_std)]
// `forbid` (unlike `deny`) cannot be locally re-enabled with `#[allow]` — the
// load-bearing guarantee behind the 100% safe engine; `unsafe` lives only at the
// platform host boundaries.
#![forbid(unsafe_code)]
#![doc = "Reticulum"]
#![deny(rustdoc::broken_intra_doc_links)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod crypto;
pub mod engine;
pub mod identity;
pub mod interfaces;
pub mod reactor;
pub mod routing;
pub mod storage;
pub mod units;
pub mod wire;
