#![cfg_attr(not(any(feature = "std", test)), no_std)]
#![forbid(unsafe_code)]
#![doc = "Deterministic Reticulum engine & wire contract used by Prns"]
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
