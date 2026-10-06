use tokio::sync::mpsc::{self, UnboundedReceiver};

use crate::engine::SendRequestFailure;
use crate::manifold::driver::HostCommand;
use crate::remote_control::REMOTE_CONTROL_REQUEST_ENDPOINT_ID;
use crate::routing::links::LinkId;
use crate::runtime::request_endpoints::RequestEndpointId;
use crate::runtime::SendError;
use crate::units::RttMillis;
use prns_core::remote_control::{
    RemoteControlAnnounceSelfOutcome, RemoteControlDescription, RemoteControlProtocolError,
    RemoteControlProtocolVersion, RemoteControlRequestSet, RemoteControlResponse,
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
async fn announce_self_owns_the_remote_control_exchange_and_returns_its_rtt() {
    let (handle, mut command_rx) = test_handle();
    let link_id = LinkId::new([0x20; 16]);
    let requesting =
        tokio::spawn(async move { handle.remote_control(link_id).announce_self().await });

    let Some(HostCommand::RequestAny(request)) = command_rx.recv().await else {
        panic!("announce issues a request command");
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
            crate::runtime::RemoteControlAnnounceSelf::REQUEST
                .kind()
                .wire_value(),
        ],
    );
    assert_eq!(
        request.maximum_response_bytes,
        crate::runtime::RemoteControlAnnounceSelf::MAXIMUM_RESPONSE_BYTES,
    );
    let response = RemoteControlResponse::AnnounceSelf(RemoteControlAnnounceSelfOutcome::Announced);
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
        RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
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
    let description = RemoteControlDescription::try_from(RemoteControlRequestSet::all()).unwrap();
    let response = RemoteControlResponse::Describe(description);
    assert!(request
        .completion
        .send(Ok((encoded_response(&response), RttMillis::new(37),)))
        .is_ok());

    let Ok(Ok((description, rtt))) = requesting.await else {
        panic!("describe returns the typed response");
    };
    assert_eq!(
        description.available_requests(),
        &RemoteControlRequestSet::all(),
    );
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

#[tokio::test]
async fn watch_interfaces_registers_its_reader_before_the_authenticated_request() {
    let (handle, mut command_rx) = test_handle();
    let link_id = LinkId::new([0x74; 16]);
    let stream_id = crate::runtime::StreamId::new(0x123).unwrap();
    let requesting = tokio::spawn(async move {
        handle
            .remote_control(link_id)
            .watch_interfaces(stream_id)
            .await
    });
    let Some(HostCommand::RegisterStreamReader {
        link_id: registered_link,
        stream_id: registered_stream,
        ready,
        ..
    }) = command_rx.recv().await
    else {
        panic!("watch registers its stream reader first");
    };
    assert_eq!(registered_link, link_id);
    assert_eq!(registered_stream, stream_id);
    assert!(command_rx.try_recv().is_err());
    assert!(ready.send(Ok(())).is_ok());

    let Some(HostCommand::RequestAny(request)) = command_rx.recv().await else {
        panic!("watch then sends its authenticated request");
    };
    assert_eq!(request.link_id, link_id);
    assert_eq!(request.data.as_slice(), &[1, 0x20, 0x01, 0x23]);
    assert_eq!(
        request.maximum_response_bytes,
        crate::runtime::RemoteControlWatchInterfaces::MAXIMUM_RESPONSE_BYTES,
    );
    assert!(request
        .completion
        .send(Ok((
            encoded_response(&RemoteControlResponse::WatchInterfaces { stream_id }),
            RttMillis::new(40),
        )))
        .is_ok());
    let (reader, rtt) = requesting.await.unwrap().unwrap();
    assert_eq!(rtt, RttMillis::new(40));
    drop(reader);
}

#[tokio::test]
async fn duplicate_watch_reader_fails_locally_before_sending_a_request() {
    let (handle, mut command_rx) = test_handle();
    let requesting = tokio::spawn(async move {
        handle
            .remote_control(LinkId::new([0x74; 16]))
            .watch_interfaces(crate::runtime::StreamId::new(0x123).unwrap())
            .await
    });
    let Some(HostCommand::RegisterStreamReader { ready, .. }) = command_rx.recv().await else {
        panic!("watch registers its reader first");
    };
    ready
        .send(Err(
            crate::runtime::StreamReaderRegistrationError::AlreadyRegistered,
        ))
        .unwrap();
    assert!(matches!(
        requesting.await.unwrap(),
        Err(super::RemoteControlWatchOpenError::Registration(
            crate::runtime::StreamReaderRegistrationError::AlreadyRegistered
        ))
    ));
    assert!(matches!(
        command_rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected)
    ));
}

#[tokio::test]
async fn radio_exchanges_preserve_configuration_status_outcomes_and_response_limits() {
    use prns_core::interfaces::InterfaceId;
    use prns_core::remote_control::{
        RemoteControlRadioConfiguration, RemoteControlRadioOutcome, RemoteControlRadioStatus,
        RemoteControlRequest,
    };
    let id = InterfaceId::new(*b"radio-id");
    let link = LinkId::new([0x76; 16]);
    for outcome in RemoteControlRadioOutcome::ALL {
        let (handle, mut commands) = test_handle();
        let requesting = tokio::spawn(async move {
            handle
                .remote_control(link)
                .configure_radio(id, RemoteControlRadioConfiguration::Unconfigured)
                .await
        });
        let Some(HostCommand::RequestAny(request)) = commands.recv().await else {
            panic!("expected configuration request")
        };
        assert_eq!(
            RemoteControlRequest::parse(request.data.as_slice()),
            Ok(RemoteControlRequest::ConfigureRadio {
                id,
                configuration: RemoteControlRadioConfiguration::Unconfigured
            })
        );
        assert_eq!(
            request.maximum_response_bytes,
            crate::runtime::RemoteControlConfigureRadio::MAXIMUM_RESPONSE_BYTES
        );
        assert!(request
            .completion
            .send(Ok((
                encoded_response(&RemoteControlResponse::ConfigureRadio(outcome)),
                RttMillis::new(12)
            )))
            .is_ok());
        assert!(
            matches!(requesting.await, Ok(Ok((found, rtt))) if found == outcome && rtt == RttMillis::new(12))
        );
    }
    let (handle, mut commands) = test_handle();
    let requesting =
        tokio::spawn(async move { handle.remote_control(link).inspect_radio(id).await });
    let Some(HostCommand::RequestAny(request)) = commands.recv().await else {
        panic!("expected inspection request")
    };
    assert_eq!(
        RemoteControlRequest::parse(request.data.as_slice()),
        Ok(RemoteControlRequest::InspectRadio { id })
    );
    assert_eq!(
        request.maximum_response_bytes,
        crate::runtime::RemoteControlInspectRadio::MAXIMUM_RESPONSE_BYTES
    );
    assert!(request
        .completion
        .send(Ok((
            encoded_response(&RemoteControlResponse::InspectRadio(
                RemoteControlRadioStatus::UnknownInterface
            )),
            RttMillis::new(13)
        )))
        .is_ok());
    assert!(
        matches!(requesting.await, Ok(Ok((RemoteControlRadioStatus::UnknownInterface, rtt))) if rtt == RttMillis::new(13))
    );
}
