use embassy_futures::{block_on, join::join};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;

use crate::engine::{
    BeginRemoteControlControllerPairing, CloseLink, DeliveryEvidence, EngineReaction, EngineState,
    EstablishLink, Identify, IdentifyFailure, IssuedCommand, Journaled, LinkEstablished,
    PacketReceiptDelivered, PrnsCommand, Settlement,
};
use crate::interfaces::AttachedInterfaces;
use crate::remote_control::REMOTE_CONTROL_REQUEST_ENDPOINT_ID;
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::runtime::request_endpoints::RequestEndpointId;
use crate::runtime::{
    configure_remote_control_service, BeginRemoteControlControllerPairingControlFailure,
    InitiateRemoteControlControllerPairing, InitiateRemoteControlControllerPairingError,
    RemoteControlAnnounceSelf, RemoteControlControllerPairingInitiationControl,
    RemoteControlDescribe, RemoteControlPairingControl, RemoteControlPairingControlError,
    RemoteControlPairingLinkCleanupOutcome, RemoteControlTargetOperationError,
    ResolvedRemoteControlTarget, SendError,
};
use crate::storage::GrowableHeap;
use crate::units::{InstantMillis, RttMillis};
use prns_core::remote_control::{
    RemoteControlAnnounceSelfOutcome, RemoteControlDescription, RemoteControlPairingContext,
    RemoteControlPairingIdentity, RemoteControlPairingInvitationCode, RemoteControlProtocolVersion,
    RemoteControlRequestKind, RemoteControlRequestSet, RemoteControlResponse,
    RemoteControlTargetAccess, RemoteControlTargetIdentity,
};

use super::super::command_handle::JournalRoute;
use super::super::{CompletionPool, PrnsNodeHandle};
use crate::runtime::remote_control_target_accesses::{
    RemoteControlTargetAccessCommand, RemoteControlTargetAccessCompletion,
};

type M = CriticalSectionRawMutex;
const RESPONSE_BYTES: usize = RemoteControlDescribe::RESPONSE_CAPACITY;
const CONTROLLER_PAIRING_LINK_ID: LinkId = LinkId::new([0x61; 16]);
const CONTROLLER_PAIRING_NOW: InstantMillis = InstantMillis(1_000);

fn encoded_response(response: &RemoteControlResponse) -> heapless::Vec<u8, RESPONSE_BYTES> {
    let mut encoded = heapless::Vec::new();
    encoded.resize_default(response.encoded_len()).unwrap();
    assert_eq!(
        response.write_into(encoded.as_mut_slice()),
        Ok(encoded.len()),
    );
    encoded
}

fn resolved_target() -> (
    crate::identity::IdentityHash,
    crate::wire::DestinationHash,
    crate::identity::IdentityHash,
    ResolvedRemoteControlTarget,
) {
    let service = crate::runtime::node_facade::test_remote_control_service();
    let identities = service
        .configuration()
        .unwrap()
        .identity_secrets()
        .identities();
    let access = RemoteControlTargetAccess::new(
        RemoteControlTargetIdentity::new(*identities.target().public_keys()),
        RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
    )
    .unwrap();
    (
        access.target().identity_hash(),
        access.endpoint().destination_hash(),
        identities.controller().identity_hash(),
        ResolvedRemoteControlTarget::from((identities.controller(), &access)),
    )
}

fn controller_pairing_begin() -> BeginRemoteControlControllerPairing {
    BeginRemoteControlControllerPairing {
        context: RemoteControlPairingContext::new(
            RemoteControlPairingIdentity::new(crate::identity::IdentityHash::new([0x71; 16]))
                .endpoint(),
            CONTROLLER_PAIRING_LINK_ID,
        ),
        invitation_code: RemoteControlPairingInvitationCode::from_value(0x1234_ABCD),
        pairing_expires_at: InstantMillis(10_000),
    }
}

fn controller_pairing_engine() -> (EngineState<GrowableHeap>, crate::identity::IdentityHash) {
    let service = crate::runtime::node_facade::test_remote_control_service();
    let controller = service
        .configuration()
        .unwrap()
        .identity_secrets()
        .identities()
        .controller()
        .identity_hash();
    let mut engine = EngineState::default();
    configure_remote_control_service(&mut engine, service).unwrap();
    (engine, controller)
}

