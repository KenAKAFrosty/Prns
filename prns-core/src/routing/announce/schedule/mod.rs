//! Announces waiting to be retransmitted: RNS's `announce_table`, keyed by destination so a fresher announce supersedes the one already waiting.
//! Entries are destination + due time only; the announce bytes live in the routing table's app_data arena and are read back at emit time, so the freshest accept is the one re-emission with no second copy.

pub mod core;
mod impls;
pub use impls::*;

pub use self::core::*;
