#![allow(clippy::unwrap_used, clippy::panic)]
use super::super::usb_pairing_discovery::tests::with_observation_kind;
use super::*;
use crate::{CreateDevice, CreateDeviceOutcome, DeviceLabel, DeviceRegistry};
use core::num::NonZeroU32;
use prns_core::identity::IdentitySigner;
use prns_core::identity::in_memory::InMemoryNodeIdentity;
use prns_core::interfaces::InterfaceId;

fn controller(seed: u8) -> RemoteControlControllerIdentity {
    {
        let signer = InMemoryNodeIdentity::from_secret_key_bytes(&[seed; 64]);
        RemoteControlControllerIdentity::new(prns_core::identity::IdentityPublicKeys {
            encryption: signer.encryption_public_key(),
            signing: signer.signing_public_key(),
        })
    }
}

fn candidate(kind: RemoteControlPairingAvailabilityKind) -> PairingCandidate {
    with_observation_kind(1, InterfaceId::new([1; 8]), 10, 100, kind, |observation| {
        PairingCandidate::from(&observation)
    })
}

fn owner(kind: RemoteControlPairingAvailabilityKind) -> UsbEnrollmentApproval {
    let mut registry = DeviceRegistry::try_new(NonZeroU32::new(1).unwrap()).unwrap();
    let CreateDeviceOutcome::Created { device } = registry
        .step(CreateDevice {
            label: DeviceLabel::new("T-Beam").unwrap(),
        })
        .unwrap()
    else {
        panic!("capacity");
    };
    UsbEnrollmentApproval::new(candidate(kind), device, controller(2))
}

fn offer(owner: &UsbEnrollmentApproval, now: u64) -> ReviewUsbEnrollmentOffer {
    ReviewUsbEnrollmentOffer {
        now: InstantMillis(now),
        endpoint: owner.candidate.endpoint(),
        controller: owner.controller,
        target: RemoteControlTargetIdentity::new(*controller(3).public_keys()),
        attempt: RemoteControlPairingAttemptId::from_test_transcript_digest_bytes([4; 32]),
        authority: RemoteControlControllerAuthority::Administrator,
        expires_at: InstantMillis(100),
    }
}

#[test]
fn matching_direct_administrator_offer_is_approved_exactly_once() {
    let mut owner = owner(RemoteControlPairingAvailabilityKind::DirectPhysicalAvailable);
    let input = offer(&owner, 99);
    let expected = BeginEnrollment {
        device: owner.device,
        attempt: input.attempt,
        target: RemoteControlTargetIdentity::new(*input.target.public_keys()),
    };
    assert_eq!(
        owner.step(input).unwrap(),
        ReviewUsbEnrollmentOfferOutcome::Approve {
            enrollment: expected
        }
    );
    assert_eq!(
        owner.step(offer(&owner, 99)).unwrap(),
        ReviewUsbEnrollmentOfferOutcome::AlreadyApproved
    );
    assert_eq!(
        owner.step(offer(&owner, 98)),
        Err(ReviewUsbEnrollmentOfferError::ClockWentBackwards {
            current: InstantMillis(99),
            observed: InstantMillis(98)
        })
    );
}

#[test]
fn invitation_foreign_controller_endpoint_authority_and_expired_windows_are_refused() {
    let mut invitation = owner(RemoteControlPairingAvailabilityKind::PairingAvailable);
    assert_eq!(
        invitation.step(offer(&invitation, 10)).unwrap(),
        ReviewUsbEnrollmentOfferOutcome::InvitationRequired
    );
    let mut owner = owner(RemoteControlPairingAvailabilityKind::DirectPhysicalAvailable);
    let mut input = offer(&owner, 10);
    input.endpoint = prns_core::remote_control::RemoteControlPairingIdentity::new(
        prns_core::identity::IdentityHash::new([8; 16]),
    )
    .endpoint();
    assert_eq!(
        owner.step(input).unwrap(),
        ReviewUsbEnrollmentOfferOutcome::WrongEndpoint
    );
    let mut input = offer(&owner, 10);
    input.controller = controller(5);
    assert_eq!(
        owner.step(input).unwrap(),
        ReviewUsbEnrollmentOfferOutcome::WrongController
    );
    let mut input = offer(&owner, 10);
    input.authority = RemoteControlControllerAuthority::Operator;
    assert_eq!(
        owner.step(input).unwrap(),
        ReviewUsbEnrollmentOfferOutcome::InsufficientAuthority
    );
    assert_eq!(
        owner.step(offer(&owner, 100)).unwrap(),
        ReviewUsbEnrollmentOfferOutcome::Expired
    );
    let mut input = offer(&owner, 110);
    input.expires_at = InstantMillis(200);
    assert_eq!(
        owner.step(input).unwrap(),
        ReviewUsbEnrollmentOfferOutcome::Expired
    );
    assert!(!owner.approved);
}

proptest::proptest! {
    #[test]
    fn approval_boundaries_match_both_independent_deadlines(now in 10u64..160, offer_deadline in 10u64..160) {
        let mut owner = owner(RemoteControlPairingAvailabilityKind::DirectPhysicalAvailable);
        let mut input = offer(&owner, now); input.expires_at = InstantMillis(offer_deadline);
        let outcome = owner.step(input).unwrap();
        proptest::prop_assert_eq!(matches!(outcome, ReviewUsbEnrollmentOfferOutcome::Approve { .. }), now < offer_deadline && now < 110);
    }
}
