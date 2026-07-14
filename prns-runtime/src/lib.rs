#![cfg_attr(not(any(feature = "std", test)), no_std)]
// Same compiler-enforced guarantee as `prns-core` (rationale there): the runtime is
// channel plumbing and async orchestration over the pure engine, so it too contains
// *zero* `unsafe` across every feature combination.
#![forbid(unsafe_code)]
#![doc = "The high-level Personal Reticulum runtime over the pure `prns-core` engine"]
#![deny(rustdoc::broken_intra_doc_links)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "log")]
#[allow(unused_imports)]
pub(crate) mod diagnostic_log {
    pub(crate) use log::{debug, error, info, trace, warn};
}

#[cfg(not(feature = "log"))]
#[allow(unused_imports, unused_macros)]
pub(crate) mod diagnostic_log {
    macro_rules! disabled {
        ($($arg:tt)*) => {{
            if false {
                let _ = format_args!($($arg)*);
            }
        }};
    }

    pub(crate) use disabled as debug;
    pub(crate) use disabled as error;
    pub(crate) use disabled as info;
    pub(crate) use disabled as trace;
    pub(crate) use disabled as warn;
}

pub use prns_core::{
    crypto, engine, identity, interfaces, persistence, routing, storage, units, wire,
};

pub mod reactor;
pub mod runtime;