fn settle_pairing_begin(
    engine: &mut EngineState<GrowableHeap>,
    issued: IssuedCommand,
) -> Settlement {
    let mut settlement = None;
    engine.ingest_command_into(
        issued,
        AttachedInterfaces::new(&[]),
        CONTROLLER_PAIRING_NOW,
        &mut |_| panic!("controller pairing begin needs no entropy"),
        &mut |reaction| match reaction {
            EngineReaction::Journaled(Journaled::CommandSettled {
                settlement: settled,
                ..
            }) => settlement = Some(settled),
            EngineReaction::Journaled(_) | EngineReaction::Directive(_) => {}
        },
    );
    settlement.unwrap()
}

#[test]
fn controller_pairing_initiation_establishes_the_observed_endpoint_before_beginning() {
    let commands = Channel::<M, IssuedCommand, 1>::new();
    let completions = CompletionPool::<M, 1>::new();
    let handle = PrnsNodeHandle::new(commands.sender(), &completions);
    let (mut engine, controller) = controller_pairing_engine();
    let begin = controller_pairing_begin();
    let endpoint = begin.context.endpoint();
    let destination = endpoint.destination_hash();

    let (result, ()) = block_on(join(
        handle.initiate_remote_control_controller_pairing(InitiateRemoteControlControllerPairing {
            endpoint,
            invitation_code: begin.invitation_code,
            expires_at: begin.pairing_expires_at,
        }),
        async {
            let issued = commands.receiver().receive().await;
            assert_eq!(
                issued.command,
                PrnsCommand::EstablishLink(EstablishLink { destination }),
            );
            assert!(matches!(
                handle.route_journaled(
                    Journaled::CommandSettled {
                        id: issued.id,
                        settlement: Settlement::EstablishLink(Ok(LinkEstablished {
                            link_id: CONTROLLER_PAIRING_LINK_ID,
                            rtt_millis: 17,
                        })),
                    },
                    |_| {},
                ),
                JournalRoute::Awaiter,
            ));

            let issued = commands.receiver().receive().await;
            assert!(matches!(
                issued.command,
                PrnsCommand::BeginRemoteControlControllerPairing(_),
            ));
            assert!(matches!(
                handle.route_journaled(
                    Journaled::CommandSettled {
                        id: issued.id,
                        settlement: settle_pairing_begin(&mut engine, issued),
                    },
                    |_| {},
                ),
                JournalRoute::Awaiter,
            ));

            let issued = commands.receiver().receive().await;
            assert_eq!(
                issued.command,
                PrnsCommand::Identify(Identify {
                    link_id: CONTROLLER_PAIRING_LINK_ID,
                    identity: controller,
                }),
            );
            assert!(matches!(
                handle.route_journaled(
                    Journaled::CommandSettled {
                        id: issued.id,
                        settlement: Settlement::Identify(Ok(())),
                    },
                    |_| {},
                ),
                JournalRoute::Awaiter,
            ));

            let issued = commands.receiver().receive().await;
            assert!(matches!(
                issued.command,
                PrnsCommand::RemoteControlControllerPairingRequest(_),
            ));
            assert!(matches!(
                handle.route_journaled(
                    Journaled::CommandSettled {
                        id: issued.id,
                        settlement: Settlement::Identify(Ok(())),
                    },
                    |_| {},
                ),
                JournalRoute::Awaiter,
            ));

            let issued = commands.receiver().receive().await;
            assert_eq!(
                issued.command,
                PrnsCommand::CloseLink(CloseLink {
                    link_id: CONTROLLER_PAIRING_LINK_ID,
                }),
            );
        },
    ));

    assert_eq!(
        result,
        Err(InitiateRemoteControlControllerPairingError::NodeStopped {
            cleanup: RemoteControlPairingLinkCleanupOutcome::Queued,
        }),
    );
}

