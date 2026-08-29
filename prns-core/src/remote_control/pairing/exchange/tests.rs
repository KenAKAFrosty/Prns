#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use super::*;
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::vault::IdentitySecretKey;
use crate::identity::{IdentityHash, IdentityPublicKeys, IdentitySigner};
use crate::remote_control::{
    RemoteControlControllerIdentity, RemoteControlPairingIdentity, RemoteControlPairingPermissions,
    RemoteControlPairingPermissionsError, RemoteControlRequestSet,
};
use crate::routing::links::LinkId;
use crate::wire::TRUNCATED_HASH_BYTE_LEN;
use proptest::prelude::*;

struct PairingFixture {
    target_signer: InMemoryNodeIdentity,
    context: RemoteControlPairingContext,
    begin: RemoteControlPairingBegin,
}

impl PairingFixture {
    fn new() -> Self {
        Self {
            target_signer: signer(0x52),
            context: context(0x73, 0x84),
            begin: RemoteControlPairingBegin::new(controller(0x31)),
        }
    }

    fn prepared_offer(&self) -> RemoteControlPairingPreparedOffer {
        RemoteControlPairingPreparedOffer::new(
            &self.target_signer,
            self.context,
            &self.begin,
            permissions(RemoteControlRequestSet::only(
                RemoteControlRequestKind::Describe,
            )),
            attempt_timeout(30_000),
        )
    }
}

fn signer(fill: u8) -> InMemoryNodeIdentity {
    InMemoryNodeIdentity::from_secret_key_bytes(&IdentitySecretKey::new(
        [fill; crate::identity::IDENTITY_SECRET_KEY_LEN],
    ))
}

fn controller(fill: u8) -> RemoteControlControllerIdentity {
    let signer = signer(fill);
    RemoteControlControllerIdentity::new(IdentityPublicKeys {
        encryption: signer.encryption_public_key(),
        signing: signer.signing_public_key(),
    })
}

fn context(endpoint_fill: u8, link_fill: u8) -> RemoteControlPairingContext {
    RemoteControlPairingContext::new(
        RemoteControlPairingIdentity::new(IdentityHash::new(
            [endpoint_fill; TRUNCATED_HASH_BYTE_LEN],
        ))
        .endpoint(),
        LinkId::new([link_fill; TRUNCATED_HASH_BYTE_LEN]),
    )
}

fn permissions(requests: RemoteControlRequestSet) -> RemoteControlPairingPermissions {
    RemoteControlPairingPermissions::try_from(requests).unwrap()
}

fn attempt_timeout(millis: u64) -> RemoteControlPairingAttemptTimeout {
    RemoteControlPairingAttemptTimeout::try_from(DurationMillis(millis)).unwrap()
}

fn encoded_request(request: &RemoteControlPairingRequest) -> std::vec::Vec<u8> {
    let mut encoded = std::vec![0u8; request.encoded_len()];
    assert_eq!(request.write_into(&mut encoded), Ok(encoded.len()));
    encoded
}

fn encoded_response(response: &RemoteControlPairingResponse) -> std::vec::Vec<u8> {
    let mut encoded = std::vec![0u8; response.encoded_len()];
    assert_eq!(response.write_into(&mut encoded), Ok(encoded.len()));
    encoded
}

fn parsed_offer(bytes: &[u8]) -> RemoteControlPairingOffer {
    let RemoteControlPairingResponse::Offer(offer) =
        RemoteControlPairingResponse::parse(bytes).unwrap()
    else {
        panic!("expected an offer")
    };
    offer
}

