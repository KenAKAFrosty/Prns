use super::UsbPairingDiscovery;
use crate::domain_primitives::PairingCandidate;
use pipecircuit::StepInputOf;
use prns_core::interfaces::InterfaceId;
use prns_core::remote_control::RemoteControlPairingEndpoint;
use prns_core::units::InstantMillis;

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub struct UsbPairingCandidatesSnapshot<const CAPACITY: usize> {
    pub interface: InterfaceId,
    pub as_of: InstantMillis,
    pub candidates: heapless::Vec<PairingCandidate, CAPACITY>,
}

pub struct ReadUsbPairingCandidates;

impl<const CAPACITY: usize> StepInputOf<UsbPairingDiscovery<CAPACITY>>
    for ReadUsbPairingCandidates
{
    type Outcome = UsbPairingCandidatesSnapshot<CAPACITY>;

    fn step(self, discovery: &mut UsbPairingDiscovery<CAPACITY>) -> Self::Outcome {
        UsbPairingCandidatesSnapshot {
            interface: discovery.interface,
            as_of: discovery.now,
            candidates: discovery.candidates.clone(),
        }
    }
}

pub struct SelectUsbPairingCandidate {
    pub endpoint: RemoteControlPairingEndpoint,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum SelectUsbPairingCandidateOutcome {
    Selected { candidate: PairingCandidate },
    Unavailable,
}

impl<const CAPACITY: usize> StepInputOf<UsbPairingDiscovery<CAPACITY>>
    for SelectUsbPairingCandidate
{
    type Outcome = SelectUsbPairingCandidateOutcome;

    fn step(self, discovery: &mut UsbPairingDiscovery<CAPACITY>) -> Self::Outcome {
        match discovery
            .candidates
            .iter()
            .find(|candidate| candidate.endpoint() == self.endpoint)
        {
            Some(&candidate) => SelectUsbPairingCandidateOutcome::Selected { candidate },
            None => SelectUsbPairingCandidateOutcome::Unavailable,
        }
    }
}