#[test]
fn controller_pairing_begin_identifies_before_sending_its_request() {
    let commands = Channel::<M, IssuedCommand, 1>::new();
    let completions = CompletionPool::<M, 1>::new();
    let handle = PrnsNodeHandle::new(commands.sender(), &completions);
    let (mut engine, controller) = controller_pairing_engine();

    let (result, ()) = block_on(join(
        handle.begin_remote_control_controller_pairing(controller_pairing_begin()),
        async {
            let issued = commands.receiver().receive().await;
            assert!(matches!(
                issued.command,
                PrnsCommand::BeginRemoteControlControllerPairing(_),
            ));
            assert!(matches!(
                handle.route_journaled(
                    Journaled::CommandSettled {
                        id: issued.id,
                        settlement: settle_pairing_begin(&mut engine, issued),
                    },
                    |_| {},
                ),
                JournalRoute::Awaiter,
            ));

            let issued = commands.receiver().receive().await;
            assert_eq!(
                issued.command,
                PrnsCommand::Identify(Identify {
                    link_id: CONTROLLER_PAIRING_LINK_ID,
                    identity: controller,
                }),
            );
            assert!(matches!(
                handle.route_journaled(
                    Journaled::CommandSettled {
                        id: issued.id,
                        settlement: Settlement::Identify(Ok(())),
                    },
                    |_| {},
                ),
                JournalRoute::Awaiter,
            ));

            let issued = commands.receiver().receive().await;
            assert!(matches!(
                issued.command,
                PrnsCommand::RemoteControlControllerPairingRequest(_),
            ));
            assert!(matches!(
                handle.route_journaled(
                    Journaled::CommandSettled {
                        id: issued.id,
                        settlement: Settlement::Identify(Ok(())),
                    },
                    |_| {},
                ),
                JournalRoute::Awaiter,
            ));
        },
    ));

    assert_eq!(result, Err(RemoteControlPairingControlError::NodeStopped));
}

#[test]
fn controller_pairing_identification_failure_closes_the_link_and_preserves_the_cause() {
    let commands = Channel::<M, IssuedCommand, 1>::new();
    let completions = CompletionPool::<M, 1>::new();
    let handle = PrnsNodeHandle::new(commands.sender(), &completions);
    let (mut engine, _) = controller_pairing_engine();

    let (result, ()) = block_on(join(
        handle.begin_remote_control_controller_pairing(controller_pairing_begin()),
        async {
            let issued = commands.receiver().receive().await;
            assert!(matches!(
                handle.route_journaled(
                    Journaled::CommandSettled {
                        id: issued.id,
                        settlement: settle_pairing_begin(&mut engine, issued),
                    },
                    |_| {},
                ),
                JournalRoute::Awaiter,
            ));

            let issued = commands.receiver().receive().await;
            assert!(matches!(
                handle.route_journaled(
                    Journaled::CommandSettled {
                        id: issued.id,
                        settlement: Settlement::Identify(Err(IdentifyFailure::WriteFailed)),
                    },
                    |_| {},
                ),
                JournalRoute::Awaiter,
            ));

            let issued = commands.receiver().receive().await;
            assert_eq!(
                issued.command,
                PrnsCommand::CloseLink(CloseLink {
                    link_id: CONTROLLER_PAIRING_LINK_ID,
                }),
            );
        },
    ));

    assert_eq!(
        result,
        Err(RemoteControlPairingControlError::Failed(
            BeginRemoteControlControllerPairingControlFailure::Identify {
                failure: SendError::Failed(IdentifyFailure::WriteFailed),
                cleanup: RemoteControlPairingLinkCleanupOutcome::Queued,
            },
        )),
    );
}

