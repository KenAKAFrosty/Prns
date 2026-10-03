use super::UsbPairingDiscovery;
use crate::domain_primitives::PairingCandidate;
use pipecircuit::StepInputOf;
use prns_core::remote_control::RemoteControlPairingAvailabilityObservation;

pub struct ObserveUsbPairingAvailability<'a> {
    pub observation: RemoteControlPairingAvailabilityObservation<'a>,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum ObserveUsbPairingAvailabilityOutcome {
    Added { candidate: PairingCandidate },
    Updated { candidate: PairingCandidate },
    StaleObservation,
    WrongInterface,
    Expired,
    CapacityExceeded { maximum: usize },
}

#[derive(Debug, PartialEq, Eq)]
pub enum ObserveUsbPairingAvailabilityError {
    ObservedInFuture,
}

impl<const CAPACITY: usize> StepInputOf<UsbPairingDiscovery<CAPACITY>>
    for ObserveUsbPairingAvailability<'_>
{
    type Outcome = Result<ObserveUsbPairingAvailabilityOutcome, ObserveUsbPairingAvailabilityError>;

    fn step(self, discovery: &mut UsbPairingDiscovery<CAPACITY>) -> Self::Outcome {
        if self.observation.source_interface() != discovery.interface {
            return Ok(ObserveUsbPairingAvailabilityOutcome::WrongInterface);
        }
        if self.observation.observed_at() > discovery.now {
            return Err(ObserveUsbPairingAvailabilityError::ObservedInFuture);
        }
        if self.observation.expires_at() <= discovery.now {
            return Ok(ObserveUsbPairingAvailabilityOutcome::Expired);
        }
        let candidate = PairingCandidate::from(&self.observation);
        if let Some(existing) = discovery
            .candidates
            .iter_mut()
            .find(|existing| existing.endpoint() == candidate.endpoint())
        {
            if candidate.observed_at() <= existing.observed_at() {
                return Ok(ObserveUsbPairingAvailabilityOutcome::StaleObservation);
            }
            *existing = candidate;
            return Ok(ObserveUsbPairingAvailabilityOutcome::Updated { candidate });
        }
        match discovery.candidates.push(candidate) {
            Ok(()) => Ok(ObserveUsbPairingAvailabilityOutcome::Added { candidate }),
            Err(_) => {
                Ok(ObserveUsbPairingAvailabilityOutcome::CapacityExceeded { maximum: CAPACITY })
            }
        }
    }
}
