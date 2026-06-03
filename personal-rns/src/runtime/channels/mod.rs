//! Per-platform runtime snapshot-delivery channel. (The per-interface seam lanes
//! moved to [`interfaces::substrate`](crate::interfaces::substrate).)

#[cfg(feature = "embassy-host")]
pub mod embassy;
