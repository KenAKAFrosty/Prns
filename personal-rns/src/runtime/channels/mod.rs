//! Per-platform seam + snapshot-channel types the runtime and its interfaces wire
//! through.
//!
//! - `embassy_seam` / `std_host`: the per-interface three-lane seam (inbound,
//!   outbound, control) a [`ContractRuntime`](super::ContractRuntime) pools — embassy
//!   (`embassy_sync` channels, no_std) and std (rtrb rings + threads).
//! - `embassy`: the snapshot `Watch` an app subscribes to.

#[cfg(feature = "embassy-host")]
pub mod embassy;

/// The contract seam for embassy platforms — gated on the narrow `embassy-seam`
/// feature (just `embassy-sync` + `embassy-time`, no `embassy-net`), so it is
/// compile-checkable on the host toolchain. `embassy-host` enables it too.
#[cfg(feature = "embassy-seam")]
pub mod embassy_seam;

#[cfg(feature = "std-host")]
pub mod std_host;
