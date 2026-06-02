//! Per-platform seam and snapshot-channel types.

#[cfg(feature = "embassy-host")]
pub mod embassy;

/// Contract seam for embassy platforms.
#[cfg(feature = "embassy-seam")]
pub mod embassy_seam;

#[cfg(feature = "std-host")]
pub mod std_host;
