use embassy_futures::{block_on, join::join};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;

use crate::engine::{
    DeliveryEvidence, IssuedCommand, Journaled, PacketReceiptDelivered, PrnsCommand, Settlement,
};
use crate::remote_control::REMOTE_CONTROL_REQUEST_ENDPOINT_ID;
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::runtime::request_endpoints::RequestEndpointId;
use crate::runtime::{RemoteControlAnnounceSelf, RemoteControlDescribe};
use crate::units::RttMillis;
use prns_core::remote_control::{
    RemoteControlAnnounceSelfOutcome, RemoteControlDescription, RemoteControlProtocolVersion,
    RemoteControlRequestSet, RemoteControlResponse,
};

use super::super::command_handle::JournalRoute;
use super::super::{CompletionPool, PrnsNodeHandle};

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
                handle.route_journaled(&response_event),
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
                handle.route_journaled(&settled),
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
            handle.route_journaled(&response_event),
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
            handle.route_journaled(&settled),
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
