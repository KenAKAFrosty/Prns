//! Bounded Ethernet datagrams around the production HaLoW supervisor.
#![expect(
    clippy::expect_used,
    reason = "bounded simulation fails at its owning invariant"
)]

use crate::{SimulationTick, TransmissionAction};
use personal_rns::interfaces::wifi_halow::PeerMac;
use personal_rns::wifi_halow::Destination;
use std::num::NonZeroUsize;

mod backend;
mod medium;
pub use backend::VirtualHaLowRadio;
pub use medium::VirtualHaLowMedium;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RadioId(u64);
impl RadioId {
    pub const fn get(self) -> u64 {
        self.0
    }
}

pub struct HaLowMediumLimits {
    pub radios: NonZeroUsize,
    pub receive_datagrams: NonZeroUsize,
    pub pending_deliveries: NonZeroUsize,
    pub trace_events: NonZeroUsize,
    pub armed_faults: NonZeroUsize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AttachError {
    Capacity,
    AddressInUse,
    IdsExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendBehavior {
    Ready,
    Stalled,
    Failed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiveBehavior {
    Ready,
    Failed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathState {
    Reachable,
    BroadcastOnly,
    Isolated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultDestination {
    Any,
    Broadcast,
    Peer(PeerMac),
}
impl FaultDestination {
    fn matches(self, target: Destination) -> bool {
        match self {
            Self::Any => true,
            Self::Broadcast => target == Destination::Broadcast,
            Self::Peer(peer) => target == Destination::Peer(peer),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NextDatagramFault {
    pub from: RadioId,
    pub destination: FaultDestination,
    pub action: TransmissionAction,
}
#[derive(Debug, PartialEq, Eq)]
pub enum ArmFaultError {
    Capacity,
    Detached,
    NoncanonicalDuplicate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryOutcome {
    Queued,
    FaultLoss,
    ReceiveQueueFull,
    PendingCapacity,
    Detached,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HaLowEvent {
    Attached {
        radio: RadioId,
        mac: PeerMac,
    },
    Detached {
        radio: RadioId,
    },
    Path {
        from: RadioId,
        to: RadioId,
        state: PathState,
        at: SimulationTick,
    },
    SendBehavior {
        radio: RadioId,
        behavior: SendBehavior,
        at: SimulationTick,
    },
    ReceiveBehavior {
        radio: RadioId,
        behavior: ReceiveBehavior,
        at: SimulationTick,
    },
    FaultArmed {
        fault: NextDatagramFault,
        at: SimulationTick,
    },
    Transmitted {
        ordinal: u64,
        from: RadioId,
        destination: Destination,
        at: SimulationTick,
        bytes: Vec<u8>,
    },
    Scheduled {
        ordinal: u64,
        to: RadioId,
        copy: u8,
        at: SimulationTick,
    },
    Delivery {
        ordinal: u64,
        to: RadioId,
        copy: u8,
        at: SimulationTick,
        outcome: DeliveryOutcome,
    },
    Injected {
        from: PeerMac,
        to: RadioId,
        at: SimulationTick,
        bytes: Vec<u8>,
        outcome: DeliveryOutcome,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HaLowSnapshot {
    pub radios: usize,
    pub queued: usize,
    pub pending: usize,
    pub armed_faults: usize,
    pub peak_queued: usize,
    pub peak_pending: usize,
    pub events: Vec<HaLowEvent>,
    pub retention: TraceRetention,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceRetention {
    Complete,
    Exhausted { omitted_events: u64 },
}

#[cfg(test)]
mod tests;
