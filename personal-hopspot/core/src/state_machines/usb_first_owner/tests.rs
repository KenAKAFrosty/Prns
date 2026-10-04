#![allow(clippy::unwrap_used)]

use super::*;
use prns_core::identity::IdentityHash;
use prns_core::remote_control::RemoteControlPairingIdentity;

const USB: InterfaceId = InterfaceId::new([1; 8]);
const OTHER: InterfaceId = InterfaceId::new([2; 8]);
const DEADLINE: InstantMillis = InstantMillis(100);

fn endpoint(seed: u8) -> RemoteControlPairingEndpoint {
    RemoteControlPairingIdentity::new(IdentityHash::new([seed; 16])).endpoint()
}

#[test]
fn only_confirmed_unowned_restore_can_open_once_on_the_selected_usb() {
    for ownership in [UsbOwnershipRestore::Owned, UsbOwnershipRestore::Uncertain] {
        let mut owner = UsbFirstOwner::new(USB, InstantMillis(0), DEADLINE, ownership);
        assert_eq!(
            owner
                .step(PrepareUsbFirstOwnerWindow {
                    now: InstantMillis(0),
                    connected_interface: USB
                })
                .unwrap(),
            PrepareUsbFirstOwnerWindowOutcome::Unavailable
        );
        assert_eq!(
            owner.step(ReadUsbFirstOwner),
            UsbFirstOwnerSnapshot {
                status: UsbFirstOwnerStatus::Disabled,
                expires_at: DEADLINE
            }
        );
    }
    let mut owner = UsbFirstOwner::new(
        USB,
        InstantMillis(0),
        DEADLINE,
        UsbOwnershipRestore::Unowned,
    );
    assert_eq!(
        owner.step(BindUsbFirstOwnerWindow {
            endpoint: endpoint(1)
        }),
        BindUsbFirstOwnerWindowOutcome::Unavailable
    );
    assert_eq!(
        owner
            .step(PrepareUsbFirstOwnerWindow {
                now: InstantMillis(0),
                connected_interface: OTHER
            })
            .unwrap(),
        PrepareUsbFirstOwnerWindowOutcome::WrongInterface
    );
    assert_eq!(
        owner
            .step(PrepareUsbFirstOwnerWindow {
                now: InstantMillis(99),
                connected_interface: USB
            })
            .unwrap(),
        PrepareUsbFirstOwnerWindowOutcome::Open {
            interface: USB,
            expires_at: DEADLINE
        }
    );
    assert_eq!(
        owner
            .step(PrepareUsbFirstOwnerWindow {
                now: InstantMillis(99),
                connected_interface: USB
            })
            .unwrap(),
        PrepareUsbFirstOwnerWindowOutcome::Unavailable
    );
    assert_eq!(
        owner.step(BindUsbFirstOwnerWindow {
            endpoint: endpoint(1)
        }),
        BindUsbFirstOwnerWindowOutcome::Bound
    );
    assert_eq!(
        owner.step(ReadUsbFirstOwner),
        UsbFirstOwnerSnapshot {
            status: UsbFirstOwnerStatus::Open {
                endpoint: endpoint(1)
            },
            expires_at: DEADLINE
        }
    );
}

#[test]
fn an_unconnected_boot_window_expires_at_the_deadline_and_cannot_reopen() {
    let mut owner = UsbFirstOwner::new(
        USB,
        InstantMillis(0),
        DEADLINE,
        UsbOwnershipRestore::Unowned,
    );
    assert_eq!(
        owner
            .step(PrepareUsbFirstOwnerWindow {
                now: DEADLINE,
                connected_interface: USB
            })
            .unwrap(),
        PrepareUsbFirstOwnerWindowOutcome::Expired
    );
    assert_eq!(
        owner.step(ReadUsbFirstOwner),
        UsbFirstOwnerSnapshot {
            status: UsbFirstOwnerStatus::Expired,
            expires_at: DEADLINE
        }
    );
    assert_eq!(
        owner
            .step(PrepareUsbFirstOwnerWindow {
                now: DEADLINE,
                connected_interface: USB
            })
            .unwrap(),
        PrepareUsbFirstOwnerWindowOutcome::Unavailable
    );
}

