use embassy_futures::{block_on, join::join};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;

use crate::engine::{
    CloseLink, DeliveryEvidence, EstablishLink, Identify, IdentifyFailure, IssuedCommand,
    Journaled, LinkEstablished, PacketReceiptDelivered, PrnsCommand, Settlement,
};
use crate::remote_control::REMOTE_CONTROL_REQUEST_ENDPOINT_ID;
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::runtime::request_endpoints::RequestEndpointId;
use crate::runtime::{RemoteControlAnnounceSelf, RemoteControlDescribe};
use crate::runtime::{RemoteControlTargetOperationError, ResolvedRemoteControlTarget};
use crate::units::RttMillis;
use prns_core::remote_control::{
    RemoteControlAnnounceSelfOutcome, RemoteControlDescription, RemoteControlProtocolVersion,
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