#[test]
fn pairing_exchange_discriminants_and_bounds_are_stable() {
    assert_eq!(REMOTE_CONTROL_PAIRING_REQUEST_ENDPOINT_ID, "/pair");
    assert_eq!(
        RemoteControlPairingProtocolVersion::ALL,
        [RemoteControlPairingProtocolVersion::V1],
    );
    assert_eq!(
        RemoteControlPairingMessageKind::ALL,
        [
            RemoteControlPairingMessageKind::Begin,
            RemoteControlPairingMessageKind::Offer,
            RemoteControlPairingMessageKind::Commit,
            RemoteControlPairingMessageKind::Completed,
        ],
    );
    assert_eq!(RemoteControlPairingProtocolVersion::V1.wire_value(), 1);
    assert_eq!(RemoteControlPairingMessageKind::Begin.wire_value(), 1);
    assert_eq!(RemoteControlPairingMessageKind::Offer.wire_value(), 2);
    assert_eq!(RemoteControlPairingMessageKind::Commit.wire_value(), 3);
    assert_eq!(RemoteControlPairingMessageKind::Completed.wire_value(), 4);
    assert_eq!(RemoteControlPairingRequest::MAX_ENCODED_LEN, 66);
    assert_eq!(RemoteControlPairingResponse::MAX_ENCODED_LEN, 137);
    const {
        assert!(
            RemoteControlPairingRequest::MAX_ENCODED_LEN
                <= crate::engine::MAX_SEND_REQUEST_DATA_LEN
        );
        assert!(
            RemoteControlPairingResponse::MAX_ENCODED_LEN <= crate::engine::MAX_RESPOND_DATA_LEN
        );
    }
}

#[test]
fn attempt_timeouts_are_positive_bounded_wire_values() {
    assert_eq!(
        RemoteControlPairingAttemptTimeout::try_from(DurationMillis(0)),
        Err(RemoteControlPairingAttemptTimeoutError::Zero),
    );
    let maximum =
        RemoteControlPairingAttemptTimeout::try_from(MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT)
            .unwrap();
    assert_eq!(
        maximum.duration(),
        MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT
    );
    let too_long = DurationMillis(
        MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT
            .0
            .saturating_add(1),
    );
    assert_eq!(
        RemoteControlPairingAttemptTimeout::try_from(too_long),
        Err(RemoteControlPairingAttemptTimeoutError::TooLong {
            actual: too_long,
            maximum: MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT,
        }),
    );
}

#[test]
fn begin_and_commit_requests_round_trip_at_their_exact_lengths() {
    let fixture = PairingFixture::new();
    let begin = RemoteControlPairingRequest::Begin(fixture.begin);
    let mut begin_out = [0xA5; RemoteControlPairingRequest::MAX_ENCODED_LEN + 1];
    assert_eq!(begin.write_into(&mut begin_out), Ok(begin.encoded_len()));
    assert_eq!(begin_out[begin.encoded_len()], 0xA5);
    assert_eq!(
        RemoteControlPairingRequest::parse(&begin_out[..begin.encoded_len()]),
        Ok(begin),
    );

    let fixture = PairingFixture::new();
    let prepared = fixture.prepared_offer();
    let commit =
        RemoteControlPairingRequest::Commit(RemoteControlPairingCommit::new(prepared.transcript()));
    let encoded = encoded_request(&commit);
    assert_eq!(encoded.len(), PAIRING_COMMIT_ENCODED_LEN);
    assert_eq!(RemoteControlPairingRequest::parse(&encoded), Ok(commit));
}

#[test]
fn a_signed_offer_round_trips_to_the_same_transcript_and_code() {
    let fixture = PairingFixture::new();
    let prepared = fixture.prepared_offer();
    let expected_code = prepared.transcript().confirmation_code();
    let (offer, expected_transcript) = prepared.into_parts();
    let response = RemoteControlPairingResponse::Offer(offer);
    let encoded = encoded_response(&response);
    let parsed = parsed_offer(&encoded);
    let verified = parsed.verify(fixture.context, &fixture.begin).unwrap();

    assert_eq!(verified, expected_transcript);
    assert_eq!(verified.confirmation_code(), expected_code);
    assert!(expected_code.value() < PAIRING_CONFIRMATION_CODE_MODULUS);
    let displayed = std::format!("{expected_code}");
    assert_eq!(
        displayed.len(),
        RemoteControlPairingConfirmationCode::DIGIT_COUNT
    );
    assert!(displayed.bytes().all(|byte| byte.is_ascii_digit()));
}

