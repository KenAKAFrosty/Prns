use tokio::sync::mpsc::{self, UnboundedReceiver};

use crate::reactor::compression;
use crate::reactor::driver::HostCommand;
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::runtime::request_router::RespondToken;
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
    request
        .completion
        .send(Ok((b"pong".to_vec(), RttMillis::new(42))))
        .unwrap();

    let (data, rtt) = requesting.await.unwrap().unwrap();
    assert_eq!(data, b"pong");
    assert_eq!(rtt, RttMillis::new(42));
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
    assert_eq!(
        respond
            .compressed_candidate
            .as_ref()
            .map(|candidate| candidate.as_slice().to_vec()),
        compression::compress_if_smaller(&body),
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
