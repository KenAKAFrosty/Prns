use tokio::sync::mpsc::{self, UnboundedReceiver};

use crate::engine::RequestResponseTimeout;
use crate::engine::Settlement;
use crate::reactor::compression;
use crate::reactor::driver::HostCommand;
use crate::routing::links::request::{parse_response_plaintext, RequestId};
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::runtime::request_router::RespondToken;
use crate::units::DurationMillis;
use crate::units::RttMillis;

use super::super::PrnsNodeHandle;
use super::RESPONSE_PACKET_CEILING;

fn handle() -> (PrnsNodeHandle, UnboundedReceiver<HostCommand>) {
    let (commands, command_rx) = mpsc::unbounded_channel();
    (PrnsNodeHandle::over(commands), command_rx)
}

#[tokio::test]
async fn request_emits_a_request_any_and_returns_the_response_with_its_rtt() {
    let (handle, mut command_rx) = handle();
    let link = LinkId::new([5; 16]);
    let path_hash = RequestPathHash::new([0x44; 16]);

    let requesting = tokio::spawn(async move { handle.request(link, path_hash, b"ping").await });

    let HostCommand::RequestAny(request) = command_rx.recv().await.unwrap() else {
        panic!("request issues a RequestAny host command");
    };
    assert_eq!(request.link_id, link);
    assert_eq!(request.path_hash, path_hash);
    assert_eq!(request.data.as_slice(), &b"ping"[..]);
    assert_eq!(
        request.response_timeout,
        RequestResponseTimeout::LinkDefault
    );
    request
        .completion
        .send(Ok((b"pong".to_vec(), RttMillis::new(42))))
        .unwrap();

    let (data, rtt) = requesting.await.unwrap().unwrap();
    assert_eq!(data, b"pong");
    assert_eq!(rtt, RttMillis::new(42));
}

#[tokio::test]
async fn request_preserves_an_explicit_response_timeout() {
    let (handle, mut command_rx) = handle();
    let link = LinkId::new([6; 16]);
    let path_hash = RequestPathHash::new([0x45; 16]);
    let timeout = RequestResponseTimeout::Exact(DurationMillis(45_000));

    let requesting = tokio::spawn(async move {
        handle
            .request_with_response_timeout(link, path_hash, b"slow", timeout)
            .await
    });
    let HostCommand::RequestAny(request) = command_rx.recv().await.unwrap() else {
        panic!("request issues a RequestAny host command");
    };
    assert_eq!(request.response_timeout, timeout);
    request
        .completion
        .send(Ok((b"done".to_vec(), RttMillis::new(42))))
        .unwrap();
    assert_eq!(requesting.await.unwrap().unwrap().0, b"done");
}

#[tokio::test]
async fn respond_returns_the_links_round_trip() {
    let (handle, _command_rx) = handle();
    let token = RespondToken {
        link_id: LinkId::new([1; 16]),
        request_id: RequestId([2; 16]),
        rtt: RttMillis::new(99),
    };
    assert_eq!(
        handle.respond(token, b"answer"),
        Some(RttMillis::new(99)),
        "respond surfaces the rtt the request arrived on",
    );
}

#[tokio::test]
async fn a_large_response_carries_a_bz2_candidate() {
    let (handle, mut command_rx) = handle();
    let token = RespondToken {
        link_id: LinkId::new([1; 16]),
        request_id: RequestId([2; 16]),
        rtt: RttMillis::new(50),
    };
    let body = std::vec![42u8; RESPONSE_PACKET_CEILING + 4096];
    assert_eq!(handle.respond(token, &body), Some(RttMillis::new(50)));
    let Some(HostCommand::RespondAny(respond)) = command_rx.recv().await else {
        panic!("expected a RespondAny command");
    };
    let (enclosed_request, enclosed_body) =
        parse_response_plaintext(respond.data.as_slice()).expect("stock RNS response envelope");
    assert_eq!(enclosed_request, token.request_id);
    assert_eq!(enclosed_body, body);
    assert_eq!(
        respond
            .compressed_candidate
            .as_ref()
            .map(|candidate| candidate.as_slice().to_vec()),
        compression::compress_if_smaller(respond.data.as_slice()),
        "a response past the packet ceiling rides a bz2 candidate matching the codec",
    );
    assert!(respond.compressed_candidate.is_some(), "a run compresses");
}

#[tokio::test]
async fn a_packet_sized_response_skips_compression() {
    let (handle, mut command_rx) = handle();
    let token = RespondToken {
        link_id: LinkId::new([1; 16]),
        request_id: RequestId([2; 16]),
        rtt: RttMillis::new(50),
    };
    let body = std::vec![42u8; RESPONSE_PACKET_CEILING];
    handle.respond(token, &body);
    let Some(HostCommand::RespondAny(respond)) = command_rx.recv().await else {
        panic!("expected a RespondAny command");
    };
    assert!(
        respond.compressed_candidate.is_none(),
        "a response that fits a packet never builds a candidate the rung would discard",
    );
}

#[tokio::test]
async fn a_settled_resource_response_waits_for_its_proof() {
    let (handle, mut command_rx) = handle();
    let token = RespondToken {
        link_id: LinkId::new([7; 16]),
        request_id: RequestId([8; 16]),
        rtt: RttMillis::new(33),
    };
    let body = std::vec![0xA5u8; RESPONSE_PACKET_CEILING + 1024];
    let responding = tokio::spawn(async move { handle.respond_owned_settled(token, body).await });

    let Some(HostCommand::RespondAny(mut response)) = command_rx.recv().await else {
        panic!("a resource response reaches the host driver");
    };
    assert!(
        !responding.is_finished(),
        "the route remains occupied until Resource proof settlement"
    );
    response
        .completion
        .take()
        .expect("settled response carries completion")
        .send(Settlement::SendResource(Ok(())))
        .expect("route awaits completion");
    assert_eq!(responding.await.unwrap().unwrap(), RttMillis::new(33));
}