#[test]
fn connection_resolves_links_identifies_and_refuses_unpermitted_egress() {
    let commands = Channel::<M, IssuedCommand, 1>::new();
    let completions = CompletionPool::<M, 1, 1, RESPONSE_BYTES>::new();
    let handle = PrnsNodeHandle::new(commands.sender(), &completions);
    let (target, destination, controller, resolved) = resolved_target();
    let link_id = LinkId::new([0x31; 16]);

    let (connected, ()) = block_on(join(handle.connect_remote_control_target(target), async {
        let RemoteControlTargetAccessCommand::ResolveTarget {
            id,
            target: submitted,
        } = handle.next_remote_control_target_access_command().await
        else {
            panic!("resolve target command")
        };
        assert_eq!(submitted, target);
        assert!(handle.settle_remote_control_target_access(
            id,
            RemoteControlTargetAccessCompletion::Resolved(Ok(resolved)),
        ));

        let issued = commands.receiver().receive().await;
        assert_eq!(
            issued.command,
            PrnsCommand::EstablishLink(EstablishLink { destination }),
        );
        assert!(matches!(
            handle.route_journaled(
                Journaled::CommandSettled {
                    id: issued.id,
                    settlement: Settlement::EstablishLink(Ok(LinkEstablished {
                        link_id,
                        rtt_millis: 17,
                    })),
                },
                |_| {},
            ),
            JournalRoute::Awaiter,
        ));

        let issued = commands.receiver().receive().await;
        assert_eq!(
            issued.command,
            PrnsCommand::Identify(Identify {
                link_id,
                identity: controller,
            }),
        );
        assert!(matches!(
            handle.route_journaled(
                Journaled::CommandSettled {
                    id: issued.id,
                    settlement: Settlement::Identify(Ok(())),
                },
                |_| {},
            ),
            JournalRoute::Awaiter,
        ));
    }));
    let connected = connected.unwrap();

    assert_eq!(connected.connection().target(), target);
    assert_eq!(connected.connection().link_id(), link_id);
    assert_eq!(
        block_on(connected.announce_self()),
        Err(RemoteControlTargetOperationError::NotPermitted(
            RemoteControlRequestKind::AnnounceSelf,
        )),
    );
    assert!(commands.try_receive().is_err());
}

#[test]
fn identification_failure_queues_link_cleanup_and_preserves_the_failure() {
    let commands = Channel::<M, IssuedCommand, 1>::new();
    let completions = CompletionPool::<M, 1, 1, RESPONSE_BYTES>::new();
    let handle = PrnsNodeHandle::new(commands.sender(), &completions);
    let (target, destination, controller, resolved) = resolved_target();
    let link_id = LinkId::new([0x41; 16]);

    let (connected, ()) = block_on(join(handle.connect_remote_control_target(target), async {
        let RemoteControlTargetAccessCommand::ResolveTarget { id, .. } =
            handle.next_remote_control_target_access_command().await
        else {
            panic!("resolve target command")
        };
        assert!(handle.settle_remote_control_target_access(
            id,
            RemoteControlTargetAccessCompletion::Resolved(Ok(resolved)),
        ));

        let issued = commands.receiver().receive().await;
        assert_eq!(
            issued.command,
            PrnsCommand::EstablishLink(EstablishLink { destination }),
        );
        assert!(matches!(
            handle.route_journaled(
                Journaled::CommandSettled {
                    id: issued.id,
                    settlement: Settlement::EstablishLink(Ok(LinkEstablished {
                        link_id,
                        rtt_millis: 18,
                    })),
                },
                |_| {},
            ),
            JournalRoute::Awaiter,
        ));

        let issued = commands.receiver().receive().await;
        assert_eq!(
            issued.command,
            PrnsCommand::Identify(Identify {
                link_id,
                identity: controller,
            }),
        );
        assert!(matches!(
            handle.route_journaled(
                Journaled::CommandSettled {
                    id: issued.id,
                    settlement: Settlement::Identify(Err(IdentifyFailure::WriteFailed)),
                },
                |_| {},
            ),
            JournalRoute::Awaiter,
        ));

        let issued = commands.receiver().receive().await;
        assert_eq!(
            issued.command,
            PrnsCommand::CloseLink(CloseLink { link_id }),
        );
    }));

    assert_eq!(
        connected.err(),
        Some(crate::runtime::ConnectRemoteControlTargetError::Identify(
            crate::runtime::SendError::Failed(IdentifyFailure::WriteFailed),
        )),
    );
}

