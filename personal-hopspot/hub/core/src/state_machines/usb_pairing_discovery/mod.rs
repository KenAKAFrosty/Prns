#[cfg(test)]
mod architecture;
#[cfg(test)]
mod behavior;
mod observe;
mod query;
#[cfg(test)]
pub(super) mod tests;
mod time;

use crate::domain_primitives::PairingCandidate;
use pipecircuit::StateMachine;
use prns_core::interfaces::InterfaceId;
use prns_core::units::InstantMillis;

pub use observe::{
    ObserveUsbPairingAvailability, ObserveUsbPairingAvailabilityError,
    ObserveUsbPairingAvailabilityOutcome,
};
pub use query::{
    ReadUsbPairingCandidates, SelectUsbPairingCandidate, SelectUsbPairingCandidateOutcome,
    UsbPairingCandidatesSnapshot,
};
pub use time::{
    AdvanceUsbPairingDiscovery, AdvanceUsbPairingDiscoveryError, AdvanceUsbPairingDiscoveryOutcome,
    ClearUsbPairingCandidates, ClearUsbPairingCandidatesOutcome,
};

pub struct UsbPairingDiscovery<const CAPACITY: usize> {
    interface: InterfaceId,
    now: InstantMillis,
    candidates: heapless::Vec<PairingCandidate, CAPACITY>,
}

impl<const CAPACITY: usize> UsbPairingDiscovery<CAPACITY> {
    pub fn new(interface: InterfaceId, now: InstantMillis) -> Self {
        Self {
            interface,
            now,
            candidates: heapless::Vec::new(),
        }
    }
}

impl<const CAPACITY: usize> StateMachine for UsbPairingDiscovery<CAPACITY> {}
