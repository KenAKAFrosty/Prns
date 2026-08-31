use crate::engine::{EngineReaction, EngineState, Journaled};
use crate::remote_control::{
    FailRemoteControlControllerPairingRequestOutcome,
    ReceiveRemoteControlControllerPairingCompletedOutcome,
    ReceiveRemoteControlControllerPairingOfferOutcome, RemoteControlControllerPairingView,
    RemoteControlPairingAttemptId, RemoteControlPairingMessageParseError,
    RemoteControlPairingResponse,
};
use crate::routing::links::request::{parse_packed_binary, PackedBinaryParseError};
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;
use crate::units::InstantMillis;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlControllerPairingResponseArrival<'a> {
    link_id: LinkId,
    packed_response: &'a [u8],
}

impl<'a> RemoteControlControllerPairingResponseArrival<'a> {
    #[must_use]
    pub const fn new(link_id: LinkId, packed_response: &'a [u8]) -> Self {
        Self {
            link_id,
            packed_response,
        }
    }

    #[must_use]
    pub const fn link_id(self) -> LinkId {
        self.link_id
    }

    #[must_use]
    pub const fn packed_response(self) -> &'a [u8] {
        self.packed_response
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlControllerPairingResponseBridgeInvariantViolation {
    ConfirmationStateUnavailable {
        attempt_id: RemoteControlPairingAttemptId,
    },
    PersistenceStateUnavailable {
        attempt_id: RemoteControlPairingAttemptId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmitRemoteControlControllerPairingResponseOutcome {
    NoActivePairing,
    UnrelatedLink { expected: LinkId, received: LinkId },
    MalformedEnvelope(PackedBinaryParseError),
    MalformedResponse(RemoteControlPairingMessageParseError),
    Offer(ReceiveRemoteControlControllerPairingOfferOutcome),
    Completed(ReceiveRemoteControlControllerPairingCompletedOutcome),
    InvariantViolation(RemoteControlControllerPairingResponseBridgeInvariantViolation),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlControllerPairingResponseEffect {
    Advanced,
    NotAdvanced(FailRemoteControlControllerPairingRequestOutcome),
}

impl<S: StorageLayout> EngineState<S> {
    pub(crate) fn remote_control_controller_pairing_response_effect(
        &mut self,
        link_id: LinkId,
        admission: AdmitRemoteControlControllerPairingResponseOutcome,
    ) -> RemoteControlControllerPairingResponseEffect {
        match admission {
            AdmitRemoteControlControllerPairingResponseOutcome::Offer(
                ReceiveRemoteControlControllerPairingOfferOutcome::ConfirmationRequired { .. },
            )
            | AdmitRemoteControlControllerPairingResponseOutcome::Completed(
                ReceiveRemoteControlControllerPairingCompletedOutcome::PersistenceOwed { .. },
            ) => RemoteControlControllerPairingResponseEffect::Advanced,
            AdmitRemoteControlControllerPairingResponseOutcome::NoActivePairing
            | AdmitRemoteControlControllerPairingResponseOutcome::UnrelatedLink { .. }
            | AdmitRemoteControlControllerPairingResponseOutcome::MalformedEnvelope(_)
            | AdmitRemoteControlControllerPairingResponseOutcome::MalformedResponse(_)
            | AdmitRemoteControlControllerPairingResponseOutcome::Offer(
                ReceiveRemoteControlControllerPairingOfferOutcome::Expired { .. }
                | ReceiveRemoteControlControllerPairingOfferOutcome::Rejected { .. }
                | ReceiveRemoteControlControllerPairingOfferOutcome::NoActiveAttempt
                | ReceiveRemoteControlControllerPairingOfferOutcome::Unexpected { .. }
                | ReceiveRemoteControlControllerPairingOfferOutcome::PairingUnavailable { .. },
            )
            | AdmitRemoteControlControllerPairingResponseOutcome::Completed(
                ReceiveRemoteControlControllerPairingCompletedOutcome::Expired { .. }
                | ReceiveRemoteControlControllerPairingCompletedOutcome::Rejected { .. }
                | ReceiveRemoteControlControllerPairingCompletedOutcome::NoActiveAttempt
                | ReceiveRemoteControlControllerPairingCompletedOutcome::OfferNotReceived
                | ReceiveRemoteControlControllerPairingCompletedOutcome::ApprovalRequired { .. }
                | ReceiveRemoteControlControllerPairingCompletedOutcome::AlreadyReceived { .. },
            )
            | AdmitRemoteControlControllerPairingResponseOutcome::InvariantViolation(_) => {
                RemoteControlControllerPairingResponseEffect::NotAdvanced(
                    self.remote_control_controller_pairing
                        .request_failed(link_id),
                )
            }
        }
    }

    pub fn admit_remote_control_controller_pairing_response<Work>(
        &mut self,
        arrival: RemoteControlControllerPairingResponseArrival<'_>,
        received_at: InstantMillis,
        sink: &mut impl FnMut(EngineReaction<'_, Work>),
    ) -> AdmitRemoteControlControllerPairingResponseOutcome {
        let expected_link = match self.remote_control_controller_pairing.view() {
            RemoteControlControllerPairingView::Idle => {
                return AdmitRemoteControlControllerPairingResponseOutcome::NoActivePairing
            }
            RemoteControlControllerPairingView::AwaitingOffer(pairing) => {
                pairing.context().link_id()
            }
            RemoteControlControllerPairingView::AwaitingApproval(pairing)
            | RemoteControlControllerPairingView::AwaitingCompletion(pairing) => {
                pairing.context().link_id()
            }
            RemoteControlControllerPairingView::Persisting(pairing) => pairing.context().link_id(),
        };
        if arrival.link_id != expected_link {
            return AdmitRemoteControlControllerPairingResponseOutcome::UnrelatedLink {
                expected: expected_link,
                received: arrival.link_id,
            };
        }
        let response = match parse_packed_binary(arrival.packed_response) {
            Ok(response) => response,
            Err(error) => {
                return AdmitRemoteControlControllerPairingResponseOutcome::MalformedEnvelope(error)
            }
        };
        let response = match RemoteControlPairingResponse::parse(response) {
            Ok(response) => response,
            Err(error) => {
                return AdmitRemoteControlControllerPairingResponseOutcome::MalformedResponse(error)
            }
        };
        match response {
            RemoteControlPairingResponse::Offer(offer) => {
                let transition = self
                    .remote_control_controller_pairing
                    .receive_offer(offer, received_at);
                match transition {
                    ReceiveRemoteControlControllerPairingOfferOutcome::ConfirmationRequired {
                        attempt_id,
                    } => {
                        let RemoteControlControllerPairingView::AwaitingApproval(pairing) =
                            self.remote_control_controller_pairing.view()
                        else {
                            return AdmitRemoteControlControllerPairingResponseOutcome::InvariantViolation(
                                RemoteControlControllerPairingResponseBridgeInvariantViolation::ConfirmationStateUnavailable {
                                    attempt_id,
                                },
                            );
                        };
                        if pairing.attempt_id() != attempt_id {
                            return AdmitRemoteControlControllerPairingResponseOutcome::InvariantViolation(
                                RemoteControlControllerPairingResponseBridgeInvariantViolation::ConfirmationStateUnavailable {
                                    attempt_id,
                                },
                            );
                        }
                        sink(EngineReaction::Journaled(
                            Journaled::RemoteControlControllerPairingConfirmationRequired(pairing),
                        ));
                    }
                    ReceiveRemoteControlControllerPairingOfferOutcome::Expired { expired } => {
                        sink(EngineReaction::Journaled(
                            Journaled::RemoteControlControllerPairingExpired { aborted: expired },
                        ));
                    }
                    ReceiveRemoteControlControllerPairingOfferOutcome::Rejected { .. }
                    | ReceiveRemoteControlControllerPairingOfferOutcome::NoActiveAttempt
                    | ReceiveRemoteControlControllerPairingOfferOutcome::Unexpected { .. }
                    | ReceiveRemoteControlControllerPairingOfferOutcome::PairingUnavailable {
                        ..
                    } => {}
                }
                AdmitRemoteControlControllerPairingResponseOutcome::Offer(transition)
            }
            RemoteControlPairingResponse::Completed(completed) => {
                let transition = self
                    .remote_control_controller_pairing
                    .receive_completed(completed, received_at);
                match transition {
                    ReceiveRemoteControlControllerPairingCompletedOutcome::PersistenceOwed {
                        attempt_id,
                    } => {
                        let RemoteControlControllerPairingView::Persisting(pairing) =
                            self.remote_control_controller_pairing.view()
                        else {
                            return AdmitRemoteControlControllerPairingResponseOutcome::InvariantViolation(
                                RemoteControlControllerPairingResponseBridgeInvariantViolation::PersistenceStateUnavailable {
                                    attempt_id,
                                },
                            );
                        };
                        if pairing.attempt_id() != attempt_id {
                            return AdmitRemoteControlControllerPairingResponseOutcome::InvariantViolation(
                                RemoteControlControllerPairingResponseBridgeInvariantViolation::PersistenceStateUnavailable {
                                    attempt_id,
                                },
                            );
                        }
                        sink(EngineReaction::Journaled(
                            Journaled::RemoteControlControllerPairingPersistenceRequired(pairing),
                        ));
                    }
                    ReceiveRemoteControlControllerPairingCompletedOutcome::Expired { expired } => {
                        sink(EngineReaction::Journaled(
                            Journaled::RemoteControlControllerPairingExpired { aborted: expired },
                        ));
                    }
                    ReceiveRemoteControlControllerPairingCompletedOutcome::Rejected { .. }
                    | ReceiveRemoteControlControllerPairingCompletedOutcome::NoActiveAttempt
                    | ReceiveRemoteControlControllerPairingCompletedOutcome::OfferNotReceived
                    | ReceiveRemoteControlControllerPairingCompletedOutcome::ApprovalRequired {
                        ..
                    }
                    | ReceiveRemoteControlControllerPairingCompletedOutcome::AlreadyReceived {
                        ..
                    } => {}
                }
                AdmitRemoteControlControllerPairingResponseOutcome::Completed(transition)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic, clippy::unwrap_used)]

    use super::*;
    use crate::engine::test_support::TestStorageLayout;
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::identity::vault::IdentitySecretKey;
    use crate::identity::{IdentityHash, IdentityPublicKeys, IdentitySigner};
    use crate::remote_control::{
        ApproveRemoteControlControllerPairingOutcome, BeginRemoteControlControllerPairingOutcome,
        CloseRemoteControlControllerPairingLinkOutcome, RemoteControlControllerIdentity,
        RemoteControlPairingAttemptTimeout, RemoteControlPairingBegin,
        RemoteControlPairingCompleted, RemoteControlPairingContext, RemoteControlPairingIdentity,
        RemoteControlPairingInvitationCode, RemoteControlPairingPermissions,
        RemoteControlPairingPreparedOffer, RemoteControlRequestKind, RemoteControlRequestSet,
    };
    use crate::routing::links::request::{
        write_packed_binary_header, MAX_PACKED_BINARY_HEADER_LEN,
    };
    use crate::units::DurationMillis;
    use crate::wire::TRUNCATED_HASH_BYTE_LEN;

    const STARTED_AT: InstantMillis = InstantMillis(1_000);
    const OFFERED_AT: InstantMillis = InstantMillis(2_000);
    const COMPLETED_AT: InstantMillis = InstantMillis(2_500);
    const PAIRING_EXPIRES_AT: InstantMillis = InstantMillis(10_000);
    const LINK_ID: LinkId = LinkId::new([0x81; TRUNCATED_HASH_BYTE_LEN]);
    const UNRELATED_LINK_ID: LinkId = LinkId::new([0x82; TRUNCATED_HASH_BYTE_LEN]);

    fn signer(fill: u8) -> InMemoryNodeIdentity {
        InMemoryNodeIdentity::from_secret_key_bytes(&IdentitySecretKey::new(
            [fill; crate::identity::IDENTITY_SECRET_KEY_LEN],
        ))
    }

    fn controller() -> RemoteControlControllerIdentity {
        let signer = signer(0x31);
        RemoteControlControllerIdentity::new(IdentityPublicKeys {
            encryption: signer.encryption_public_key(),
            signing: signer.signing_public_key(),
        })
    }

    fn context() -> RemoteControlPairingContext {
        RemoteControlPairingContext::new(
            RemoteControlPairingIdentity::new(IdentityHash::new([0x73; TRUNCATED_HASH_BYTE_LEN]))
                .endpoint(),
            LINK_ID,
        )
    }

    fn permissions() -> RemoteControlPairingPermissions {
        RemoteControlPairingPermissions::try_from(RemoteControlRequestSet::only(
            RemoteControlRequestKind::Describe,
        ))
        .unwrap()
    }

    fn begin(engine: &mut EngineState<TestStorageLayout>) -> RemoteControlPairingBegin {
        let BeginRemoteControlControllerPairingOutcome::BeginOwed { begin, .. } =
            engine.remote_control_controller_pairing.begin(
                controller(),
                context(),
                RemoteControlPairingInvitationCode::from_value(0x1234_ABCD),
                STARTED_AT,
                PAIRING_EXPIRES_AT,
            )
        else {
            panic!("fresh controller pairing")
        };
        begin
    }

    fn prepared_offer(
        begin: &RemoteControlPairingBegin,
    ) -> (
        RemoteControlPairingResponse,
        crate::remote_control::RemoteControlPairingTranscript,
        InMemoryNodeIdentity,
    ) {
        let target_signer = signer(0x52);
        let prepared = RemoteControlPairingPreparedOffer::new(
            &target_signer,
            context(),
            begin,
            permissions(),
            RemoteControlPairingAttemptTimeout::try_from(DurationMillis(3_000)).unwrap(),
        );
        let (offer, transcript) = prepared.into_parts();
        (
            RemoteControlPairingResponse::Offer(offer),
            transcript,
            target_signer,
        )
    }

    fn packed_response(response: RemoteControlPairingResponse) -> std::vec::Vec<u8> {
        let mut encoded = [0u8; RemoteControlPairingResponse::MAX_ENCODED_LEN];
        let encoded_len = response.write_into(&mut encoded).unwrap();
        packed_message(&encoded[..encoded_len])
    }

    fn packed_message(encoded: &[u8]) -> std::vec::Vec<u8> {
        let mut header = [0u8; MAX_PACKED_BINARY_HEADER_LEN];
        let header_len = write_packed_binary_header(encoded.len(), &mut header).unwrap();
        let mut packed = std::vec::Vec::with_capacity(header_len + encoded.len());
        packed.extend_from_slice(&header[..header_len]);
        packed.extend_from_slice(encoded);
        packed
    }

    #[test]
    fn an_authenticated_offer_journals_the_exact_confirmation() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let begin = begin(&mut engine);
        let (offer, transcript, _target_signer) = prepared_offer(&begin);
        let attempt_id = (&transcript).into();
        let confirmation_code = transcript.confirmation_code();
        let target = transcript.target().identity_hash();
        let packed = packed_response(offer);
        let mut journaled = None;

        let outcome = engine.admit_remote_control_controller_pairing_response(
            RemoteControlControllerPairingResponseArrival::new(LINK_ID, &packed),
            OFFERED_AT,
            &mut |reaction: EngineReaction<'_, crate::engine::NoOwedWork>| match reaction {
                EngineReaction::Journaled(
                    Journaled::RemoteControlControllerPairingConfirmationRequired(pairing),
                ) => {
                    journaled = Some((
                        pairing.attempt_id(),
                        pairing.context(),
                        pairing.confirmation_code(),
                        pairing.target().identity_hash(),
                    ));
                }
                EngineReaction::Journaled(_) | EngineReaction::Directive(_) => {}
            },
        );

        assert_eq!(
            outcome,
            AdmitRemoteControlControllerPairingResponseOutcome::Offer(
                ReceiveRemoteControlControllerPairingOfferOutcome::ConfirmationRequired {
                    attempt_id,
                },
            ),
        );
        assert_eq!(
            journaled,
            Some((attempt_id, context(), confirmation_code, target)),
        );
        assert!(matches!(
            engine.remote_control_controller_pairing.view(),
            RemoteControlControllerPairingView::AwaitingApproval(pairing)
                if pairing.attempt_id() == attempt_id
        ));
    }

    #[test]
    fn unrelated_and_malformed_responses_preserve_the_pending_offer() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let begin = begin(&mut engine);
        let (offer, _transcript, _target_signer) = prepared_offer(&begin);
        let packed = packed_response(offer);

        let unrelated = engine.admit_remote_control_controller_pairing_response(
            RemoteControlControllerPairingResponseArrival::new(UNRELATED_LINK_ID, &packed),
            OFFERED_AT,
            &mut |_: EngineReaction<'_, crate::engine::NoOwedWork>| {
                panic!("unrelated response must be silent")
            },
        );
        assert_eq!(
            unrelated,
            AdmitRemoteControlControllerPairingResponseOutcome::UnrelatedLink {
                expected: LINK_ID,
                received: UNRELATED_LINK_ID,
            },
        );
        assert_eq!(
            engine.remote_control_controller_pairing_response_effect(UNRELATED_LINK_ID, unrelated,),
            RemoteControlControllerPairingResponseEffect::NotAdvanced(
                FailRemoteControlControllerPairingRequestOutcome::UnrelatedLink,
            ),
        );
        assert_eq!(
            engine.admit_remote_control_controller_pairing_response(
                RemoteControlControllerPairingResponseArrival::new(LINK_ID, &[0x00]),
                OFFERED_AT,
                &mut |_: EngineReaction<'_, crate::engine::NoOwedWork>| {
                    panic!("malformed response must be silent")
                },
            ),
            AdmitRemoteControlControllerPairingResponseOutcome::MalformedEnvelope(
                PackedBinaryParseError::NotBinary,
            ),
        );
        let malformed_response = packed_message(&[]);
        assert_eq!(
            engine.admit_remote_control_controller_pairing_response(
                RemoteControlControllerPairingResponseArrival::new(LINK_ID, &malformed_response,),
                OFFERED_AT,
                &mut |_: EngineReaction<'_, crate::engine::NoOwedWork>| {
                    panic!("malformed response must be silent")
                },
            ),
            AdmitRemoteControlControllerPairingResponseOutcome::MalformedResponse(
                RemoteControlPairingMessageParseError::Truncated,
            ),
        );
        assert!(matches!(
            engine.remote_control_controller_pairing.view(),
            RemoteControlControllerPairingView::AwaitingOffer(pairing)
                if pairing.context() == context()
        ));
    }