#[test]
fn an_offer_with_every_permission_fills_the_reported_response_bound() {
    let fixture = PairingFixture::new();
    let prepared = RemoteControlPairingPreparedOffer::new(
        &fixture.target_signer,
        fixture.context,
        &fixture.begin,
        permissions(RemoteControlRequestSet::all()),
        attempt_timeout(30_000),
    );
    let (offer, expected_transcript) = prepared.into_parts();
    let response = RemoteControlPairingResponse::Offer(offer);
    assert_eq!(
        response.encoded_len(),
        RemoteControlPairingResponse::MAX_ENCODED_LEN
    );
    let encoded = encoded_response(&response);
    let parsed = parsed_offer(&encoded);
    assert_eq!(
        parsed.verify(fixture.context, &fixture.begin),
        Ok(expected_transcript),
    );
}

#[test]
fn the_transcript_and_confirmation_code_have_a_pinned_vector() {
    let fixture = PairingFixture::new();
    let prepared = fixture.prepared_offer();

    assert_eq!(
        prepared.transcript().digest().as_bytes(),
        &[
            0xa8, 0x56, 0x69, 0x47, 0xc2, 0xce, 0x4d, 0x86, 0x40, 0xa4, 0x29, 0x85, 0x56, 0x2b,
            0x0d, 0xbc, 0x47, 0x01, 0x40, 0xf0, 0xc2, 0xb4, 0xa5, 0xdc, 0x24, 0x41, 0xb0, 0x3e,
            0xfc, 0xe0, 0x70, 0x31,
        ],
    );
    assert_eq!(prepared.transcript().confirmation_code().value(), 85_800);
}

#[test]
fn every_offer_transcript_fact_is_covered_by_the_target_signature() {
    let fixture = PairingFixture::new();
    let prepared = fixture.prepared_offer();
    let encoded = encoded_response(&RemoteControlPairingResponse::Offer(
        prepared.into_parts().0,
    ));

    let parsed = parsed_offer(&encoded);
    assert_eq!(
        parsed.verify(context(0x74, 0x84), &fixture.begin),
        Err(RemoteControlPairingOfferVerificationError::InvalidTargetSignature),
    );
    let parsed = parsed_offer(&encoded);
    assert_eq!(
        parsed.verify(context(0x73, 0x85), &fixture.begin),
        Err(RemoteControlPairingOfferVerificationError::InvalidTargetSignature),
    );
    let parsed = parsed_offer(&encoded);
    assert_eq!(
        parsed.verify(
            fixture.context,
            &RemoteControlPairingBegin::new(controller(0x32)),
        ),
        Err(RemoteControlPairingOfferVerificationError::InvalidTargetSignature),
    );

    for (offset, mask) in [
        (PAIRING_MESSAGE_HEADER_ENCODED_LEN, 0x01),
        (
            PAIRING_MESSAGE_HEADER_ENCODED_LEN + PAIRING_IDENTITY_ENCODED_LEN + 1,
            RemoteControlRequestKind::Describe.wire_value()
                ^ RemoteControlRequestKind::AnnounceSelf.wire_value(),
        ),
        (
            PAIRING_MESSAGE_HEADER_ENCODED_LEN
                + PAIRING_IDENTITY_ENCODED_LEN
                + PAIRING_REQUEST_SET_COUNT_ENCODED_LEN
                + 1,
            0x01,
        ),
        (encoded.len() - 1, 0x01),
    ] {
        let mut tampered = encoded.clone();
        tampered[offset] ^= mask;
        let offer = parsed_offer(&tampered);
        assert_eq!(
            offer.verify(fixture.context, &fixture.begin),
            Err(RemoteControlPairingOfferVerificationError::InvalidTargetSignature),
        );
    }
}

