use hopspot_hub_core::*;
use personal_rns::interfaces::InterfaceId;
use personal_rns::remote_control::{
    RemoteControlPairingAvailabilityObservation, RemoteControlPairingEndpoint,
};
use personal_rns::units::InstantMillis;
use pipecircuit::{StateMachine, Switchboard};

#[cfg(test)]
mod behavior;
#[cfg(test)]
mod tests;

pub struct UsbDiscoverySwitchboard<const CAPACITY: usize> {
    discovery: UsbPairingDiscovery<CAPACITY>,
}

impl<const CAPACITY: usize> UsbDiscoverySwitchboard<CAPACITY> {
    pub fn new(interface: InterfaceId, now: InstantMillis) -> Self {
        Self {
            discovery: UsbPairingDiscovery::new(interface, now),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbDiscoveryIntent {
    Inspect,
    Select {
        endpoint: RemoteControlPairingEndpoint,
    },
    Clear,
}

pub enum UsbDiscoveryMessage<'a> {
    Available(RemoteControlPairingAvailabilityObservation<'a>),
    Intent(UsbDiscoveryIntent),
}

pub struct UsbDiscoveryInput<'a> {
    pub now: InstantMillis,
    pub message: UsbDiscoveryMessage<'a>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum UsbDiscoveryEvent {
    Availability(ObserveUsbPairingAvailabilityOutcome),
    Selection(SelectUsbPairingCandidateOutcome),
    Cleared(ClearUsbPairingCandidatesOutcome),
    Inspected,
}

#[derive(Debug, PartialEq, Eq)]
pub enum UsbDiscoveryRoutingError {
    Clock(AdvanceUsbPairingDiscoveryError),
    Observation(ObserveUsbPairingAvailabilityError),
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub struct UsbDiscoveryUpdate<const CAPACITY: usize> {
    pub expired: usize,
    pub event: UsbDiscoveryEvent,
    pub snapshot: UsbPairingCandidatesSnapshot<CAPACITY>,
}

impl<const CAPACITY: usize> Switchboard for UsbDiscoverySwitchboard<CAPACITY> {
    type From<'message> = UsbDiscoveryInput<'message>;
    type To<'message> = Result<UsbDiscoveryUpdate<CAPACITY>, UsbDiscoveryRoutingError>;

    fn route<'message>(&mut self, input: Self::From<'message>) -> Self::To<'message> {
        let AdvanceUsbPairingDiscoveryOutcome::Advanced { expired } = self
            .discovery
            .step(AdvanceUsbPairingDiscovery { now: input.now })
            .map_err(UsbDiscoveryRoutingError::Clock)?;
        let event = match input.message {
            UsbDiscoveryMessage::Available(observation) => UsbDiscoveryEvent::Availability(
                self.discovery
                    .step(ObserveUsbPairingAvailability { observation })
                    .map_err(UsbDiscoveryRoutingError::Observation)?,
            ),
            UsbDiscoveryMessage::Intent(UsbDiscoveryIntent::Inspect) => {
                UsbDiscoveryEvent::Inspected
            }
            UsbDiscoveryMessage::Intent(UsbDiscoveryIntent::Select { endpoint }) => {
                UsbDiscoveryEvent::Selection(
                    self.discovery.step(SelectUsbPairingCandidate { endpoint }),
                )
            }
            UsbDiscoveryMessage::Intent(UsbDiscoveryIntent::Clear) => {
                UsbDiscoveryEvent::Cleared(self.discovery.step(ClearUsbPairingCandidates))
            }
        };
        Ok(UsbDiscoveryUpdate {
            expired,
            event,
            snapshot: self.discovery.step(ReadUsbPairingCandidates),
        })
    }
}
