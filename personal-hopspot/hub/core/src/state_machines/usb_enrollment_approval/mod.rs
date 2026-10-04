use crate::{BeginEnrollment, DeviceId, PairingCandidate};
use pipecircuit::{StateMachine, StepInputOf};
use prns_core::remote_control::{
    RemoteControlControllerAuthority, RemoteControlControllerIdentity,
    RemoteControlPairingAttemptId, RemoteControlPairingAvailabilityKind,
    RemoteControlPairingEndpoint, RemoteControlTargetIdentity,
};
use prns_core::units::InstantMillis;

#[cfg(test)]
mod architecture;
#[cfg(test)]
mod behavior;
#[cfg(test)]
mod tests;

pub struct UsbEnrollmentApproval {
    candidate: PairingCandidate,
    device: DeviceId,
    controller: RemoteControlControllerIdentity,
    now: InstantMillis,
    approved: bool,
}

impl UsbEnrollmentApproval {
    pub fn new(
        candidate: PairingCandidate,
        device: DeviceId,
        controller: RemoteControlControllerIdentity,
    ) -> Self {
        Self {
            now: candidate.observed_at(),
            candidate,
            device,
            controller,
            approved: false,
        }
    }
}

impl StateMachine for UsbEnrollmentApproval {}

pub struct ReviewUsbEnrollmentOffer {
    pub now: InstantMillis,
    pub endpoint: RemoteControlPairingEndpoint,
    pub controller: RemoteControlControllerIdentity,
    pub target: RemoteControlTargetIdentity,
    pub attempt: RemoteControlPairingAttemptId,
    pub authority: RemoteControlControllerAuthority,
    pub expires_at: InstantMillis,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReviewUsbEnrollmentOfferError {
    ClockWentBackwards {
        current: InstantMillis,
        observed: InstantMillis,
    },
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum ReviewUsbEnrollmentOfferOutcome {
    Approve { enrollment: BeginEnrollment },
    AlreadyApproved,
    InvitationRequired,
    WrongEndpoint,
    WrongController,
    InsufficientAuthority,
    Expired,
}

impl StepInputOf<UsbEnrollmentApproval> for ReviewUsbEnrollmentOffer {
    type Outcome = Result<ReviewUsbEnrollmentOfferOutcome, ReviewUsbEnrollmentOfferError>;

    fn step(self, owner: &mut UsbEnrollmentApproval) -> Self::Outcome {
        if self.now < owner.now {
            return Err(ReviewUsbEnrollmentOfferError::ClockWentBackwards {
                current: owner.now,
                observed: self.now,
            });
        }
        owner.now = self.now;
        if owner.approved {
            return Ok(ReviewUsbEnrollmentOfferOutcome::AlreadyApproved);
        }
        if owner.candidate.kind() != RemoteControlPairingAvailabilityKind::DirectPhysicalAvailable {
            return Ok(ReviewUsbEnrollmentOfferOutcome::InvitationRequired);
        }
        if self.endpoint != owner.candidate.endpoint() {
            return Ok(ReviewUsbEnrollmentOfferOutcome::WrongEndpoint);
        }
        if self.controller != owner.controller {
            return Ok(ReviewUsbEnrollmentOfferOutcome::WrongController);
        }
        if self.authority != RemoteControlControllerAuthority::Administrator {
            return Ok(ReviewUsbEnrollmentOfferOutcome::InsufficientAuthority);
        }
        if self.now >= self.expires_at || self.now >= owner.candidate.expires_at() {
            return Ok(ReviewUsbEnrollmentOfferOutcome::Expired);
        }
        owner.approved = true;
        Ok(ReviewUsbEnrollmentOfferOutcome::Approve {
            enrollment: BeginEnrollment {
                device: owner.device,
                attempt: self.attempt,
                target: self.target,
            },
        })
    }
}