#[test]
fn announce_self_uses_the_bounded_embassy_request_lane() {
    let commands = Channel::<M, IssuedCommand, 1>::new();
    let completions = CompletionPool::<M, 0, 1, RESPONSE_BYTES>::new();
    let handle = PrnsNodeHandle::new(commands.sender(), &completions);
    let link_id = LinkId::new([0x20; 16]);

    let (result, ()) = block_on(join(
        handle.remote_control(link_id).announce_self(),
        async {
            let issued = commands.receiver().receive().await;
            let PrnsCommand::SendRequest(request) = issued.command else {
                panic!("announce request command")
            };
            assert_eq!(request.link_id, link_id);
            assert_eq!(
                request.path_hash,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
            );
            assert_eq!(
                request.data.as_slice(),
                &[
                    RemoteControlProtocolVersion::V1.wire_value(),
                    RemoteControlAnnounceSelf::REQUEST.kind().wire_value(),
                ],
            );
            assert_eq!(
                request.maximum_response_bytes,
                RemoteControlAnnounceSelf::MAXIMUM_RESPONSE_BYTES,
            );

            let response = encoded_response(&RemoteControlResponse::AnnounceSelf(
                RemoteControlAnnounceSelfOutcome::Announced,
            ));
            let response_event = Journaled::ResponseReceived {
                command_id: issued.id,
                link_id,
                request_id: RequestId([0x42; 16]),
                data: response.as_slice(),
            };
            assert!(matches!(
                handle.route_journaled(response_event, |_| {}),
                JournalRoute::Awaiter,
            ));
            let settled = Journaled::CommandSettled {
                id: issued.id,
                settlement: Settlement::SendRequest(Ok(PacketReceiptDelivered {
                    rtt: RttMillis::new(36),
                    evidence: DeliveryEvidence::Response,
                })),
            };
            assert!(matches!(
                handle.route_journaled(settled, |_| {}),
                JournalRoute::Awaiter,
            ));
        },
    ));

    assert!(matches!(result, Ok(rtt) if rtt == RttMillis::new(36)));
}

#[test]
fn describe_uses_the_bounded_embassy_request_lane() {
    let commands = Channel::<M, IssuedCommand, 1>::new();
    let completions = CompletionPool::<M, 0, 1, RESPONSE_BYTES>::new();
    let handle = PrnsNodeHandle::new(commands.sender(), &completions);
    let link_id = LinkId::new([0x21; 16]);

    let (result, ()) = block_on(join(handle.remote_control(link_id).describe(), async {
        let issued = commands.receiver().receive().await;
        let PrnsCommand::SendRequest(request) = issued.command else {
            panic!("describe request command")
        };
        assert_eq!(request.link_id, link_id);
        assert_eq!(
            request.path_hash,
            RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
        );
        assert_eq!(
            request.data.as_slice(),
            &[
                RemoteControlProtocolVersion::V1.wire_value(),
                RemoteControlDescribe::REQUEST.kind().wire_value(),
            ],
        );
        assert_eq!(
            request.maximum_response_bytes,
            RemoteControlDescribe::MAXIMUM_RESPONSE_BYTES,
        );

        let description =
            RemoteControlDescription::try_from(RemoteControlRequestSet::all()).unwrap();
        let response = encoded_response(&RemoteControlResponse::Describe(description));
        let response_event = Journaled::ResponseReceived {
            command_id: issued.id,
            link_id,
            request_id: RequestId([0x43; 16]),
            data: response.as_slice(),
        };
        assert!(matches!(
            handle.route_journaled(response_event, |_| {}),
            JournalRoute::Awaiter,
        ));
        let settled = Journaled::CommandSettled {
            id: issued.id,
            settlement: Settlement::SendRequest(Ok(PacketReceiptDelivered {
                rtt: RttMillis::new(37),
                evidence: DeliveryEvidence::Response,
            })),
        };
        assert!(matches!(
            handle.route_journaled(settled, |_| {}),
            JournalRoute::Awaiter,
        ));
    }));

    let Ok((description, rtt)) = result else {
        panic!("typed description")
    };
    assert_eq!(
        description.available_requests(),
        &RemoteControlRequestSet::all(),
    );
    assert_eq!(rtt, RttMillis::new(37));
}
