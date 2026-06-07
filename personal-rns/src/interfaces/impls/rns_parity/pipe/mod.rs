//! WIP — unreviewed (PipeInterface, rns_parity). API, naming, and structure may still change.

pub mod core;
pub use core::{descriptor, PIPE_MTU};

mod impls;
pub use impls::*;
