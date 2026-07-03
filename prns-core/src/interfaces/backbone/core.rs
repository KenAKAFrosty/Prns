//! The host-agnostic core of the Backbone interface. Backbone is wire-identical to TCP, so the
//! sizing brain *is* TCP's, re-exported from [`tcp::core`](super::super::tcp::core) rather than
//! duplicated; the descriptor is kind-agnostic, so a Backbone interface mints its own
//! Backbone-kind id and passes it through.
//!
//! Only the bitrate *guesses* are Backbone's own, mirroring the reference's two `BITRATE_GUESS`
//! constants. Both clamp to the engine's MTU ceiling through TCP's tier table, so on our wire
//! they declare the same MTU TCP would; `BackboneInterface`'s 1 MiB `HW_MTU` is a Python autoconfigure ceiling far above what we carry.

pub use crate::interfaces::tcp::core::{
    descriptor, FRAMED_LEN, FRAME_CAP, READ_BUF_LEN, TCP_HW_MTU_CAP as HW_MTU_CAP,
};

/// What the listener and its spawned server-side connections claim about their pipe when the config
/// gives no bitrate — the reference's `BackboneInterface.BITRATE_GUESS` (1 Gbps). A real figure from
/// config overrides it, in either direction.
pub const BACKBONE_BITRATE_GUESS_BPS: u32 = 1_000_000_000;

/// What an outbound connector claims when the config gives no bitrate — the reference's
/// `BackboneClientInterface.BITRATE_GUESS` (100 Mbps). Both guesses land at the same declared MTU on
/// our wire (the tier table tops out below the engine ceiling), so the distinction is honesty about
/// the pipe, not a wire difference.
pub const BACKBONE_CLIENT_BITRATE_GUESS_BPS: u32 = 100_000_000;