#[test]
fn only_the_bound_endpoint_can_claim_once_before_expiry_and_time_cannot_rewind() {
    use prns_core::remote_control::RemoteControlPairingAttemptId;
    let attempt = RemoteControlPairingAttemptId::from_test_transcript_digest_bytes([1; 32]);
    for expired in [false, true] {
        let mut owner = UsbFirstOwner::new(
            USB,
            InstantMillis(0),
            DEADLINE,
            UsbOwnershipRestore::Unowned,
        );
        let _opened = owner
            .step(PrepareUsbFirstOwnerWindow {
                now: InstantMillis(1),
                connected_interface: USB,
            })
            .unwrap();
        let _bound = owner.step(BindUsbFirstOwnerWindow {
            endpoint: endpoint(1),
        });
        assert_eq!(
            owner
                .step(ApproveUsbFirstOwner {
                    now: InstantMillis(2),
                    endpoint: endpoint(2),
                    attempt
                })
                .unwrap(),
            ApproveUsbFirstOwnerOutcome::WrongEndpoint
        );
        assert_eq!(
            owner.step(PrepareUsbFirstOwnerWindow {
                now: InstantMillis(1),
                connected_interface: USB
            }),
            Err(UsbFirstOwnerClockError::WentBackwards {
                current: InstantMillis(2),
                observed: InstantMillis(1)
            })
        );
        assert_eq!(
            owner.step(ApproveUsbFirstOwner {
                now: InstantMillis(1),
                endpoint: endpoint(1),
                attempt
            }),
            Err(UsbFirstOwnerClockError::WentBackwards {
                current: InstantMillis(2),
                observed: InstantMillis(1)
            })
        );
        let now = if expired { DEADLINE } else { InstantMillis(99) };
        assert_eq!(
            owner
                .step(ApproveUsbFirstOwner {
                    now,
                    endpoint: endpoint(1),
                    attempt
                })
                .unwrap(),
            if expired {
                ApproveUsbFirstOwnerOutcome::Expired
            } else {
                ApproveUsbFirstOwnerOutcome::Approve { attempt }
            }
        );
        assert_eq!(
            owner
                .step(ApproveUsbFirstOwner {
                    now,
                    endpoint: endpoint(1),
                    attempt
                })
                .unwrap(),
            ApproveUsbFirstOwnerOutcome::Unavailable
        );
        assert_eq!(
            owner.step(ReadUsbFirstOwner),
            UsbFirstOwnerSnapshot {
                expires_at: DEADLINE,
                status: if expired {
                    UsbFirstOwnerStatus::Expired
                } else {
                    UsbFirstOwnerStatus::Claimed { attempt }
                }
            }
        );
    }
}

proptest::proptest! {
    #[test]
    fn arbitrary_approval_times_match_the_window_and_single_claim_model(times in proptest::collection::vec(0u64..150, 0..80)) {
        use proptest::prop_assert_eq;
        let attempt = RemoteControlPairingAttemptId::from_test_transcript_digest_bytes([9; 32]);
        let mut owner = UsbFirstOwner::new(USB, InstantMillis(0), DEADLINE, UsbOwnershipRestore::Unowned);
        prop_assert_eq!(owner.step(PrepareUsbFirstOwnerWindow { now: InstantMillis(0), connected_interface: USB }).unwrap(), PrepareUsbFirstOwnerWindowOutcome::Open { interface: USB, expires_at: DEADLINE });
        prop_assert_eq!(owner.step(BindUsbFirstOwnerWindow { endpoint: endpoint(1) }), BindUsbFirstOwnerWindowOutcome::Bound);
        let mut clock = 0;
        let mut status = UsbFirstOwnerStatus::Open { endpoint: endpoint(1) };
        for time in times {
            let actual = owner.step(ApproveUsbFirstOwner { now: InstantMillis(time), endpoint: endpoint(1), attempt });
            let expected = if time < clock {
                Err(UsbFirstOwnerClockError::WentBackwards { current: InstantMillis(clock), observed: InstantMillis(time) })
            } else {
                clock = time;
                Ok(match status {
                    UsbFirstOwnerStatus::Open { .. } if time < DEADLINE.0 => {
                        status = UsbFirstOwnerStatus::Claimed { attempt };
                        ApproveUsbFirstOwnerOutcome::Approve { attempt }
                    }
                    UsbFirstOwnerStatus::Open { .. } => {
                        status = UsbFirstOwnerStatus::Expired;
                        ApproveUsbFirstOwnerOutcome::Expired
                    }
                    _ => ApproveUsbFirstOwnerOutcome::Unavailable,
                })
            };
            prop_assert_eq!(actual, expected);
            prop_assert_eq!(owner.step(ReadUsbFirstOwner), UsbFirstOwnerSnapshot { status, expires_at: DEADLINE });
        }
    }
}
