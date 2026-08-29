use crate::crypto::ratchets::RatchetPolicy;
use crate::engine::node_egress::fan_frame;
use crate::engine::{
    CloseRemoteControlPairingFailure, CloseRemoteControlPairingOutcome, CommandId, CommandOutcome,
    Directive, EgressTarget, EngineReaction, FanTarget, OpenRemoteControlPairing,
    OpenRemoteControlPairingFailure, OpenRemoteControlPairingRejection, RemoteControlPairingOpened,
    Respond, RespondData, RespondPayload, RetireDestinationOutcome, SendPlainPacket,
    SendPlainPacketPayload,
};
use crate::identity::held::ReleaseHeldIdentityOutcome;
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::vault::IdentitySecretKey;
use crate::identity::{IdentityHash, IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::interfaces::{AttachedInterfaces, InterfaceId};
use crate::remote_control::{
    BeginRemoteControlTargetPairingOutcome,
    CloseRemoteControlPairingOutcome as PairingStateCloseOutcome,
    DispatchRemoteControlTargetPairingOfferOutcome, ExpireRemoteControlTargetPairingOutcome,
    FailRemoteControlTargetPairingOfferDispatchOutcome,
    OpenRemoteControlPairingOutcome as PairingStateOpenOutcome, RemoteControlPairingAttemptId,
    RemoteControlPairingAvailability, RemoteControlPairingAvailabilityDestination,
    RemoteControlPairingAvailabilityDestinationError, RemoteControlPairingIdentity,
    RemoteControlPairingMessageParseError, RemoteControlPairingMessageWriteError,
    RemoteControlPairingRequest, RemoteControlPairingResponse, RemoteControlPairingSession,
    RemoteControlPairingState, RemoteControlPairingView, RemoteControlPairingWindow,
    RemoteControlTargetPairingAborted, RemoteControlTargetPairingAttemptWindowError,
    RemoteControlTargetPairingBeginArrival, RemoteControlTargetPairingBeginRejection,
    RemoteControlTargetPairingResponder, REMOTE_CONTROL_PAIRING_APPLICATION_ASPECTS,
    REMOTE_CONTROL_PAIRING_APPLICATION_NAME, REMOTE_CONTROL_PAIRING_REQUEST_ENDPOINT_ID,
};
use crate::routing::announce::{AnnounceEntropy, AnnounceId};
use crate::routing::links::request::{
    packed_binary_len, parse_packed_binary, write_packed_binary_header, LinkRequestWriteError,
    PackBinaryError, PackedBinaryParseError, RequestId, MAX_PACKED_BINARY_HEADER_LEN,
    REQUEST_WIRE_OVERHEAD,
};
use crate::routing::links::LinkId;
use crate::routing::request_handlers::{RequestPathHash, RequestPolicy};
use crate::routing::{LinkRequestPolicy, ProofStrategy};
use crate::storage::StorageLayout;
use crate::units::{ByteLimit, InstantMillis};
use crate::wire::{DestinationHash, BROADCAST_MDU, BROADCAST_MTU};

const PAIRING_IDENTITY_MINT_ATTEMPTS: usize = 4;
const PAIRING_REQUEST_PACKED_LEN: usize =
    match packed_binary_len(RemoteControlPairingRequest::MAX_ENCODED_LEN) {
        Some(length) => length,
        None => panic!(),
    };
const PAIRING_REQUEST_PLAINTEXT_MAX: usize =
    REQUEST_WIRE_OVERHEAD.saturating_add(PAIRING_REQUEST_PACKED_LEN);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigureRemoteControlPairingError {
    AlreadyConfigured,
    TargetIdentityNotHeld,
    RegisterAvailability(crate::routing::upstream_app_destinations::RegisterDestinationError),
    AvailabilityDestination(RemoteControlPairingAvailabilityDestinationError),
}

pub(crate) struct RemoteControlPairingRequestIngress<'a> {
    pub destination: DestinationHash,
    pub link_id: LinkId,
    pub request_id: RequestId,
    pub requester: Option<crate::identity::IdentityHash>,
    pub path_hash: RequestPathHash,
    pub data: &'a [u8],
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RemoteControlPairingRequestIngressOutcome {
    ForwardToApplication,
    Pairing(RemoteControlPairingRequestOutcome),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RemoteControlPairingRequestOutcome {
    Unidentified,
    MalformedEnvelope(PackedBinaryParseError),
    MalformedMessage(RemoteControlPairingMessageParseError),
    UnexpectedRequest(RemoteControlPairingRequest),
    BeginExpired {
        expired: RemoteControlTargetPairingAborted,
    },
    BeginBusy {
        active: RemoteControlPairingAttemptId,
    },
    BeginUnavailable {
        reason: RemoteControlTargetPairingAttemptWindowError,
    },
    BeginRejected {
        rejected: RemoteControlTargetPairingResponder,
        reason: RemoteControlTargetPairingBeginRejection,
    },
    OfferDispatched {
        attempt_id: RemoteControlPairingAttemptId,
    },
    OfferDispatchFailed {
        attempt_id: RemoteControlPairingAttemptId,
        failure: RemoteControlPairingResponseDispatchFailure,
    },
    InvariantViolation(RemoteControlPairingBridgeInvariantViolation),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteControlPairingResponseDispatchFailure {
    Encode(RemoteControlPairingMessageWriteError),
    Pack(PackBinaryError),
    Capacity { required: usize, maximum: usize },
    Write(LinkRequestWriteError),
    EgressUnavailable { interface: InterfaceId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteControlPairingBridgeInvariantViolation {
    TargetSignerUnavailable {
        target_identity: IdentityHash,
    },
    OfferDispatchRollbackFailed {
        attempt_id: RemoteControlPairingAttemptId,
        dispatch_failure: RemoteControlPairingResponseDispatchFailure,
        reducer: FailRemoteControlTargetPairingOfferDispatchOutcome,
    },
    OfferDispatchAttemptMismatch {
        dispatched: RemoteControlPairingAttemptId,
        active: RemoteControlPairingAttemptId,
    },
    NoOfferPreparedAfterDispatch {
        attempt_id: RemoteControlPairingAttemptId,
    },
}

impl<S: StorageLayout> crate::engine::EngineState<S> {
    pub fn configure_remote_control_pairing(
        &mut self,
        target_identity: crate::identity::IdentityHash,
    ) -> Result<RemoteControlPairingAvailabilityDestination, ConfigureRemoteControlPairingError>
    {
        match self.remote_control_pairing.view() {
            RemoteControlPairingView::Unavailable => {}
            RemoteControlPairingView::Closed | RemoteControlPairingView::Open(_) => {
                return Err(ConfigureRemoteControlPairingError::AlreadyConfigured)
            }
        }
        if !self.held_identities.contains(&target_identity) {
            return Err(ConfigureRemoteControlPairingError::TargetIdentityNotHeld);
        }
        let destination = self
            .register_plain_destination(
                REMOTE_CONTROL_PAIRING_APPLICATION_NAME,
                REMOTE_CONTROL_PAIRING_APPLICATION_ASPECTS,
            )
            .map_err(ConfigureRemoteControlPairingError::RegisterAvailability)?;
        let destination = RemoteControlPairingAvailabilityDestination::try_from(destination)
            .map_err(ConfigureRemoteControlPairingError::AvailabilityDestination)?;
        self.remote_control_pairing = RemoteControlPairingState::available(target_identity);
        Ok(destination)
    }

    #[must_use]
    pub const fn remote_control_pairing_view(&self) -> RemoteControlPairingView<'_> {
        self.remote_control_pairing.view()
    }

    pub(crate) fn ingest_remote_control_pairing_request<F>(
        &mut self,
        ingress: RemoteControlPairingRequestIngress<'_>,
        interfaces: AttachedInterfaces<'_>,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> RemoteControlPairingRequestIngressOutcome
    where
        F: FnMut(&mut [u8]),
    {
        let open_session = match self.remote_control_pairing.open_session() {
            Some(open_session)
                if open_session.session().endpoint().destination_hash() == ingress.destination =>
            {
                open_session
            }
            Some(_) | None => {
                return RemoteControlPairingRequestIngressOutcome::ForwardToApplication
            }
        };
        if ingress.path_hash != RequestPathHash::of(REMOTE_CONTROL_PAIRING_REQUEST_ENDPOINT_ID) {
            return RemoteControlPairingRequestIngressOutcome::ForwardToApplication;
        }
        let Some(identified_controller) = ingress.requester else {
            return RemoteControlPairingRequestIngressOutcome::Pairing(
                RemoteControlPairingRequestOutcome::Unidentified,
            );
        };
        let data = match parse_packed_binary(ingress.data) {
            Ok(data) => data,
            Err(error) => {
                return RemoteControlPairingRequestIngressOutcome::Pairing(
                    RemoteControlPairingRequestOutcome::MalformedEnvelope(error),
                )
            }
        };
        let request = match RemoteControlPairingRequest::parse(data) {
            Ok(request) => request,
            Err(error) => {
                return RemoteControlPairingRequestIngressOutcome::Pairing(
                    RemoteControlPairingRequestOutcome::MalformedMessage(error),
                )
            }
        };
        let begin = match request {
            RemoteControlPairingRequest::Begin(begin) => begin,
            request @ RemoteControlPairingRequest::Commit(_) => {
                return RemoteControlPairingRequestIngressOutcome::Pairing(
                    RemoteControlPairingRequestOutcome::UnexpectedRequest(request),
                )
            }
        };
        let target_identity = open_session.target_identity();
        let Some(target_signer) = self.held_identities.get(&target_identity) else {
            return RemoteControlPairingRequestIngressOutcome::Pairing(
                RemoteControlPairingRequestOutcome::InvariantViolation(
                    RemoteControlPairingBridgeInvariantViolation::TargetSignerUnavailable {
                        target_identity,
                    },
                ),
            );
        };
        let session = open_session.session();
        let responder =
            RemoteControlTargetPairingResponder::new(ingress.link_id, ingress.request_id);
        let arrival =
            RemoteControlTargetPairingBeginArrival::new(begin, responder, identified_controller);
        let begin_outcome =
            self.remote_control_target_pairing
                .begin(&target_signer, session, &arrival, now);
        let (attempt_id, responder, offer) = match begin_outcome {
            BeginRemoteControlTargetPairingOutcome::OfferPrepared {
                attempt_id,
                responder,
                offer,
            } => (attempt_id, responder, offer),
            BeginRemoteControlTargetPairingOutcome::Expired { expired } => {
                sink(EngineReaction::Journaled(
                    crate::engine::Journaled::RemoteControlTargetPairingExpired {
                        aborted: expired,
                    },
                ));
                return RemoteControlPairingRequestIngressOutcome::Pairing(
                    RemoteControlPairingRequestOutcome::BeginExpired { expired },
                );
            }
            BeginRemoteControlTargetPairingOutcome::Busy { active } => {
                return RemoteControlPairingRequestIngressOutcome::Pairing(
                    RemoteControlPairingRequestOutcome::BeginBusy { active },
                )
            }
            BeginRemoteControlTargetPairingOutcome::PairingUnavailable { reason } => {
                return RemoteControlPairingRequestIngressOutcome::Pairing(
                    RemoteControlPairingRequestOutcome::BeginUnavailable { reason },
                )
            }
            BeginRemoteControlTargetPairingOutcome::Rejected { rejected, reason } => {
                return RemoteControlPairingRequestIngressOutcome::Pairing(
                    RemoteControlPairingRequestOutcome::BeginRejected { rejected, reason },
                )
            }
        };
        if let Err(failure) = self.dispatch_remote_control_pairing_response(
            responder,
            RemoteControlPairingResponse::Offer(offer),
            interfaces,
            now,
            fill_entropy,
            sink,
        ) {
            let reducer = self
                .remote_control_target_pairing
                .offer_dispatch_failed(attempt_id);

            return match reducer {
                FailRemoteControlTargetPairingOfferDispatchOutcome::Aborted {
                    attempt_id: aborted,
                } if aborted == attempt_id => RemoteControlPairingRequestIngressOutcome::Pairing(
                    RemoteControlPairingRequestOutcome::OfferDispatchFailed {
                        attempt_id,
                        failure,
                    },
                ),
                reducer @ (FailRemoteControlTargetPairingOfferDispatchOutcome::Aborted { .. }
                | FailRemoteControlTargetPairingOfferDispatchOutcome::AttemptMismatch { .. }
                | FailRemoteControlTargetPairingOfferDispatchOutcome::NoOfferPrepared) => {
                    RemoteControlPairingRequestIngressOutcome::Pairing(
                        RemoteControlPairingRequestOutcome::InvariantViolation(
                            RemoteControlPairingBridgeInvariantViolation::OfferDispatchRollbackFailed {
                                attempt_id,
                                dispatch_failure: failure,
                                reducer,
                            },
                        ),
                    )
                }
            };
        }
        match self
            .remote_control_target_pairing
            .offer_dispatched(attempt_id)
        {
            DispatchRemoteControlTargetPairingOfferOutcome::AwaitingConfirmation { attempt } => {
                sink(EngineReaction::Journaled(
                    crate::engine::Journaled::RemoteControlTargetPairingConfirmationRequired(
                        attempt,
                    ),
                ));
                RemoteControlPairingRequestIngressOutcome::Pairing(
                    RemoteControlPairingRequestOutcome::OfferDispatched { attempt_id },
                )
            }
            DispatchRemoteControlTargetPairingOfferOutcome::AttemptMismatch {
                dispatched,
                active,
            } => RemoteControlPairingRequestIngressOutcome::Pairing(
                RemoteControlPairingRequestOutcome::InvariantViolation(
                    RemoteControlPairingBridgeInvariantViolation::OfferDispatchAttemptMismatch {
                        dispatched,
                        active,
                    },
                ),
            ),
            DispatchRemoteControlTargetPairingOfferOutcome::NoOfferPrepared => {
                RemoteControlPairingRequestIngressOutcome::Pairing(
                    RemoteControlPairingRequestOutcome::InvariantViolation(
                        RemoteControlPairingBridgeInvariantViolation::NoOfferPreparedAfterDispatch {
                            attempt_id,
                        },
                    ),
                )
            }
        }
    }

    fn dispatch_remote_control_pairing_response<F>(
        &mut self,
        responder: RemoteControlTargetPairingResponder,
        response: RemoteControlPairingResponse,
        interfaces: AttachedInterfaces<'_>,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> Result<(), RemoteControlPairingResponseDispatchFailure>
    where
        F: FnMut(&mut [u8]),
    {
        let mut encoded = [0u8; RemoteControlPairingResponse::MAX_ENCODED_LEN];
        let encoded_len = response
            .write_into(&mut encoded)
            .map_err(RemoteControlPairingResponseDispatchFailure::Encode)?;
        let mut data = RespondData::new();
        let mut header = [0u8; MAX_PACKED_BINARY_HEADER_LEN];
        let header_len = write_packed_binary_header(encoded_len, &mut header)
            .map_err(RemoteControlPairingResponseDispatchFailure::Pack)?;
        let header_required = header_len;
        let maximum = data.capacity();
        data.extend_from_slice(&header[..header_len]).map_err(|_| {
            RemoteControlPairingResponseDispatchFailure::Capacity {
                required: header_required,
                maximum,
            }
        })?;
        let response_required = data.len().saturating_add(encoded_len);
        data.extend_from_slice(&encoded[..encoded_len])
            .map_err(|_| RemoteControlPairingResponseDispatchFailure::Capacity {
                required: response_required,
                maximum,
            })?;
        let respond = Respond {
            link_id: responder.link_id(),
            request_id: responder.request_id(),
            payload: RespondPayload::Packed(data),
        };
        let mut iv = [0u8; crate::identity::ENCRYPTION_IV_LEN];
        fill_entropy(&mut iv);
        let mut frame = [0u8; BROADCAST_MTU];
        let dispatch = self
            .write_commanded_respond(&respond, &iv, &mut frame)
            .map_err(RemoteControlPairingResponseDispatchFailure::Write)?;
        if !interfaces.is_egress_eligible(dispatch.fire_on, crate::interfaces::Egress::Transmit) {
            return Err(
                RemoteControlPairingResponseDispatchFailure::EgressUnavailable {
                    interface: dispatch.fire_on,
                },
            );
        }
        self.links.note_outbound(&respond.link_id, now);
        sink(EngineReaction::Directive(Directive::Send {
            target: dispatch.fire_on,
            bytes: &frame[..dispatch.wire_bytes],
        }));
        Ok(())
    }

    pub(crate) fn ingest_open_remote_control_pairing(
        &self,
        id: CommandId,
        open: OpenRemoteControlPairing,
        interfaces: AttachedInterfaces<'_>,
    ) -> CommandOutcome {
        let rejection = match self.remote_control_pairing.view() {
            RemoteControlPairingView::Unavailable => {
                Some(OpenRemoteControlPairingRejection::Unavailable)
            }
            RemoteControlPairingView::Open(_) => {
                Some(OpenRemoteControlPairingRejection::AlreadyOpen)
            }
            RemoteControlPairingView::Closed
                if open.attempt_timeout.duration() > open.expires_after.duration() =>
            {
                Some(
                    OpenRemoteControlPairingRejection::AttemptTimeoutExceedsWindow {
                        attempt_timeout: open.attempt_timeout,
                        pairing_expires_after: open.expires_after,
                    },
                )
            }
            RemoteControlPairingView::Closed => match open.target {
                EgressTarget::AllInterfaces
                    if !interfaces
                        .iter()
                        .any(|interface| interface.capabilities.allows_transmit()) =>
                {
                    Some(OpenRemoteControlPairingRejection::NoTransmittingInterfaces)
                }
                target => target
                    .admit(interfaces)
                    .err()
                    .map(OpenRemoteControlPairingRejection::EgressTarget),
            },
        };
        match rejection {
            Some(rejection) => CommandOutcome::OpenRemoteControlPairingRejected { id, rejection },
            None => CommandOutcome::OwesOpenRemoteControlPairing { id, open },
        }
    }

    pub(crate) fn ingest_close_remote_control_pairing(&self, id: CommandId) -> CommandOutcome {
        match self.remote_control_pairing.view() {
            RemoteControlPairingView::Unavailable => {
                CommandOutcome::CloseRemoteControlPairingRejected {
                    id,
                    failure: CloseRemoteControlPairingFailure::Unavailable,
                }
            }
            RemoteControlPairingView::Closed | RemoteControlPairingView::Open(_) => {
                CommandOutcome::OwesCloseRemoteControlPairing { id }
            }
        }
    }

    pub(crate) fn open_remote_control_pairing_into<F>(
        &mut self,
        open: OpenRemoteControlPairing,
        interfaces: AttachedInterfaces<'_>,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> Result<RemoteControlPairingOpened, OpenRemoteControlPairingFailure>
    where
        F: FnMut(&mut [u8]),
    {
        let expires_at = open.expires_after.deadline_from(now);
        let window = RemoteControlPairingWindow::new(now, expires_at).map_err(|_| {
            OpenRemoteControlPairingFailure::Rejected(
                OpenRemoteControlPairingRejection::DeadlineOverflow,
            )
        })?;
        let (identity_secret, signer) = self.mint_pairing_identity(fill_entropy)?;
        let identity_hash = signer.identity_hash();
        let identity = RemoteControlPairingIdentity::new(identity_hash);
        let endpoint = identity.endpoint();

        let mut announce_entropy = [0u8; AnnounceEntropy::LEN];
        fill_entropy(&mut announce_entropy);
        let announce_id = AnnounceId::mint(AnnounceEntropy::new(announce_entropy), now);
        let mut availability = [0u8; BROADCAST_MDU];
        let availability_len = RemoteControlPairingAvailability::write_signed(
            &signer,
            announce_id,
            open.expires_after,
            open.public_app_data.as_borrowed(),
            &mut availability,
        )
        .map_err(OpenRemoteControlPairingFailure::WriteAvailability)?;
        let payload = SendPlainPacketPayload::from_slice(&availability[..availability_len])
            .map_err(|()| OpenRemoteControlPairingFailure::PayloadCapacity)?;
        let send = SendPlainPacket {
            destination: RemoteControlPairingAvailabilityDestination::canonical()
                .destination_hash(),
            target: open.target,
            payload,
        };
        let mut frame = [0u8; BROADCAST_MTU];
        let frame_len = self
            .write_commanded_send_plain_packet(&send, &mut frame)
            .map_err(OpenRemoteControlPairingFailure::WritePacket)?;

        self.hold_identity(identity_secret)
            .map_err(OpenRemoteControlPairingFailure::HoldIdentity)?;
        if let Err(error) = self.register_single_destination(
            &identity_hash,
            REMOTE_CONTROL_PAIRING_APPLICATION_NAME,
            REMOTE_CONTROL_PAIRING_APPLICATION_ASPECTS,
            b"",
            ProofStrategy::ProveAll,
            LinkRequestPolicy::AcceptDirect,
            RatchetPolicy::NoRatchets,
        ) {
            let _released = self.held_identities.release(&identity_hash);
            return Err(OpenRemoteControlPairingFailure::RegisterEndpoint(error));
        }

        let destination = endpoint.destination_hash();
        if !self.set_maximum_request_bytes(
            &destination,
            ByteLimit::Maximum(PAIRING_REQUEST_PLAINTEXT_MAX as u64),
        ) {
            let _unregistered = self.unregister_destination(&destination);
            let _released = self.held_identities.release(&identity_hash);
            return Err(OpenRemoteControlPairingFailure::ConfigureRequestLimit);
        }
        if let Err(error) = self.register_request_handler_hash(
            &destination,
            RequestPathHash::of(REMOTE_CONTROL_PAIRING_REQUEST_ENDPOINT_ID),
            RequestPolicy::RequireIdentified,
        ) {
            let _unregistered = self.unregister_destination(&destination);
            let _released = self.held_identities.release(&identity_hash);
            return Err(OpenRemoteControlPairingFailure::RegisterRequestEndpoint(
                error,
            ));
        }

        let session = RemoteControlPairingSession::new(
            identity,
            window,
            open.permissions,
            open.attempt_timeout,
        );
        let rejected = match self.remote_control_pairing.open(session, now) {
            PairingStateOpenOutcome::Opened => None,
            PairingStateOpenOutcome::Unavailable { unopened } => {
                Some((unopened, OpenRemoteControlPairingRejection::Unavailable))
            }
            PairingStateOpenOutcome::AlreadyOpen { unopened } => {
                Some((unopened, OpenRemoteControlPairingRejection::AlreadyOpen))
            }
            PairingStateOpenOutcome::DeadlineElapsed { unopened } => Some((
                unopened,
                OpenRemoteControlPairingRejection::DeadlineOverflow,
            )),
        };
        if let Some((unopened, rejection)) = rejected {
            let endpoint = unopened.endpoint();
            let _unregistered = self.unregister_destination(&endpoint.destination_hash());
            let _released = self
                .held_identities
                .release(&unopened.identity().identity_hash());
            return Err(OpenRemoteControlPairingFailure::Rejected(rejection));
        }

        let fanout = match send.target {
            EgressTarget::AllInterfaces => FanTarget::All,
            EgressTarget::Interface(interface) => FanTarget::Only(interface),
        };
        fan_frame(interfaces, fanout, &frame[..frame_len], sink);
        Ok(RemoteControlPairingOpened {
            endpoint,
            expires_at,
        })
    }

    pub(crate) fn close_remote_control_pairing_into<F>(
        &mut self,
        interfaces: AttachedInterfaces<'_>,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> Result<CloseRemoteControlPairingOutcome, CloseRemoteControlPairingFailure>
    where
        F: FnMut(&mut [u8]),
    {
        let (endpoint, identity) = match self.remote_control_pairing.view() {
            RemoteControlPairingView::Unavailable => {
                return Err(CloseRemoteControlPairingFailure::Unavailable)
            }
            RemoteControlPairingView::Closed => {
                return Ok(CloseRemoteControlPairingOutcome::AlreadyClosed)
            }
            RemoteControlPairingView::Open(session) => {
                (session.endpoint(), session.identity().identity_hash())
            }
        };

        match self.retire_destination(&endpoint.destination_hash(), interfaces, fill_entropy, sink)
        {
            RetireDestinationOutcome::Retired { .. } => {}
            RetireDestinationOutcome::RetirementIncomplete {
                first_remaining_link,
                retired_links,
            } => {
                return Err(CloseRemoteControlPairingFailure::RetirementIncomplete {
                    first_remaining_link,
                    retired_links,
                })
            }
            RetireDestinationOutcome::NotRegistered => {
                let _closed = self.remote_control_pairing.close();
                let _released = self.held_identities.release(&identity);
                return Err(CloseRemoteControlPairingFailure::EndpointNotRegistered);
            }
        }
        let release = self.held_identities.release(&identity);
        let closed = self.remote_control_pairing.close();
        match (release, closed) {
            (ReleaseHeldIdentityOutcome::Released, PairingStateCloseOutcome::Closed { .. }) => {
                Ok(CloseRemoteControlPairingOutcome::Closed { endpoint })
            }
            (ReleaseHeldIdentityOutcome::NotHeld, _) => {
                Err(CloseRemoteControlPairingFailure::IdentityNotHeld)
            }
            (
                ReleaseHeldIdentityOutcome::Released,
                PairingStateCloseOutcome::AlreadyClosed | PairingStateCloseOutcome::Unavailable,
            ) => Err(CloseRemoteControlPairingFailure::EndpointNotRegistered),
        }
    }

    fn mint_pairing_identity<F>(
        &self,
        fill_entropy: &mut F,
    ) -> Result<(IdentitySecretKey, InMemoryNodeIdentity), OpenRemoteControlPairingFailure>
    where
        F: FnMut(&mut [u8]),
    {
        for _ in 0..PAIRING_IDENTITY_MINT_ATTEMPTS {
            let mut secret = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
            fill_entropy(&mut secret[..]);
            let signer = InMemoryNodeIdentity::from_secret_key_bytes(&secret);
            if !self.held_identities.contains(&signer.identity_hash()) {
                return Ok((secret, signer));
            }
        }
        Err(OpenRemoteControlPairingFailure::IdentityGenerationExhausted)
    }

    pub fn fire_due_remote_control_pairing<F>(
        &mut self,
        now: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> crate::engine::WakeSchedules
    where
        F: FnMut(&mut [u8]),
    {
        match self.remote_control_target_pairing.expire(now) {
            ExpireRemoteControlTargetPairingOutcome::Expired { aborted } => {
                sink(EngineReaction::Journaled(
                    crate::engine::Journaled::RemoteControlTargetPairingExpired { aborted },
                ));
            }
            ExpireRemoteControlTargetPairingOutcome::NotDue { .. }
            | ExpireRemoteControlTargetPairingOutcome::NoActiveAttempt
            | ExpireRemoteControlTargetPairingOutcome::FinalizationInProgress { .. } => {}
        }
        let endpoint = match self.remote_control_pairing.view() {
            RemoteControlPairingView::Open(session) if now >= session.window().expires_at() => {
                session.endpoint()
            }
            RemoteControlPairingView::Unavailable
            | RemoteControlPairingView::Closed
            | RemoteControlPairingView::Open(_) => {
                return crate::engine::WakeSchedules {
                    remote_control_pairing: self.remote_control_pairing_wake(),
                    ..crate::engine::WakeSchedules::UNCHANGED
                }
            }
        };
        match self.close_remote_control_pairing_into(interfaces, fill_entropy, sink) {
            Ok(CloseRemoteControlPairingOutcome::Closed { .. }) => {
                sink(EngineReaction::Journaled(
                    crate::engine::Journaled::RemoteControlPairingExpired { endpoint },
                ));
            }
            Ok(CloseRemoteControlPairingOutcome::AlreadyClosed) => {}
            Err(failure) => {
                sink(EngineReaction::Journaled(
                    crate::engine::Journaled::RemoteControlPairingExpiryFailed {
                        endpoint,
                        failure,
                    },
                ));
            }
        }
        crate::engine::WakeSchedules {
            remote_control_pairing: self.remote_control_pairing_wake(),
            ..crate::engine::WakeSchedules::UNCHANGED
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

    use super::*;
    use crate::crypto::{x25519_diffie_hellman, x25519_public_key, X25519SecretKey};
    use crate::engine::test_support::{fixed_secret_key, routable_descriptor, TestStorageLayout};
    use crate::engine::{
        CloseRemoteControlPairing, CommandId, Directive, IngestIo, IssuedCommand, Journaled,
        PrnsCommand, Settlement, WakeReason,
    };
    use crate::identity::{IdentityPublicKeys, IdentitySigner};
    use crate::interfaces::{EgressCapability, InboundPacket, InterfaceDescriptor, InterfaceId};
    use crate::remote_control::{
        RemoteControlControllerIdentity, RemoteControlPairingAttemptTimeout,
        RemoteControlPairingBegin, RemoteControlPairingEndpoint, RemoteControlPairingExpiresAfter,
        RemoteControlPairingPermissions, RemoteControlPairingPublicAppDataBytes,
        RemoteControlRequestKind, RemoteControlRequestSet, RemoteControlTargetPairingView,
    };
    use crate::routing::dedup::PacketHash;
    use crate::routing::links::data::write_link_packet;
    use crate::routing::links::request::{
        parse_response_plaintext, write_request_plaintext, RequestId,
    };
    use crate::routing::links::table::RespondingLink;
    use crate::routing::links::LinkKey;
    use crate::storage::TestFixedStorage;
    use crate::units::{DurationMillis, RttMillis};
    use crate::wire::{DestinationType, PacketType, WireContext, WirePacketHeader};

    fn open(target: EgressTarget) -> OpenRemoteControlPairing {
        OpenRemoteControlPairing {
            target,
            expires_after: RemoteControlPairingExpiresAfter::try_from(DurationMillis(60_000))
                .unwrap(),
            attempt_timeout: RemoteControlPairingAttemptTimeout::try_from(DurationMillis(30_000))
                .unwrap(),
            permissions: RemoteControlPairingPermissions::try_from(RemoteControlRequestSet::only(
                RemoteControlRequestKind::Describe,
            ))
            .unwrap(),
            public_app_data: RemoteControlPairingPublicAppDataBytes::try_from(
                b"nearby node".as_slice(),
            )
            .unwrap(),
        }
    }

    fn configure_pairing<S: StorageLayout>(
        engine: &mut crate::engine::EngineState<S>,
    ) -> Result<RemoteControlPairingAvailabilityDestination, ConfigureRemoteControlPairingError>
    {
        let target_identity = engine.hold_identity(fixed_secret_key()).unwrap();
        engine.configure_remote_control_pairing(target_identity)
    }

    fn stable_target_identity() -> crate::identity::IdentityHash {
        InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key()).identity_hash()
    }

    fn controller_identity() -> RemoteControlControllerIdentity {
        let signer = InMemoryNodeIdentity::from_secret_key_bytes(&Zeroizing::new(
            [0xD2; IDENTITY_SECRET_KEY_LEN],
        ));
        RemoteControlControllerIdentity::new(IdentityPublicKeys {
            encryption: signer.encryption_public_key(),
            signing: signer.signing_public_key(),
        })
    }

    fn fill_pairing_entropy(bytes: &mut [u8]) {
        match bytes.len() {
            IDENTITY_SECRET_KEY_LEN => bytes.fill(0xA1),
            AnnounceEntropy::LEN => bytes.fill(0xB2),
            length => panic!("unexpected entropy request of {length} bytes"),
        }
    }

    fn open_pairing_link(
        identified_controller: crate::identity::IdentityHash,
    ) -> (
        crate::engine::EngineState<TestStorageLayout>,
        [InterfaceDescriptor; 1],
        RemoteControlPairingEndpoint,
        LinkId,
        LinkKey,
        RemoteControlControllerIdentity,
    ) {
        let interface = InterfaceId::new([0xD1; 8]);
        let interfaces = [routable_descriptor(interface)];
        let mut engine = crate::engine::EngineState::<TestStorageLayout>::default();
        configure_pairing(&mut engine).unwrap();
        let opened = engine
            .open_remote_control_pairing_into(
                open(EgressTarget::AllInterfaces),
                AttachedInterfaces::new(&interfaces),
                InstantMillis(1_000),
                &mut fill_pairing_entropy,
                &mut |_| {},
            )
            .unwrap();
        let pairing_identity = match engine.remote_control_pairing_view() {
            RemoteControlPairingView::Open(session) => session.identity().identity_hash(),
            RemoteControlPairingView::Unavailable | RemoteControlPairingView::Closed => {
                panic!("open pairing")
            }
        };
        let controller = controller_identity();
        let link_id = LinkId::new([0xD3; 16]);
        let shared = x25519_diffie_hellman(
            &X25519SecretKey::new([0xD4; 32]),
            &x25519_public_key(&X25519SecretKey::new([0xD5; 32])),
        );
        let peer_key = LinkKey::derive(&link_id, &shared);
        engine
            .links
            .track_responding(RespondingLink {
                link_id,
                key: LinkKey::derive(&link_id, &shared),
                requested_at: InstantMillis(1_500),
                timeout_at: InstantMillis(60_000),
                mtu: BROADCAST_MTU,
                initiator_signing: *controller.public_keys().signing.as_ed25519(),
                destination: opened.endpoint.destination_hash(),
                identity: pairing_identity,
                proof_strategy: ProofStrategy::ProveAll,
            })
            .unwrap();
        engine
            .links
            .activate_responding(
                &link_id,
                RttMillis::new(25),
                interface,
                InstantMillis(1_600),
            )
            .unwrap();
        engine
            .links
            .note_identified(&link_id, identified_controller);
        (
            engine,
            interfaces,
            opened.endpoint,
            link_id,
            peer_key,
            controller,
        )
    }

    fn pairing_request_frame(
        request: RemoteControlPairingRequest,
        link_id: &LinkId,
        key: &LinkKey,
    ) -> std::vec::Vec<u8> {
        let mut encoded = [0u8; RemoteControlPairingRequest::MAX_ENCODED_LEN];
        let encoded_len = request.write_into(&mut encoded).unwrap();
        pairing_payload_frame(&encoded[..encoded_len], link_id, key)
    }

    fn pairing_payload_frame(encoded: &[u8], link_id: &LinkId, key: &LinkKey) -> std::vec::Vec<u8> {
        let packed = packed_pairing_payload(encoded);
        let mut plaintext = [0u8; 256];
        let plaintext_len = write_request_plaintext(
            InstantMillis(1_900),
            &RequestPathHash::of(REMOTE_CONTROL_PAIRING_REQUEST_ENDPOINT_ID),
            &packed,
            &mut plaintext,
        )
        .unwrap();
        let mut frame = [0u8; BROADCAST_MTU];
        let wire_len = write_link_packet(
            link_id,
            key,
            BROADCAST_MTU,
            WireContext::Request,
            &plaintext[..plaintext_len],
            &[0xD6; 16],
            &mut frame,
        )
        .unwrap();
        frame[..wire_len].to_vec()
    }

    fn packed_pairing_payload(encoded: &[u8]) -> std::vec::Vec<u8> {
        let mut packed = [0u8; RemoteControlPairingRequest::MAX_ENCODED_LEN + 5];
        let header_len = write_packed_binary_header(encoded.len(), &mut packed).unwrap();
        packed[header_len..header_len + encoded.len()].copy_from_slice(encoded);
        packed[..header_len + encoded.len()].to_vec()
    }

    #[test]
    fn configuration_registers_the_listener_before_becoming_available() {
        let mut unheld = crate::engine::EngineState::<TestStorageLayout>::default();
        assert_eq!(
            unheld.configure_remote_control_pairing(crate::identity::IdentityHash::new([0x90; 16])),
            Err(ConfigureRemoteControlPairingError::TargetIdentityNotHeld),
        );
        assert_eq!(
            unheld.remote_control_pairing_view(),
            RemoteControlPairingView::Unavailable,
        );
        assert_eq!(unheld.upstream_app_destinations().count(), 0);

        let mut engine = crate::engine::EngineState::<TestStorageLayout>::default();
        let canonical = RemoteControlPairingAvailabilityDestination::canonical();
        assert_eq!(configure_pairing(&mut engine), Ok(canonical));
        assert_eq!(
            engine.remote_control_pairing_view(),
            RemoteControlPairingView::Closed,
        );
        assert!(engine
            .upstream_app_destinations()
            .any(|registration| registration.destination == canonical.destination_hash()));
        assert_eq!(
            configure_pairing(&mut engine),
            Err(ConfigureRemoteControlPairingError::AlreadyConfigured),
        );
        assert_eq!(engine.upstream_app_destinations().count(), 1);

        type FullRegistry = TestFixedStorage<8, 8, 512, 1, 2, 16, 2, 2, 2, 2, 4, 2>;
        let mut full = crate::engine::EngineState::<FullRegistry>::default();
        full.register_plain_destination("full", &["registry"])
            .unwrap();
        assert_eq!(
            configure_pairing(&mut full),
            Err(ConfigureRemoteControlPairingError::RegisterAvailability(
                crate::routing::upstream_app_destinations::RegisterDestinationError::RegistryFull,
            )),
        );
        assert_eq!(
            full.remote_control_pairing_view(),
            RemoteControlPairingView::Unavailable,
        );
    }

    #[test]
    fn opening_emits_one_signed_availability_and_commits_the_exact_session() {
        let selected = InterfaceId::new([0x91; 8]);
        let other = InterfaceId::new([0x92; 8]);
        let interfaces = [routable_descriptor(other), routable_descriptor(selected)];
        let mut engine = crate::engine::EngineState::<TestStorageLayout>::default();
        assert_eq!(
            configure_pairing(&mut engine),
            Ok(RemoteControlPairingAvailabilityDestination::canonical()),
        );
        let mut emitted = std::vec::Vec::new();
        let mut settled = None;

        let schedules = engine.ingest_command_into(
            IssuedCommand {
                id: CommandId(41),
                command: PrnsCommand::OpenRemoteControlPairing(open(EgressTarget::Interface(
                    selected,
                ))),
            },
            AttachedInterfaces::new(&interfaces),
            InstantMillis(1_000),
            &mut fill_pairing_entropy,
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::Send { target, bytes }) => {
                    emitted.push((target, bytes.to_vec()));
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    settled = Some((id, settlement));
                }
                EngineReaction::Directive(_) | EngineReaction::Journaled(_) => {}
            },
        );

        let [(target, frame)] = emitted.as_slice() else {
            panic!("one direct PLAIN frame")
        };
        assert_eq!(*target, selected);
        let (header, payload) = WirePacketHeader::parse(frame).unwrap();
        assert_eq!(header.destination_type, DestinationType::Plain);
        assert_eq!(header.packet_type, PacketType::Data);
        assert_eq!(
            header.address,
            RemoteControlPairingAvailabilityDestination::canonical()
                .destination_hash()
                .to_address(),
        );
        let availability = RemoteControlPairingAvailability::parse(payload).unwrap();
        assert_eq!(availability.public_app_data().as_bytes(), b"nearby node");
        assert_eq!(
            availability.expires_after().duration(),
            DurationMillis(60_000),
        );
        let endpoint = availability.pairing_endpoint();
        assert_eq!(
            settled,
            Some((
                CommandId(41),
                Settlement::OpenRemoteControlPairing(Ok(RemoteControlPairingOpened {
                    endpoint,
                    expires_at: InstantMillis(61_000),
                })),
            )),
        );
        assert_eq!(
            engine.remote_control_pairing_view(),
            RemoteControlPairingView::Open(&RemoteControlPairingSession::new(
                availability.pairing_identity(),
                RemoteControlPairingWindow::new(InstantMillis(1_000), InstantMillis(61_000),)
                    .unwrap(),
                RemoteControlPairingPermissions::try_from(RemoteControlRequestSet::only(
                    RemoteControlRequestKind::Describe,
                ))
                .unwrap(),
                RemoteControlPairingAttemptTimeout::try_from(DurationMillis(30_000)).unwrap(),
            )),
        );
        let endpoint_registration = engine
            .upstream_app_destinations
            .lookup_single(&endpoint.destination_hash())
            .unwrap();
        assert_eq!(
            endpoint_registration.link_request_policy,
            LinkRequestPolicy::AcceptDirect,
        );
        assert_eq!(
            endpoint_registration.maximum_request_bytes,
            ByteLimit::Maximum(PAIRING_REQUEST_PLAINTEXT_MAX as u64),
        );
        let pairing_path = RequestPathHash::of(REMOTE_CONTROL_PAIRING_REQUEST_ENDPOINT_ID);
        assert!(!engine.request_handlers.permits(
            &endpoint.destination_hash(),
            &pairing_path,
            None,
        ));
        assert!(engine.request_handlers.permits(
            &endpoint.destination_hash(),
            &pairing_path,
            Some(&crate::identity::IdentityHash::new([0xD4; 16])),
        ));
        assert_eq!(
            engine.held_identity_hashes(),
            &[
                stable_target_identity(),
                availability.pairing_identity().identity_hash(),
            ]
        );
        assert_eq!(
            schedules.remote_control_pairing,
            crate::engine::WakeSchedule::At(InstantMillis(61_000)),
        );
        assert_eq!(
            engine.next_wake(InstantMillis(1_000), AttachedInterfaces::new(&interfaces),),
            crate::engine::NextWake::At {
                at: InstantMillis(61_000),
                reason: WakeReason::RemoteControlPairing,
            },
        );
    }

    #[test]
    fn authenticated_begin_returns_one_real_offer_and_one_confirmation_event() {
        let expected_controller = controller_identity();
        let (mut engine, interfaces, endpoint, link_id, peer_key, controller) =
            open_pairing_link(expected_controller.identity_hash());
        let begin = RemoteControlPairingBegin::new(controller);
        let mut request_frame = pairing_request_frame(
            RemoteControlPairingRequest::Begin(RemoteControlPairingBegin::new(controller)),
            &link_id,
            &peer_key,
        );
        let request_id = RequestId::of_packet(&PacketHash::of_wire_packet(&request_frame).unwrap());
        let mut response_frames = std::vec::Vec::new();
        let mut confirmation = None;
        let mut ordinary_requests = 0usize;

        let schedules = engine.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: interfaces[0].id,
                bytes: &mut request_frame,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&interfaces),
                now: InstantMillis(2_000),
                fill_entropy: &mut |bytes| bytes.fill(0xD7),
                should_prove: &mut |_| false,
                should_accept_resource: &mut |_| false,
                sink: &mut |reaction| match reaction {
                    EngineReaction::Directive(Directive::Send { bytes, .. }) => {
                        response_frames.push(bytes.to_vec());
                    }
                    EngineReaction::Journaled(
                        Journaled::RemoteControlTargetPairingConfirmationRequired(attempt),
                    ) => {
                        confirmation = Some((
                            attempt.attempt_id(),
                            attempt.controller().identity_hash(),
                            attempt.target().identity_hash(),
                            attempt.permissions().clone(),
                            attempt.confirmation_code(),
                            attempt.window().expires_at(),
                        ));
                    }
                    EngineReaction::Journaled(Journaled::RequestReceived { .. }) => {
                        ordinary_requests += 1;
                    }
                    EngineReaction::Directive(_) | EngineReaction::Journaled(_) => {}
                },
            },
        );

        let [response_frame] = response_frames.as_slice() else {
            panic!("one pairing response")
        };
        let (header, sealed) = WirePacketHeader::parse(response_frame).unwrap();
        assert_eq!(header.context, WireContext::Response);
        assert_eq!(header.address, link_id.to_address());
        let mut plaintext = [0u8; BROADCAST_MTU];
        let plaintext_len = peer_key.open(sealed, &mut plaintext).unwrap();
        let (responded_to, packed_response) =
            parse_response_plaintext(&plaintext[..plaintext_len]).unwrap();
        assert_eq!(responded_to, request_id);
        let response =
            RemoteControlPairingResponse::parse(parse_packed_binary(packed_response).unwrap())
                .unwrap();
        let RemoteControlPairingResponse::Offer(offer) = response else {
            panic!("offer response")
        };
        let transcript = offer
            .verify(
                crate::remote_control::RemoteControlPairingContext::new(endpoint, link_id),
                &begin,
            )
            .unwrap();
        let expected_attempt_id = (&transcript).into();
        assert_eq!(
            confirmation,
            Some((
                expected_attempt_id,
                expected_controller.identity_hash(),
                stable_target_identity(),
                RemoteControlPairingPermissions::try_from(RemoteControlRequestSet::only(
                    RemoteControlRequestKind::Describe,
                ))
                .unwrap(),
                transcript.confirmation_code(),
                InstantMillis(32_000),
            )),
        );
        assert_eq!(ordinary_requests, 0);
        assert_eq!(
            schedules.remote_control_pairing,
            crate::engine::WakeSchedule::At(InstantMillis(32_000)),
        );
        assert!(matches!(
            engine.remote_control_target_pairing.view(),
            RemoteControlTargetPairingView::AwaitingBoth(attempt)
                if attempt.attempt_id() == expected_attempt_id
        ));
        let mut expired = None;
        let expiry_schedules = engine.fire_due_remote_control_pairing(
            InstantMillis(32_000),
            AttachedInterfaces::new(&interfaces),
            &mut |bytes| bytes.fill(0xD8),
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::RemoteControlTargetPairingExpired {
                    aborted,
                }) = reaction
                {
                    expired = Some(aborted.attempt_id());
                }
            },
        );
        assert_eq!(expired, Some(expected_attempt_id));
        assert_eq!(
            engine.remote_control_target_pairing.view(),
            RemoteControlTargetPairingView::Idle,
        );
        assert!(matches!(
            engine.remote_control_pairing_view(),
            RemoteControlPairingView::Open(_)
        ));
        assert_eq!(
            expiry_schedules.remote_control_pairing,
            crate::engine::WakeSchedule::At(InstantMillis(61_000)),
        );
    }

    #[test]
    fn mismatched_and_invalid_pairing_requests_are_silently_consumed() {
        let claimed = controller_identity();
        let identified = crate::identity::IdentityHash::new([0xE1; 16]);
        let (mut mismatch_engine, interfaces, _, link_id, peer_key, _) =
            open_pairing_link(identified);
        let mut mismatch_frame = pairing_request_frame(
            RemoteControlPairingRequest::Begin(RemoteControlPairingBegin::new(claimed)),
            &link_id,
            &peer_key,
        );
        let mut mismatch_reactions = 0usize;
        mismatch_engine.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: interfaces[0].id,
                bytes: &mut mismatch_frame,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&interfaces),
                now: InstantMillis(2_000),
                fill_entropy: &mut |bytes| bytes.fill(0xE2),
                should_prove: &mut |_| false,
                should_accept_resource: &mut |_| false,
                sink: &mut |_| mismatch_reactions += 1,
            },
        );
        assert_eq!(mismatch_reactions, 0);
        assert_eq!(
            mismatch_engine.remote_control_target_pairing.view(),
            RemoteControlTargetPairingView::Idle,
        );

        let (mut invalid_engine, interfaces, endpoint, link_id, peer_key, controller) =
            open_pairing_link(claimed.identity_hash());
        let mut malformed = pairing_payload_frame(&[0xFF], &link_id, &peer_key);
        let mut malformed_reactions = 0usize;
        invalid_engine.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: interfaces[0].id,
                bytes: &mut malformed,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&interfaces),
                now: InstantMillis(2_000),
                fill_entropy: &mut |bytes| bytes.fill(0xE3),
                should_prove: &mut |_| false,
                should_accept_resource: &mut |_| false,
                sink: &mut |_| malformed_reactions += 1,
            },
        );
        assert_eq!(malformed_reactions, 0);

        let target_signer = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let begin = RemoteControlPairingBegin::new(controller);
        let prepared = match invalid_engine.remote_control_pairing_view() {
            RemoteControlPairingView::Open(session) => {
                crate::remote_control::RemoteControlPairingPreparedOffer::new(
                    &target_signer,
                    crate::remote_control::RemoteControlPairingContext::new(endpoint, link_id),
                    &begin,
                    session.permissions().clone(),
                    session.attempt_timeout(),
                )
            }
            RemoteControlPairingView::Unavailable | RemoteControlPairingView::Closed => {
                panic!("open pairing")
            }
        };
        let commit = crate::remote_control::RemoteControlPairingCommit::new(prepared.transcript());
        let mut premature_commit = pairing_request_frame(
            RemoteControlPairingRequest::Commit(commit),
            &link_id,
            &peer_key,
        );
        let mut commit_reactions = 0usize;
        invalid_engine.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(2_100),
                source_interface: interfaces[0].id,
                bytes: &mut premature_commit,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&interfaces),
                now: InstantMillis(2_100),
                fill_entropy: &mut |bytes| bytes.fill(0xE4),
                should_prove: &mut |_| false,
                should_accept_resource: &mut |_| false,
                sink: &mut |_| commit_reactions += 1,
            },
        );
        assert_eq!(commit_reactions, 0);
        assert_eq!(
            invalid_engine.remote_control_target_pairing.view(),
            RemoteControlTargetPairingView::Idle,
        );
    }

    #[test]
    fn pairing_ingress_preserves_exact_silent_outcomes() {
        let claimed = controller_identity();
        let claimed_identity = claimed.identity_hash();
        let identified = crate::identity::IdentityHash::new([0xE7; 16]);
        let (mut engine, interfaces, endpoint, link_id, _, _) = open_pairing_link(identified);
        let request_id = RequestId([0xE8; 16]);
        let responder = RemoteControlTargetPairingResponder::new(link_id, request_id);
        let path_hash = RequestPathHash::of(REMOTE_CONTROL_PAIRING_REQUEST_ENDPOINT_ID);
        let mut encoded = [0u8; RemoteControlPairingRequest::MAX_ENCODED_LEN];
        let begin_len = RemoteControlPairingRequest::Begin(RemoteControlPairingBegin::new(claimed))
            .write_into(&mut encoded)
            .unwrap();
        let begin = packed_pairing_payload(&encoded[..begin_len]);
        let mut reactions = 0usize;

        assert_eq!(
            engine.ingest_remote_control_pairing_request(
                RemoteControlPairingRequestIngress {
                    destination: endpoint.destination_hash(),
                    link_id,
                    request_id,
                    requester: Some(identified),
                    path_hash,
                    data: &begin,
                },
                AttachedInterfaces::new(&interfaces),
                InstantMillis(2_000),
                &mut |bytes| bytes.fill(0xE9),
                &mut |_| reactions += 1,
            ),
            RemoteControlPairingRequestIngressOutcome::Pairing(
                RemoteControlPairingRequestOutcome::BeginRejected {
                    rejected: responder,
                    reason: RemoteControlTargetPairingBeginRejection::ControllerIdentityMismatch {
                        claimed: claimed_identity,
                        identified,
                    },
                },
            ),
        );
        assert_eq!(reactions, 0);
        assert_eq!(
            engine.ingest_remote_control_pairing_request(
                RemoteControlPairingRequestIngress {
                    destination: endpoint.destination_hash(),
                    link_id,
                    request_id,
                    requester: None,
                    path_hash,
                    data: &begin,
                },
                AttachedInterfaces::new(&interfaces),
                InstantMillis(2_000),
                &mut |bytes| bytes.fill(0xEA),
                &mut |_| reactions += 1,
            ),
            RemoteControlPairingRequestIngressOutcome::Pairing(
                RemoteControlPairingRequestOutcome::Unidentified,
            ),
        );
        assert_eq!(
            engine.ingest_remote_control_pairing_request(
                RemoteControlPairingRequestIngress {
                    destination: endpoint.destination_hash(),
                    link_id,
                    request_id,
                    requester: Some(identified),
                    path_hash: RequestPathHash::of("/not-pair"),
                    data: &begin,
                },
                AttachedInterfaces::new(&interfaces),
                InstantMillis(2_000),
                &mut |bytes| bytes.fill(0xEB),
                &mut |_| reactions += 1,
            ),
            RemoteControlPairingRequestIngressOutcome::ForwardToApplication,
        );
        assert_eq!(
            engine.ingest_remote_control_pairing_request(
                RemoteControlPairingRequestIngress {
                    destination: endpoint.destination_hash(),
                    link_id,
                    request_id,
                    requester: Some(identified),
                    path_hash,
                    data: &[0xFF],
                },
                AttachedInterfaces::new(&interfaces),
                InstantMillis(2_000),
                &mut |bytes| bytes.fill(0xEC),
                &mut |_| reactions += 1,
            ),
            RemoteControlPairingRequestIngressOutcome::Pairing(
                RemoteControlPairingRequestOutcome::MalformedEnvelope(
                    PackedBinaryParseError::NotBinary,
                ),
            ),
        );
        let malformed = packed_pairing_payload(&[0xFF]);
        assert_eq!(
            engine.ingest_remote_control_pairing_request(
                RemoteControlPairingRequestIngress {
                    destination: endpoint.destination_hash(),
                    link_id,
                    request_id,
                    requester: Some(identified),
                    path_hash,
                    data: &malformed,
                },
                AttachedInterfaces::new(&interfaces),
                InstantMillis(2_000),
                &mut |bytes| bytes.fill(0xED),
                &mut |_| reactions += 1,
            ),
            RemoteControlPairingRequestIngressOutcome::Pairing(
                RemoteControlPairingRequestOutcome::MalformedMessage(
                    RemoteControlPairingMessageParseError::UnsupportedVersion { found: 0xFF },
                ),
            ),
        );
        let target_signer = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let commit = match engine.remote_control_pairing_view() {
            RemoteControlPairingView::Open(session) => {
                let begin = RemoteControlPairingBegin::new(controller_identity());
                let prepared = crate::remote_control::RemoteControlPairingPreparedOffer::new(
                    &target_signer,
                    crate::remote_control::RemoteControlPairingContext::new(endpoint, link_id),
                    &begin,
                    session.permissions().clone(),
                    session.attempt_timeout(),
                );
                crate::remote_control::RemoteControlPairingCommit::new(prepared.transcript())
            }
            RemoteControlPairingView::Unavailable | RemoteControlPairingView::Closed => {
                panic!("open pairing")
            }
        };
        let commit_request = RemoteControlPairingRequest::Commit(commit);
        let commit_len = commit_request.write_into(&mut encoded).unwrap();
        let packed_commit = packed_pairing_payload(&encoded[..commit_len]);
        assert_eq!(
            engine.ingest_remote_control_pairing_request(
                RemoteControlPairingRequestIngress {
                    destination: endpoint.destination_hash(),
                    link_id,
                    request_id,
                    requester: Some(identified),
                    path_hash,
                    data: &packed_commit,
                },
                AttachedInterfaces::new(&interfaces),
                InstantMillis(2_000),
                &mut |bytes| bytes.fill(0xEE),
                &mut |_| reactions += 1,
            ),
            RemoteControlPairingRequestIngressOutcome::Pairing(
                RemoteControlPairingRequestOutcome::UnexpectedRequest(
                    RemoteControlPairingRequest::Commit(commit),
                ),
            ),
        );
        assert_eq!(reactions, 0);
        assert_eq!(
            engine.remote_control_target_pairing.view(),
            RemoteControlTargetPairingView::Idle,
        );
    }

    #[test]
    fn a_begin_that_discovers_expiry_emits_the_exact_aborted_attempt() {
        let controller = controller_identity();
        let (mut engine, interfaces, endpoint, link_id, _, _) =
            open_pairing_link(controller.identity_hash());
        let mut encoded = [0u8; RemoteControlPairingRequest::MAX_ENCODED_LEN];
        let encoded_len =
            RemoteControlPairingRequest::Begin(RemoteControlPairingBegin::new(controller))
                .write_into(&mut encoded)
                .unwrap();
        let packed = packed_pairing_payload(&encoded[..encoded_len]);
        let ingress = |request_id| RemoteControlPairingRequestIngress {
            destination: endpoint.destination_hash(),
            link_id,
            request_id,
            requester: Some(controller.identity_hash()),
            path_hash: RequestPathHash::of(REMOTE_CONTROL_PAIRING_REQUEST_ENDPOINT_ID),
            data: &packed,
        };
        let first = engine.ingest_remote_control_pairing_request(
            ingress(RequestId([0xF1; 16])),
            AttachedInterfaces::new(&interfaces),
            InstantMillis(2_000),
            &mut |bytes| bytes.fill(0xF2),
            &mut |_| {},
        );
        let RemoteControlPairingRequestIngressOutcome::Pairing(
            RemoteControlPairingRequestOutcome::OfferDispatched { attempt_id },
        ) = first
        else {
            panic!("first offer dispatched")
        };
        let mut competing_reactions = 0usize;
        assert_eq!(
            engine.ingest_remote_control_pairing_request(
                ingress(RequestId([0xF3; 16])),
                AttachedInterfaces::new(&interfaces),
                InstantMillis(2_001),
                &mut |bytes| bytes.fill(0xF4),
                &mut |_| competing_reactions += 1,
            ),
            RemoteControlPairingRequestIngressOutcome::Pairing(
                RemoteControlPairingRequestOutcome::BeginBusy { active: attempt_id },
            ),
        );
        assert_eq!(competing_reactions, 0);
        let expected = RemoteControlTargetPairingAborted::AwaitingBoth {
            attempt_id,
            context: crate::remote_control::RemoteControlPairingContext::new(endpoint, link_id),
        };
        let mut emitted = None;

        assert_eq!(
            engine.ingest_remote_control_pairing_request(
                ingress(RequestId([0xF5; 16])),
                AttachedInterfaces::new(&interfaces),
                InstantMillis(32_000),
                &mut |bytes| bytes.fill(0xF6),
                &mut |reaction| {
                    if let EngineReaction::Journaled(
                        Journaled::RemoteControlTargetPairingExpired { aborted },
                    ) = reaction
                    {
                        emitted = Some(aborted);
                    }
                },
            ),
            RemoteControlPairingRequestIngressOutcome::Pairing(
                RemoteControlPairingRequestOutcome::BeginExpired { expired: expected },
            ),
        );
        assert_eq!(emitted, Some(expected));
        assert_eq!(
            engine.remote_control_target_pairing.view(),
            RemoteControlTargetPairingView::Idle,
        );
        assert_eq!(
            engine.remote_control_pairing_wake(),
            crate::engine::WakeSchedule::At(InstantMillis(61_000)),
        );
    }

    #[test]
    fn retiring_the_pairing_link_aborts_only_its_confirmation_attempt() {
        let controller = controller_identity();
        let (mut engine, interfaces, _, link_id, peer_key, _) =
            open_pairing_link(controller.identity_hash());
        let mut request_frame = pairing_request_frame(
            RemoteControlPairingRequest::Begin(RemoteControlPairingBegin::new(controller)),
            &link_id,
            &peer_key,
        );
        engine.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: interfaces[0].id,
                bytes: &mut request_frame,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&interfaces),
                now: InstantMillis(2_000),
                fill_entropy: &mut |bytes| bytes.fill(0xE5),
                should_prove: &mut |_| false,
                should_accept_resource: &mut |_| false,
                sink: &mut |_| {},
            },
        );
        let attempt_id = match engine.remote_control_target_pairing.view() {
            RemoteControlTargetPairingView::AwaitingBoth(attempt) => attempt.attempt_id(),
            RemoteControlTargetPairingView::Idle
            | RemoteControlTargetPairingView::OfferPrepared(_)
            | RemoteControlTargetPairingView::AwaitingTargetApproval(_)
            | RemoteControlTargetPairingView::AwaitingControllerCommit(_)
            | RemoteControlTargetPairingView::Authorizing(_)
            | RemoteControlTargetPairingView::Completing(_) => panic!("confirmation attempt"),
        };
        let mut pairing_link_closed = None;
        let mut ordinary_link_closed = None;
        engine.retire_link(
            &link_id,
            crate::engine::LinkClosedReason::PeerClosed,
            &mut |reaction| match reaction {
                EngineReaction::Journaled(Journaled::RemoteControlTargetPairingLinkClosed {
                    aborted,
                }) => pairing_link_closed = Some(aborted.attempt_id()),
                EngineReaction::Journaled(Journaled::LinkClosed { link_id, reason }) => {
                    ordinary_link_closed = Some((link_id, reason));
                }
                EngineReaction::Directive(_) | EngineReaction::Journaled(_) => {}
            },
        );
        assert_eq!(pairing_link_closed, Some(attempt_id));
        assert_eq!(
            ordinary_link_closed,
            Some((link_id, crate::engine::LinkClosedReason::PeerClosed)),
        );
        assert_eq!(
            engine.remote_control_target_pairing.view(),
            RemoteControlTargetPairingView::Idle,
        );
        assert!(matches!(
            engine.remote_control_pairing_view(),
            RemoteControlPairingView::Open(_)
        ));
    }

    #[test]
    fn an_undeliverable_offer_never_exposes_a_confirmation_attempt() {
        let controller = controller_identity();
        let (mut engine, mut interfaces, endpoint, link_id, _, _) =
            open_pairing_link(controller.identity_hash());
        interfaces[0].capabilities.egress = EgressCapability::Disabled;
        let mut encoded = [0u8; RemoteControlPairingRequest::MAX_ENCODED_LEN];
        let encoded_len =
            RemoteControlPairingRequest::Begin(RemoteControlPairingBegin::new(controller))
                .write_into(&mut encoded)
                .unwrap();
        let packed = packed_pairing_payload(&encoded[..encoded_len]);
        let request_id = RequestId([0xEF; 16]);
        let mut reactions = 0usize;
        let outcome = engine.ingest_remote_control_pairing_request(
            RemoteControlPairingRequestIngress {
                destination: endpoint.destination_hash(),
                link_id,
                request_id,
                requester: Some(controller.identity_hash()),
                path_hash: RequestPathHash::of(REMOTE_CONTROL_PAIRING_REQUEST_ENDPOINT_ID),
                data: &packed,
            },
            AttachedInterfaces::new(&interfaces),
            InstantMillis(2_000),
            &mut |bytes| bytes.fill(0xF0),
            &mut |_| reactions += 1,
        );
        let RemoteControlPairingRequestIngressOutcome::Pairing(
            RemoteControlPairingRequestOutcome::OfferDispatchFailed {
                attempt_id: _,
                failure,
            },
        ) = outcome
        else {
            panic!("offer dispatch failure")
        };
        assert_eq!(
            failure,
            RemoteControlPairingResponseDispatchFailure::EgressUnavailable {
                interface: interfaces[0].id,
            },
        );
        assert_eq!(reactions, 0);
        assert_eq!(
            engine.remote_control_target_pairing.view(),
            RemoteControlTargetPairingView::Idle,
        );
        assert_eq!(
            engine.remote_control_pairing_wake(),
            crate::engine::WakeSchedule::At(InstantMillis(61_000)),
        );
    }

    #[test]
    fn unavailable_and_non_transmitting_opens_reject_before_drawing_entropy() {
        let receive_only = InterfaceId::new([0x93; 8]);
        let mut descriptor = routable_descriptor(receive_only);
        descriptor.capabilities.egress = EgressCapability::Disabled;
        let interfaces = [descriptor];
        let mut engine = crate::engine::EngineState::<TestStorageLayout>::default();
        let mut entropy_calls = 0usize;
        let mut settlements = std::vec::Vec::new();
        for (id, expected) in [
            (
                CommandId(51),
                OpenRemoteControlPairingRejection::Unavailable,
            ),
            (
                CommandId(52),
                OpenRemoteControlPairingRejection::NoTransmittingInterfaces,
            ),
        ] {
            if id == CommandId(52) {
                assert_eq!(
                    configure_pairing(&mut engine),
                    Ok(RemoteControlPairingAvailabilityDestination::canonical()),
                );
            }
            let _ = engine.ingest_command_into(
                IssuedCommand {
                    id,
                    command: PrnsCommand::OpenRemoteControlPairing(open(
                        EgressTarget::AllInterfaces,
                    )),
                },
                AttachedInterfaces::new(&interfaces),
                InstantMillis(1_000),
                &mut |_| entropy_calls += 1,
                &mut |reaction| {
                    if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                        reaction
                    {
                        settlements.push((id, settlement));
                    }
                },
            );
            assert_eq!(
                settlements.last(),
                Some(&(
                    id,
                    Settlement::OpenRemoteControlPairing(Err(
                        OpenRemoteControlPairingFailure::Rejected(expected),
                    )),
                )),
            );
        }
        assert_eq!(entropy_calls, 0);
        assert_eq!(engine.held_identity_hashes(), &[stable_target_identity()]);
        assert_eq!(
            engine.remote_control_pairing_view(),
            RemoteControlPairingView::Closed,
        );
    }

    #[test]
    fn an_attempt_timeout_cannot_exceed_the_pairing_window() {
        let interface = InterfaceId::new([0x93; 8]);
        let interfaces = [routable_descriptor(interface)];
        let mut engine = crate::engine::EngineState::<TestStorageLayout>::default();
        configure_pairing(&mut engine).unwrap();
        let mut impossible = open(EgressTarget::AllInterfaces);
        impossible.expires_after =
            RemoteControlPairingExpiresAfter::try_from(DurationMillis(10_000)).unwrap();
        impossible.attempt_timeout =
            RemoteControlPairingAttemptTimeout::try_from(DurationMillis(10_001)).unwrap();
        let expected = OpenRemoteControlPairingRejection::AttemptTimeoutExceedsWindow {
            attempt_timeout: impossible.attempt_timeout,
            pairing_expires_after: impossible.expires_after,
        };
        let mut entropy_calls = 0usize;
        let mut settlement = None;
        engine.ingest_command_into(
            IssuedCommand {
                id: CommandId(53),
                command: PrnsCommand::OpenRemoteControlPairing(impossible),
            },
            AttachedInterfaces::new(&interfaces),
            InstantMillis(1_000),
            &mut |_| entropy_calls += 1,
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::CommandSettled {
                    settlement: observed,
                    ..
                }) = reaction
                {
                    settlement = Some(observed);
                }
            },
        );
        assert_eq!(
            settlement,
            Some(Settlement::OpenRemoteControlPairing(Err(
                OpenRemoteControlPairingFailure::Rejected(expected),
            ))),
        );
        assert_eq!(entropy_calls, 0);
        assert_eq!(
            engine.remote_control_pairing_view(),
            RemoteControlPairingView::Closed,
        );
    }

    #[test]
    fn identity_minting_is_bounded_when_entropy_repeats_an_existing_identity() {
        let interface = InterfaceId::new([0x94; 8]);
        let interfaces = [routable_descriptor(interface)];
        let mut engine = crate::engine::EngineState::<TestStorageLayout>::default();
        let existing = engine.hold_identity(fixed_secret_key()).unwrap();
        assert_eq!(
            configure_pairing(&mut engine),
            Ok(RemoteControlPairingAvailabilityDestination::canonical()),
        );
        let mut calls = 0usize;
        let mut settlement = None;
        let _ = engine.ingest_command_into(
            IssuedCommand {
                id: CommandId(61),
                command: PrnsCommand::OpenRemoteControlPairing(open(EgressTarget::AllInterfaces)),
            },
            AttachedInterfaces::new(&interfaces),
            InstantMillis(1_000),
            &mut |bytes| {
                calls += 1;
                let repeated = fixed_secret_key();
                bytes.copy_from_slice(&repeated[..]);
            },
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::CommandSettled {
                    settlement: observed,
                    ..
                }) = reaction
                {
                    settlement = Some(observed);
                }
            },
        );
        assert_eq!(calls, PAIRING_IDENTITY_MINT_ATTEMPTS);
        assert_eq!(
            settlement,
            Some(Settlement::OpenRemoteControlPairing(Err(
                OpenRemoteControlPairingFailure::IdentityGenerationExhausted,
            ))),
        );
        assert_eq!(engine.held_identity_hashes(), &[existing]);
        assert_eq!(
            engine.remote_control_pairing_view(),
            RemoteControlPairingView::Closed,
        );
    }

    #[test]
    fn registration_failure_releases_the_provisional_identity() {
        type OneDestination = TestFixedStorage<8, 8, 512, 1, 2, 16, 2, 2, 2, 2, 4, 2>;
        let interface = InterfaceId::new([0x95; 8]);
        let interfaces = [routable_descriptor(interface)];
        let mut engine = crate::engine::EngineState::<OneDestination>::default();
        assert_eq!(
            configure_pairing(&mut engine),
            Ok(RemoteControlPairingAvailabilityDestination::canonical()),
        );
        let mut settlement = None;
        let _ = engine.ingest_command_into(
            IssuedCommand {
                id: CommandId(71),
                command: PrnsCommand::OpenRemoteControlPairing(open(EgressTarget::AllInterfaces)),
            },
            AttachedInterfaces::new(&interfaces),
            InstantMillis(1_000),
            &mut fill_pairing_entropy,
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::CommandSettled {
                    settlement: observed,
                    ..
                }) = reaction
                {
                    settlement = Some(observed);
                }
            },
        );
        assert_eq!(
            settlement,
            Some(Settlement::OpenRemoteControlPairing(Err(
                OpenRemoteControlPairingFailure::RegisterEndpoint(
                    crate::routing::upstream_app_destinations::RegisterDestinationError::RegistryFull,
                ),
            ))),
        );
        assert_eq!(engine.held_identity_hashes(), &[stable_target_identity()]);
        assert_eq!(engine.upstream_app_destinations().count(), 1);
        assert_eq!(
            engine.remote_control_pairing_view(),
            RemoteControlPairingView::Closed,
        );
    }

    #[test]
    fn explicit_close_and_expiry_both_remove_the_endpoint_and_secret() {
        for close_at in [InstantMillis(2_000), InstantMillis(61_000)] {
            let interface = InterfaceId::new([0x96; 8]);
            let interfaces = [routable_descriptor(interface)];
            let mut engine = crate::engine::EngineState::<TestStorageLayout>::default();
            assert_eq!(
                configure_pairing(&mut engine),
                Ok(RemoteControlPairingAvailabilityDestination::canonical()),
            );
            let opened = engine
                .open_remote_control_pairing_into(
                    open(EgressTarget::AllInterfaces),
                    AttachedInterfaces::new(&interfaces),
                    InstantMillis(1_000),
                    &mut fill_pairing_entropy,
                    &mut |_| {},
                )
                .unwrap();
            if close_at == InstantMillis(2_000) {
                let mut settlement = None;
                let _ = engine.ingest_command_into(
                    IssuedCommand {
                        id: CommandId(81),
                        command: PrnsCommand::CloseRemoteControlPairing(CloseRemoteControlPairing),
                    },
                    AttachedInterfaces::new(&interfaces),
                    close_at,
                    &mut fill_pairing_entropy,
                    &mut |reaction| {
                        if let EngineReaction::Journaled(Journaled::CommandSettled {
                            id,
                            settlement: observed,
                        }) = reaction
                        {
                            settlement = Some((id, observed));
                        }
                    },
                );
                assert_eq!(
                    settlement,
                    Some((
                        CommandId(81),
                        Settlement::CloseRemoteControlPairing(Ok(
                            CloseRemoteControlPairingOutcome::Closed {
                                endpoint: opened.endpoint,
                            },
                        )),
                    )),
                );
            } else {
                let mut expired = None;
                let schedules = engine.fire_due_remote_control_pairing(
                    close_at,
                    AttachedInterfaces::new(&interfaces),
                    &mut fill_pairing_entropy,
                    &mut |reaction| {
                        if let EngineReaction::Journaled(Journaled::RemoteControlPairingExpired {
                            endpoint,
                        }) = reaction
                        {
                            expired = Some(endpoint);
                        }
                    },
                );
                assert_eq!(
                    schedules.remote_control_pairing,
                    crate::engine::WakeSchedule::Idle,
                );
                assert_eq!(expired, Some(opened.endpoint));
            }
            assert_eq!(
                engine.remote_control_pairing_view(),
                RemoteControlPairingView::Closed,
            );
            assert_eq!(engine.held_identity_hashes(), &[stable_target_identity()]);
            assert!(!engine.upstream_app_destinations().any(|registration| {
                registration.destination == opened.endpoint.destination_hash()
            }));
        }
    }

    #[test]
    fn automatic_expiry_surfaces_cleanup_failure_and_still_closes_when_endpoint_is_absent() {
        let interface = InterfaceId::new([0x97; 8]);
        let interfaces = [routable_descriptor(interface)];
        let mut engine = crate::engine::EngineState::<TestStorageLayout>::default();
        assert_eq!(
            configure_pairing(&mut engine),
            Ok(RemoteControlPairingAvailabilityDestination::canonical()),
        );
        let opened = engine
            .open_remote_control_pairing_into(
                open(EgressTarget::AllInterfaces),
                AttachedInterfaces::new(&interfaces),
                InstantMillis(1_000),
                &mut fill_pairing_entropy,
                &mut |_| {},
            )
            .unwrap();
        assert!(matches!(
            engine.unregister_destination(&opened.endpoint.destination_hash()),
            crate::engine::UnregisterDestinationOutcome::Unregistered { .. },
        ));
        let mut failure = None;
        let schedules = engine.fire_due_remote_control_pairing(
            InstantMillis(61_000),
            AttachedInterfaces::new(&interfaces),
            &mut fill_pairing_entropy,
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::RemoteControlPairingExpiryFailed {
                    endpoint,
                    failure: observed,
                }) = reaction
                {
                    failure = Some((endpoint, observed));
                }
            },
        );
        assert_eq!(
            failure,
            Some((
                opened.endpoint,
                CloseRemoteControlPairingFailure::EndpointNotRegistered,
            )),
        );
        assert_eq!(
            engine.remote_control_pairing_view(),
            RemoteControlPairingView::Closed,
        );
        assert_eq!(engine.held_identity_hashes(), &[stable_target_identity()]);
        assert_eq!(
            schedules.remote_control_pairing,
            crate::engine::WakeSchedule::Idle,
        );
    }
}
