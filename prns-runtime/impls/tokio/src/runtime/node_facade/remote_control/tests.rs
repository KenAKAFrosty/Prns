use tokio::sync::mpsc::{self, UnboundedReceiver};

use crate::engine::SendRequestFailure;
use crate::manifold::driver::HostCommand;
use crate::routing::links::LinkId;
use crate::runtime::request_endpoints::RequestEndpointId;
use crate::runtime::{SendError, REMOTE_CONTROL_ENDPOINT_ID};
use crate::units::RttMillis;
use prns_core::remote_control::{
    RemoteControlAnnounceOutcome, RemoteControlDescription, RemoteControlProtocolError,
    RemoteControlProtocolVersion, RemoteControlRequestKind, RemoteControlResponse,
    RemoteControlResponseKind, RemoteControlResponseParseError,
};

use super::super::PrnsNodeHandle;
use super::RemoteControlError;

fn test_handle() -> (PrnsNodeHandle, UnboundedReceiver<HostCommand>) {
    let (commands, command_rx) = mpsc::unbounded_channel();
    (PrnsNodeHandle::over(commands), command_rx)
}

fn encoded_response(response: &RemoteControlResponse) -> std::vec::Vec<u8> {
    let encoded_len = response.encoded_len();
    let mut encoded = std::vec![0u8; encoded_len];
    assert_eq!(response.write_into(encoded.as_mut_slice()), Ok(encoded_len));
    encoded
}

#[tokio::test]
async fn announce_owns_the_remote_control_exchange_and_returns_its_rtt() {
    let (handle, mut command_rx) = test_handle();
    let link_id = LinkId::new([0x20; 16]);
    let requesting = tokio::spawn(async move { handle.remote_control(link_id).announce().await });

    let Some(HostCommand::RequestAny(request)) = command_rx.recv().await else {
        panic!("announce issues a request command");
    };
    assert_eq!(request.link_id, link_id);
    assert_eq!(
        request.path_hash,
        RequestEndpointId::of(REMOTE_CONTROL_ENDPOINT_ID),
    );
    assert_eq!(
        request.data.as_slice(),
        &[
            RemoteControlProtocolVersion::V1.wire_value(),
            crate::runtime::RemoteControlAnnounce::REQUEST
                .kind()
                .wire_value(),
        ],
    );
    assert_eq!(
        request.maximum_response_bytes,
        crate::runtime::RemoteControlAnnounce::MAXIMUM_RESPONSE_BYTES,
    );
    let response = RemoteControlResponse::Announce(RemoteControlAnnounceOutcome::Announced);
    assert!(request
        .completion
        .send(Ok((encoded_response(&response), RttMillis::new(36))))
        .is_ok());

    assert!(matches!(requesting.await, Ok(Ok(rtt)) if rtt == RttMillis::new(36)));
}

#[tokio::test]
async fn describe_owns_the_remote_control_exchange_and_returns_the_typed_description() {
    let (handle, mut command_rx) = test_handle();
    let link_id = LinkId::new([0x21; 16]);
    let requesting = tokio::spawn(async move { handle.remote_control(link_id).describe().await });

    let Some(HostCommand::RequestAny(request)) = command_rx.recv().await else {
        panic!("describe issues a request command");
    };
    assert_eq!(request.link_id, link_id);
    assert_eq!(
        request.path_hash,
        RequestEndpointId::of(REMOTE_CONTROL_ENDPOINT_ID),
    );
    assert_eq!(
        request.data.as_slice(),
        &[
            RemoteControlProtocolVersion::V1.wire_value(),
            crate::runtime::RemoteControlDescribe::REQUEST
                .kind()
                .wire_value(),
        ],
    );
    assert_eq!(
        request.maximum_response_bytes,
        crate::runtime::RemoteControlDescribe::MAXIMUM_RESPONSE_BYTES,
    );
    let response = RemoteControlResponse::Describe(RemoteControlDescription::default());
    assert!(request
        .completion
        .send(Ok((encoded_response(&response), RttMillis::new(37),)))
        .is_ok());

    let Ok(Ok((description, rtt))) = requesting.await else {
        panic!("describe returns the typed response");
    };
    assert!(description
        .supported_requests()
        .supports(RemoteControlRequestKind::Describe));
    assert_eq!(rtt, RttMillis::new(37));
}

#[tokio::test]
async fn describe_preserves_remote_protocol_errors() {
    let (handle, mut command_rx) = test_handle();
    let link_id = LinkId::new([0x43; 16]);
    let requesting = tokio::spawn(async move { handle.remote_control(link_id).describe().await });
    let Some(HostCommand::RequestAny(request)) = command_rx.recv().await else {
        panic!("describe issues a request command");
    };
    let error = RemoteControlProtocolError::UnknownRequestKind { found: 0xA5 };
    let response = RemoteControlResponse::ProtocolError(error);
    assert!(request
        .completion
        .send(Ok((encoded_response(&response), RttMillis::new(38))))
        .is_ok());

    assert!(matches!(
        requesting.await,
        Ok(Err(RemoteControlError::Remote(found))) if found == error
    ));
}

#[tokio::test]
async fn describe_preserves_transport_and_response_failures() {
    let (handle, mut command_rx) = test_handle();
    let link_id = LinkId::new([0x65; 16]);
    let requesting = tokio::spawn(async move { handle.remote_control(link_id).describe().await });
    let Some(HostCommand::RequestAny(request)) = command_rx.recv().await else {
        panic!("describe issues a request command");
    };
    assert!(request
        .completion
        .send(Err(SendRequestFailure::Timeout))
        .is_ok());
    assert!(matches!(
        requesting.await,
        Ok(Err(RemoteControlError::Request(SendError::Failed(
            SendRequestFailure::Timeout
        ))))
    ));

    let (handle, mut command_rx) = test_handle();
    let requesting = tokio::spawn(async move { handle.remote_control(link_id).describe().await });
    let Some(HostCommand::RequestAny(request)) = command_rx.recv().await else {
        panic!("describe issues a request command");
    };
    let truncated_response = std::vec![
        RemoteControlProtocolVersion::V1.wire_value(),
        RemoteControlResponseKind::Describe.wire_value(),
    ];
    assert!(request
        .completion
        .send(Ok((truncated_response, RttMillis::new(39))))
        .is_ok());
    assert!(matches!(
        requesting.await,
        Ok(Err(RemoteControlError::Response(
            RemoteControlResponseParseError::Truncated
        )))
    ));
}