#[test]
fn commit_and_completed_bind_the_exact_durably_committed_transcript() {
    let fixture = PairingFixture::new();
    let prepared = fixture.prepared_offer();
    let transcript = prepared.transcript();
    let commit = RemoteControlPairingCommit::new(transcript);
    assert!(commit.matches(transcript));

    let alternate = RemoteControlPairingPreparedOffer::new(
        &fixture.target_signer,
        context(0x73, 0x85),
        &fixture.begin,
        permissions(RemoteControlRequestSet::only(
            RemoteControlRequestKind::Describe,
        )),
        attempt_timeout(30_000),
    );
    assert!(!commit.matches(alternate.transcript()));

    let completed =
        RemoteControlPairingCompleted::signed_by(&fixture.target_signer, transcript).unwrap();
    assert_eq!(completed.verify(transcript), Ok(()));
    assert!(matches!(
        RemoteControlPairingCompleted::signed_by(&signer(0x53), transcript),
        Err(RemoteControlPairingCompletionSigningError::TargetIdentityMismatch { .. }),
    ));

    let response = RemoteControlPairingResponse::Completed(completed);
    let encoded = encoded_response(&response);
    let RemoteControlPairingResponse::Completed(parsed) =
        RemoteControlPairingResponse::parse(&encoded).unwrap()
    else {
        panic!("expected completed")
    };
    assert_eq!(parsed.verify(transcript), Ok(()));

    let mut wrong_transcript = encoded.clone();
    wrong_transcript[PAIRING_MESSAGE_HEADER_ENCODED_LEN] ^= 0x01;
    let RemoteControlPairingResponse::Completed(parsed) =
        RemoteControlPairingResponse::parse(&wrong_transcript).unwrap()
    else {
        panic!("expected completed")
    };
    assert!(matches!(
        parsed.verify(transcript),
        Err(RemoteControlPairingCompletedVerificationError::TranscriptMismatch { .. }),
    ));

    let mut wrong_signature = encoded;
    let last = wrong_signature.len() - 1;
    wrong_signature[last] ^= 0x01;
    let RemoteControlPairingResponse::Completed(parsed) =
        RemoteControlPairingResponse::parse(&wrong_signature).unwrap()
    else {
        panic!("expected completed")
    };
    assert_eq!(
        parsed.verify(transcript),
        Err(RemoteControlPairingCompletedVerificationError::InvalidTargetSignature),
    );
}

