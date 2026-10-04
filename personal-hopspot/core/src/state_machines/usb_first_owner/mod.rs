use pipecircuit::{StateMachine, StepInputOf};
use prns_core::interfaces::InterfaceId;
use prns_core::remote_control::{RemoteControlPairingAttemptId, RemoteControlPairingEndpoint};
use prns_core::units::InstantMillis;

#[cfg(test)]
mod architecture;
#[cfg(test)]
mod behavior;
#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbOwnershipRestore {
    Unowned,
    Owned,
    Uncertain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbFirstOwnerStatus {
    WaitingForUsb,
    Opening,
    Open {
        endpoint: RemoteControlPairingEndpoint,
    },
    Claimed {
        attempt: RemoteControlPairingAttemptId,
    },
    Disabled,
    Expired,
}

pub struct UsbFirstOwner {
    interface: InterfaceId,
    now: InstantMillis,
    expires_at: InstantMillis,
    status: UsbFirstOwnerStatus,
}

impl UsbFirstOwner {
    pub fn new(
        interface: InterfaceId,
        now: InstantMillis,
        expires_at: InstantMillis,
        ownership: UsbOwnershipRestore,
    ) -> Self {
        Self {
            interface,
            now,
            expires_at,
            status: match ownership {
                UsbOwnershipRestore::Unowned => UsbFirstOwnerStatus::WaitingForUsb,
                UsbOwnershipRestore::Owned | UsbOwnershipRestore::Uncertain => {
                    UsbFirstOwnerStatus::Disabled
                }
            },
        }
    }
    fn advance(&mut self, now: InstantMillis) -> Result<(), UsbFirstOwnerClockError> {
        if now < self.now {
            return Err(UsbFirstOwnerClockError::WentBackwards {
                current: self.now,
                observed: now,
            });
        }
        self.now = now;
        Ok(())
    }
}

impl StateMachine for UsbFirstOwner {}

#[derive(Debug, PartialEq, Eq)]
pub enum UsbFirstOwnerClockError {
    WentBackwards {
        current: InstantMillis,
        observed: InstantMillis,
    },
}

pub struct PrepareUsbFirstOwnerWindow {
    pub now: InstantMillis,
    pub connected_interface: InterfaceId,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum PrepareUsbFirstOwnerWindowOutcome {
    Open {
        interface: InterfaceId,
        expires_at: InstantMillis,
    },
    WrongInterface,
    Unavailable,
    Expired,
}

impl StepInputOf<UsbFirstOwner> for PrepareUsbFirstOwnerWindow {
    type Outcome = Result<PrepareUsbFirstOwnerWindowOutcome, UsbFirstOwnerClockError>;

    fn step(self, owner: &mut UsbFirstOwner) -> Self::Outcome {
        owner.advance(self.now)?;
        if self.connected_interface != owner.interface {
            return Ok(PrepareUsbFirstOwnerWindowOutcome::WrongInterface);
        }
        if owner.status != UsbFirstOwnerStatus::WaitingForUsb {
            return Ok(PrepareUsbFirstOwnerWindowOutcome::Unavailable);
        }
        if self.now >= owner.expires_at {
            owner.status = UsbFirstOwnerStatus::Expired;
            return Ok(PrepareUsbFirstOwnerWindowOutcome::Expired);
        }
        owner.status = UsbFirstOwnerStatus::Opening;
        Ok(PrepareUsbFirstOwnerWindowOutcome::Open {
            interface: owner.interface,
            expires_at: owner.expires_at,
        })
    }
}

pub struct BindUsbFirstOwnerWindow {
    pub endpoint: RemoteControlPairingEndpoint,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum BindUsbFirstOwnerWindowOutcome {
    Bound,
    Unavailable,
}

impl StepInputOf<UsbFirstOwner> for BindUsbFirstOwnerWindow {
    type Outcome = BindUsbFirstOwnerWindowOutcome;

    fn step(self, owner: &mut UsbFirstOwner) -> Self::Outcome {
        if owner.status != UsbFirstOwnerStatus::Opening {
            return Self::Outcome::Unavailable;
        }
        owner.status = UsbFirstOwnerStatus::Open {
            endpoint: self.endpoint,
        };
        Self::Outcome::Bound
    }
}

pub struct ApproveUsbFirstOwner {
    pub now: InstantMillis,
    pub endpoint: RemoteControlPairingEndpoint,
    pub attempt: RemoteControlPairingAttemptId,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum ApproveUsbFirstOwnerOutcome {
    Approve {
        attempt: RemoteControlPairingAttemptId,
    },
    Unavailable,
    WrongEndpoint,
    Expired,
}

impl StepInputOf<UsbFirstOwner> for ApproveUsbFirstOwner {
    type Outcome = Result<ApproveUsbFirstOwnerOutcome, UsbFirstOwnerClockError>;

    fn step(self, owner: &mut UsbFirstOwner) -> Self::Outcome {
        owner.advance(self.now)?;
        let UsbFirstOwnerStatus::Open { endpoint } = owner.status else {
            return Ok(ApproveUsbFirstOwnerOutcome::Unavailable);
        };
        if self.endpoint != endpoint {
            return Ok(ApproveUsbFirstOwnerOutcome::WrongEndpoint);
        }
        if self.now >= owner.expires_at {
            owner.status = UsbFirstOwnerStatus::Expired;
            return Ok(ApproveUsbFirstOwnerOutcome::Expired);
        }
        owner.status = UsbFirstOwnerStatus::Claimed {
            attempt: self.attempt,
        };
        Ok(ApproveUsbFirstOwnerOutcome::Approve {
            attempt: self.attempt,
        })
    }
}

pub struct ReadUsbFirstOwner;

#[derive(Debug, PartialEq, Eq)]
pub struct UsbFirstOwnerSnapshot {
    pub status: UsbFirstOwnerStatus,
    pub expires_at: InstantMillis,
}

impl StepInputOf<UsbFirstOwner> for ReadUsbFirstOwner {
    type Outcome = UsbFirstOwnerSnapshot;

    fn step(self, owner: &mut UsbFirstOwner) -> Self::Outcome {
        UsbFirstOwnerSnapshot {
            status: owner.status,
            expires_at: owner.expires_at,
        }
    }
}