    #[test]
    fn authenticated_completion_journals_exact_persistence_without_releasing_link_correlation() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let begin = begin(&mut engine);
        let (offer, transcript, target_signer) = prepared_offer(&begin);
        let offer = packed_response(offer);
        let _offer = engine.admit_remote_control_controller_pairing_response(
            RemoteControlControllerPairingResponseArrival::new(LINK_ID, &offer),
            OFFERED_AT,
            &mut |_: EngineReaction<'_, crate::engine::NoOwedWork>| {},
        );
        let attempt_id = (&transcript).into();
        assert!(matches!(
            engine
                .remote_control_controller_pairing
                .approve(attempt_id, OFFERED_AT),
            ApproveRemoteControlControllerPairingOutcome::CommitOwed { .. }
        ));
        let completed = RemoteControlPairingCompleted::signed_by(&target_signer, &transcript)
            .map(RemoteControlPairingResponse::Completed)
            .unwrap();
        let completed = packed_response(completed);
        let mut journaled = None;

        assert_eq!(
            engine.admit_remote_control_controller_pairing_response(
                RemoteControlControllerPairingResponseArrival::new(UNRELATED_LINK_ID, &completed,),
                COMPLETED_AT,
                &mut |_: EngineReaction<'_, crate::engine::NoOwedWork>| {
                    panic!("unrelated completion must be silent")
                },
            ),
            AdmitRemoteControlControllerPairingResponseOutcome::UnrelatedLink {
                expected: LINK_ID,
                received: UNRELATED_LINK_ID,
            },
        );
        let outcome = engine.admit_remote_control_controller_pairing_response(
            RemoteControlControllerPairingResponseArrival::new(LINK_ID, &completed),
            COMPLETED_AT,
            &mut |reaction: EngineReaction<'_, crate::engine::NoOwedWork>| match reaction {
                EngineReaction::Journaled(
                    Journaled::RemoteControlControllerPairingPersistenceRequired(pairing),
                ) => {
                    journaled = Some((
                        pairing.attempt_id(),
                        pairing.context(),
                        pairing.access().target().identity_hash(),
                        *pairing.access().permitted_requests(),
                    ));
                }
                EngineReaction::Journaled(_) | EngineReaction::Directive(_) => {}
            },
        );

        assert_eq!(
            outcome,
            AdmitRemoteControlControllerPairingResponseOutcome::Completed(
                ReceiveRemoteControlControllerPairingCompletedOutcome::PersistenceOwed {
                    attempt_id,
                },
            ),
        );
        assert_eq!(
            journaled,
            Some((
                attempt_id,
                context(),
                transcript.target().identity_hash(),
                *transcript.permissions().permitted_requests(),
            )),
        );
        assert_eq!(
            engine
                .remote_control_controller_pairing
                .close_link(UNRELATED_LINK_ID),
            CloseRemoteControlControllerPairingLinkOutcome::UnrelatedLink,
        );
        assert_eq!(
            engine.remote_control_controller_pairing.close_link(LINK_ID),
            CloseRemoteControlControllerPairingLinkOutcome::PersistenceInProgress { attempt_id },
        );
    }

    #[test]
    fn an_offer_at_the_pairing_deadline_journals_the_exact_expiry() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let begin = begin(&mut engine);
        let (offer, _transcript, _target_signer) = prepared_offer(&begin);
        let offer = packed_response(offer);
        let expired = crate::remote_control::RemoteControlControllerPairingAborted::AwaitingOffer {
            context: context(),
        };
        let mut journaled = None;

        let outcome = engine.admit_remote_control_controller_pairing_response(
            RemoteControlControllerPairingResponseArrival::new(LINK_ID, &offer),
            PAIRING_EXPIRES_AT,
            &mut |reaction: EngineReaction<'_, crate::engine::NoOwedWork>| match reaction {
                EngineReaction::Journaled(Journaled::RemoteControlControllerPairingExpired {
                    aborted,
                }) => journaled = Some(aborted),
                EngineReaction::Journaled(_) | EngineReaction::Directive(_) => {}
            },
        );

        assert_eq!(
            outcome,
            AdmitRemoteControlControllerPairingResponseOutcome::Offer(
                ReceiveRemoteControlControllerPairingOfferOutcome::Expired { expired },
            ),
        );
        assert_eq!(journaled, Some(expired));
        assert_eq!(
            engine.remote_control_controller_pairing.view(),
            RemoteControlControllerPairingView::Idle,
        );
    }
}