#[test]
fn parsers_reject_wrong_directions_versions_kinds_lengths_and_noncanonical_fields() {
    let version = RemoteControlPairingProtocolVersion::V1.wire_value();
    assert_eq!(
        RemoteControlPairingRequest::parse(&[
            version,
            RemoteControlPairingMessageKind::Offer.wire_value(),
        ]),
        Err(RemoteControlPairingMessageParseError::UnexpectedKind {
            direction: RemoteControlPairingMessageDirection::Request,
            found: RemoteControlPairingMessageKind::Offer,
        }),
    );
    assert_eq!(
        RemoteControlPairingResponse::parse(&[
            version,
            RemoteControlPairingMessageKind::Begin.wire_value(),
        ]),
        Err(RemoteControlPairingMessageParseError::UnexpectedKind {
            direction: RemoteControlPairingMessageDirection::Response,
            found: RemoteControlPairingMessageKind::Begin,
        }),
    );
    assert_eq!(
        RemoteControlPairingRequest::parse(&[0x7F, 1]),
        Err(RemoteControlPairingMessageParseError::UnsupportedVersion { found: 0x7F }),
    );
    assert_eq!(
        RemoteControlPairingRequest::parse(&[version, 0x7F]),
        Err(RemoteControlPairingMessageParseError::UnknownKind { found: 0x7F }),
    );
    assert_eq!(
        RemoteControlPairingRequest::parse(&std::vec![
            0;
            RemoteControlPairingRequest::MAX_ENCODED_LEN + 1
        ]),
        Err(RemoteControlPairingMessageParseError::TooLong {
            actual: RemoteControlPairingRequest::MAX_ENCODED_LEN + 1,
            maximum: RemoteControlPairingRequest::MAX_ENCODED_LEN,
        }),
    );

    let fixture = PairingFixture::new();
    let begin = RemoteControlPairingRequest::Begin(fixture.begin);
    let mut invalid_controller = encoded_request(&begin);
    let signing_key_offset =
        PAIRING_MESSAGE_HEADER_ENCODED_LEN + crate::crypto::X25519PublicKey::LEN;
    let invalid_signing_key = invalid_controller
        .get_mut(signing_key_offset..signing_key_offset + crate::crypto::Ed25519PublicKey::LEN)
        .unwrap();
    let mut rejected_key = [0u8; crate::crypto::Ed25519PublicKey::LEN];
    rejected_key[1] = 3;
    invalid_signing_key.copy_from_slice(&rejected_key);
    assert_eq!(
        RemoteControlPairingRequest::parse(&invalid_controller),
        Err(
            RemoteControlPairingMessageParseError::InvalidSigningPublicKey {
                role: RemoteControlPairingIdentityRole::Controller,
            },
        ),
    );

    let fixture = PairingFixture::new();
    let prepared = fixture.prepared_offer();
    let mut invalid_target = encoded_response(&RemoteControlPairingResponse::Offer(
        prepared.into_parts().0,
    ));
    let target_signing_key_offset =
        PAIRING_MESSAGE_HEADER_ENCODED_LEN + crate::crypto::X25519PublicKey::LEN;
    invalid_target[target_signing_key_offset
        ..target_signing_key_offset + crate::crypto::Ed25519PublicKey::LEN]
        .copy_from_slice(&rejected_key);
    assert_eq!(
        RemoteControlPairingResponse::parse(&invalid_target),
        Err(
            RemoteControlPairingMessageParseError::InvalidSigningPublicKey {
                role: RemoteControlPairingIdentityRole::Target,
            },
        ),
    );

    let fixture = PairingFixture::new();
    let prepared = fixture.prepared_offer();
    let mut encoded = encoded_response(&RemoteControlPairingResponse::Offer(
        prepared.into_parts().0,
    ));
    let permission_count_offset = PAIRING_MESSAGE_HEADER_ENCODED_LEN + PAIRING_IDENTITY_ENCODED_LEN;
    encoded.insert(
        permission_count_offset + PAIRING_REQUEST_SET_COUNT_ENCODED_LEN + 1,
        RemoteControlRequestKind::Describe.wire_value(),
    );
    encoded[permission_count_offset] = 2;
    assert_eq!(
        RemoteControlPairingResponse::parse(&encoded),
        Err(RemoteControlPairingMessageParseError::NonCanonicalPermissions),
    );

    let fixture = PairingFixture::new();
    let prepared = fixture.prepared_offer();
    let mut unknown_permission = encoded_response(&RemoteControlPairingResponse::Offer(
        prepared.into_parts().0,
    ));
    unknown_permission[permission_count_offset + 1] = 0x7F;
    assert_eq!(
        RemoteControlPairingResponse::parse(&unknown_permission),
        Err(RemoteControlPairingMessageParseError::UnknownRequestKind { found: 0x7F }),
    );

    let fixture = PairingFixture::new();
    let prepared = fixture.prepared_offer();
    let mut empty_permissions = encoded_response(&RemoteControlPairingResponse::Offer(
        prepared.into_parts().0,
    ));
    empty_permissions.remove(permission_count_offset + 1);
    empty_permissions[permission_count_offset] = 0;
    assert_eq!(
        RemoteControlPairingResponse::parse(&empty_permissions),
        Err(RemoteControlPairingMessageParseError::InvalidPermissions(
            RemoteControlPairingPermissionsError::NoPermittedRequests,
        )),
    );

    let fixture = PairingFixture::new();
    let prepared = fixture.prepared_offer();
    let mut zero_timeout = encoded_response(&RemoteControlPairingResponse::Offer(
        prepared.into_parts().0,
    ));
    let timeout_offset = permission_count_offset + 2;
    zero_timeout[timeout_offset..timeout_offset + PAIRING_ATTEMPT_TIMEOUT_ENCODED_LEN].fill(0);
    assert_eq!(
        RemoteControlPairingResponse::parse(&zero_timeout),
        Err(
            RemoteControlPairingMessageParseError::InvalidAttemptTimeout(
                RemoteControlPairingAttemptTimeoutError::Zero,
            )
        ),
    );

    let fixture = PairingFixture::new();
    let prepared = fixture.prepared_offer();
    let mut excessive_timeout = encoded_response(&RemoteControlPairingResponse::Offer(
        prepared.into_parts().0,
    ));
    let excessive = u32::try_from(
        MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT
            .0
            .saturating_add(1),
    )
    .unwrap()
    .to_le_bytes();
    excessive_timeout[timeout_offset..timeout_offset + PAIRING_ATTEMPT_TIMEOUT_ENCODED_LEN]
        .copy_from_slice(&excessive);
    assert_eq!(
        RemoteControlPairingResponse::parse(&excessive_timeout),
        Err(
            RemoteControlPairingMessageParseError::InvalidAttemptTimeout(
                RemoteControlPairingAttemptTimeoutError::TooLong {
                    actual: DurationMillis(u64::from(u32::from_le_bytes(excessive))),
                    maximum: MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT,
                },
            ),
        ),
    );
}

