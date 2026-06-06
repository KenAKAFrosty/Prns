//! App-issued commands, ingested by the engine as plain data.
//! Commands cross thread, task, and FFI boundaries as owned values,
//! so any host can queue them and the engine cycle drains them deterministically.
//!
//! RNS 1.3.1 has no scheduled announces at all: `Destination.announce()` is
//! app-called, and periodic announcing lives in app land (LXMF runs its own
//! timers). So [`AnnounceNow`] is the reference primitive, and this engine's
//! re-announce schedule is the extension built ahead of it.

use crate::engine::self_announce::SelfAnnounceAppData;
use crate::interfaces::InterfaceId;
use crate::wire::DestinationHash;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineCommand {
    AnnounceNow(AnnounceNow),
}

/// `Destination.announce(app_data=…, attached_interface=…)` as data
/// (RNS 1.3.1 Destination.py).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnounceNow {
    pub destination: DestinationHash,
    pub target: AnnounceTarget,
    pub app_data: AnnounceAppData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceTarget {
    AllInterfaces,
    Interface(InterfaceId),
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnnounceAppData {
    Scheduled,
    Data(SelfAnnounceAppData),
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    OwesAnnounce(AnnounceNow),
    AnnounceRejected(AnnounceNowError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceNowError {
    UnknownDestination,
    NotASingleDestination,
    AppDataTooLong,
    UnknownInterface,
}
