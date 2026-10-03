use super::UsbPairingDiscovery;
use pipecircuit::StepInputOf;
use prns_core::units::InstantMillis;

pub struct AdvanceUsbPairingDiscovery {
    pub now: InstantMillis,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum AdvanceUsbPairingDiscoveryOutcome {
    Advanced { expired: usize },
}

#[derive(Debug, PartialEq, Eq)]
pub enum AdvanceUsbPairingDiscoveryError {
    TimeWentBackwards { current: InstantMillis },
}

impl<const CAPACITY: usize> StepInputOf<UsbPairingDiscovery<CAPACITY>>
    for AdvanceUsbPairingDiscovery
{
    type Outcome = Result<AdvanceUsbPairingDiscoveryOutcome, AdvanceUsbPairingDiscoveryError>;

    fn step(self, discovery: &mut UsbPairingDiscovery<CAPACITY>) -> Self::Outcome {
        if self.now < discovery.now {
            return Err(AdvanceUsbPairingDiscoveryError::TimeWentBackwards {
                current: discovery.now,
            });
        }
        discovery.now = self.now;
        let before = discovery.candidates.len();
        discovery
            .candidates
            .retain(|candidate| candidate.expires_at() > self.now);
        Ok(AdvanceUsbPairingDiscoveryOutcome::Advanced {
            expired: before.saturating_sub(discovery.candidates.len()),
        })
    }
}

pub struct ClearUsbPairingCandidates;

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub struct ClearUsbPairingCandidatesOutcome {
    pub removed: usize,
}

impl<const CAPACITY: usize> StepInputOf<UsbPairingDiscovery<CAPACITY>>
    for ClearUsbPairingCandidates
{
    type Outcome = ClearUsbPairingCandidatesOutcome;

    fn step(self, discovery: &mut UsbPairingDiscovery<CAPACITY>) -> Self::Outcome {
        let removed = discovery.candidates.len();
        discovery.candidates.clear();
        ClearUsbPairingCandidatesOutcome { removed }
    }
}