#[test]
fn every_truncated_prefix_and_short_writer_buffer_is_rejected() {
    let fixture = PairingFixture::new();
    let begin = RemoteControlPairingRequest::Begin(fixture.begin);
    let begin_bytes = encoded_request(&begin);
    for length in 0..begin_bytes.len() {
        assert!(RemoteControlPairingRequest::parse(&begin_bytes[..length]).is_err());
        let mut out = std::vec![0u8; length];
        assert_eq!(
            begin.write_into(&mut out),
            Err(RemoteControlPairingMessageWriteError::BufferTooShort {
                required: begin.encoded_len(),
                actual: length,
            }),
        );
    }

    let fixture = PairingFixture::new();
    let prepared = fixture.prepared_offer();
    let offer = RemoteControlPairingResponse::Offer(prepared.into_parts().0);
    let offer_bytes = encoded_response(&offer);
    for length in 0..offer_bytes.len() {
        assert!(RemoteControlPairingResponse::parse(&offer_bytes[..length]).is_err());
        let mut out = std::vec![0u8; length];
        assert_eq!(
            offer.write_into(&mut out),
            Err(RemoteControlPairingMessageWriteError::BufferTooShort {
                required: offer.encoded_len(),
                actual: length,
            }),
        );
    }
}

#[test]
fn valid_shorter_messages_reject_trailing_bytes() {
    let fixture = PairingFixture::new();
    let prepared = fixture.prepared_offer();
    let commit =
        RemoteControlPairingRequest::Commit(RemoteControlPairingCommit::new(prepared.transcript()));
    let mut commit_bytes = encoded_request(&commit);
    commit_bytes.push(0xA5);
    assert_eq!(
        RemoteControlPairingRequest::parse(&commit_bytes),
        Err(RemoteControlPairingMessageParseError::TrailingBytes { actual: 1 }),
    );

    let offer = RemoteControlPairingResponse::Offer(prepared.into_parts().0);
    let mut offer_bytes = encoded_response(&offer);
    offer_bytes.push(0xA5);
    assert_eq!(
        RemoteControlPairingResponse::parse(&offer_bytes),
        Err(RemoteControlPairingMessageParseError::TrailingBytes { actual: 1 }),
    );
}

proptest! {
    #[test]
    fn every_successfully_parsed_pairing_request_round_trips(
        bytes in proptest::collection::vec(
            any::<u8>(),
            0..=RemoteControlPairingRequest::MAX_ENCODED_LEN + 1,
        ),
    ) {
        if let Ok(request) = RemoteControlPairingRequest::parse(&bytes) {
            let encoded = encoded_request(&request);
            prop_assert_eq!(RemoteControlPairingRequest::parse(&encoded), Ok(request));
        }
    }

    #[test]
    fn every_successfully_parsed_pairing_response_round_trips(
        bytes in proptest::collection::vec(
            any::<u8>(),
            0..=RemoteControlPairingResponse::MAX_ENCODED_LEN + 1,
        ),
    ) {
        if let Ok(response) = RemoteControlPairingResponse::parse(&bytes) {
            let encoded = encoded_response(&response);
            prop_assert_eq!(RemoteControlPairingResponse::parse(&encoded), Ok(response));
        }
    }
}
