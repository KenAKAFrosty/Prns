//! RNS 1.3.5 `Destination.register_request_handler`: rows keyed by `(destination, truncated_hash(path))`, each carrying its allow policy and, for [`RequestPolicy::AllowList`], the permitted identity hashes. A request with no handler, or one its policy refuses, dies silently: the reference's exact posture.

pub mod core;
mod impls;
pub use impls::*;

pub use self::core::*;
