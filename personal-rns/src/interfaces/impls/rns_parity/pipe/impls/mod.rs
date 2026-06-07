//! WIP — unreviewed (PipeInterface, rns_parity). API, naming, and structure may still change.

#[cfg(feature = "std-sync-host")]
mod std;
#[cfg(feature = "std-sync-host")]
pub use std::std_pipe_interface;
