use personal_rns::interfaces::usb_auto::{
    UsbControllerEnrollment, UsbControllerEnrollmentBusy, UsbControllerEnrollmentStatus,
};
use personal_rns::remote_control::RemoteControlControllerGrant;

#[cfg(any(target_arch = "riscv32", target_arch = "xtensa"))]
mod runtime;
#[cfg(any(target_arch = "riscv32", target_arch = "xtensa"))]
pub(crate) use runtime::{run, serial_enrollment};

#[derive(Debug, PartialEq, Eq)]
enum Admission {
    New,
    Duplicate,
}

struct AcceptedEnrollment {
    transaction: u32,
    grant: RemoteControlControllerGrant,
    status: UsbControllerEnrollmentStatus,
}

/// One in-flight enrollment, retained independently of the USB connection.
/// Only the latest transaction is remembered; this is not a durable replay ledger.
struct EnrollmentState {
    latest: Option<AcceptedEnrollment>,
}

impl EnrollmentState {
    const fn new() -> Self {
        Self { latest: None }
    }

    fn status(&self) -> UsbControllerEnrollmentStatus {
        self.latest
            .as_ref()
            .map_or(UsbControllerEnrollmentStatus::Idle, |latest| latest.status)
    }

    fn admit(
        &mut self,
        enrollment: &UsbControllerEnrollment,
    ) -> Result<Admission, UsbControllerEnrollmentBusy> {
        if let Some(latest) = &self.latest {
            if latest.transaction == enrollment.transaction {
                return if latest.grant == enrollment.grant {
                    Ok(Admission::Duplicate)
                } else {
                    Err(UsbControllerEnrollmentBusy)
                };
            }
            if matches!(latest.status, UsbControllerEnrollmentStatus::Pending { .. }) {
                return Err(UsbControllerEnrollmentBusy);
            }
        }
        self.latest = Some(AcceptedEnrollment {
            transaction: enrollment.transaction,
            grant: enrollment.grant,
            status: UsbControllerEnrollmentStatus::Pending {
                transaction: enrollment.transaction,
            },
        });
        Ok(Admission::New)
    }

    fn settle(&mut self, transaction: u32, saved: bool) {
        if let Some(latest) = &mut self.latest {
            if latest.transaction == transaction
                && matches!(latest.status, UsbControllerEnrollmentStatus::Pending { .. })
            {
                latest.status = if saved {
                    UsbControllerEnrollmentStatus::Saved { transaction }
                } else {
                    UsbControllerEnrollmentStatus::Failed { transaction }
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_rns::identity::PrivateIdentityMaterial;
    use personal_rns::remote_control::{
        RemoteControlControllerAuthority, RemoteControlControllerIdentity, RemoteControlRequestSet,
    };

    fn request(transaction: u32, seed: u8) -> UsbControllerEnrollment {
        let identity = PrivateIdentityMaterial::from_bytes([seed; 64]);
        UsbControllerEnrollment {
            transaction,
            grant: RemoteControlControllerGrant::new(
                RemoteControlControllerIdentity::new(identity.public().public_keys()),
                RemoteControlControllerAuthority::Administrator,
                RemoteControlRequestSet::all(),
            )
            .unwrap(),
        }
    }

    #[test]
    fn pending_requests_are_bounded_and_duplicates_preserve_the_full_grant() {
        let mut state = EnrollmentState::new();
        assert_eq!(state.status(), UsbControllerEnrollmentStatus::Idle);
        assert_eq!(state.admit(&request(7, 1)), Ok(Admission::New));
        assert_eq!(state.admit(&request(7, 1)), Ok(Admission::Duplicate));
        assert_eq!(
            state.admit(&request(7, 2)),
            Err(UsbControllerEnrollmentBusy)
        );
        assert_eq!(
            state.admit(&request(8, 1)),
            Err(UsbControllerEnrollmentBusy)
        );
        state.settle(8, true);
        assert_eq!(
            state.status(),
            UsbControllerEnrollmentStatus::Pending { transaction: 7 }
        );
    }

    #[test]
    fn saved_transaction_is_not_reenrolled_after_reconnect_or_revocation() {
        let mut state = EnrollmentState::new();
        state.admit(&request(7, 1)).unwrap();
        state.settle(7, true);
        assert_eq!(
            state.status(),
            UsbControllerEnrollmentStatus::Saved { transaction: 7 }
        );
        // USB reconnect does not recreate this owner, and revocation elsewhere does
        // not turn an old enrollment receipt into a fresh grant command.
        assert_eq!(state.admit(&request(7, 1)), Ok(Admission::Duplicate));
        assert_eq!(
            state.admit(&request(7, 2)),
            Err(UsbControllerEnrollmentBusy)
        );
        assert_eq!(
            state.status(),
            UsbControllerEnrollmentStatus::Saved { transaction: 7 }
        );
        assert_eq!(state.admit(&request(8, 1)), Ok(Admission::New));
    }

    #[test]
    fn failed_transaction_needs_an_explicit_new_transaction_to_retry() {
        let mut state = EnrollmentState::new();
        state.admit(&request(7, 1)).unwrap();
        state.settle(7, false);
        assert_eq!(
            state.status(),
            UsbControllerEnrollmentStatus::Failed { transaction: 7 }
        );
        assert_eq!(state.admit(&request(7, 1)), Ok(Admission::Duplicate));
        assert_eq!(state.admit(&request(8, 1)), Ok(Admission::New));
        state.settle(8, true);
        assert_eq!(
            state.status(),
            UsbControllerEnrollmentStatus::Saved { transaction: 8 }
        );
    }
}
