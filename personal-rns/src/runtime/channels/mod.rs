//! Per-platform runtime snapshot-delivery channel. (The per-interface seam lanes
//! moved to [`interfaces::substrate`](crate::interfaces::substrate).)

// Just `embassy-sync` (a `Watch`) — no embassy-net/LoRa — so a contract-only board
// like the Hopspot S3 can subscribe its OLED to the snapshot stream.
#[cfg(feature = "embassy-seam")]
pub mod embassy;
