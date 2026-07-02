#![cfg_attr(not(any(feature = "std", test)), no_std)]
#![forbid(unsafe_code)]
#![doc = "Reticulum"]
#![deny(rustdoc::broken_intra_doc_links)]

pub use prns_runtime::*;

mod lane_guards;
pub mod prelude;
pub use prelude::*;
