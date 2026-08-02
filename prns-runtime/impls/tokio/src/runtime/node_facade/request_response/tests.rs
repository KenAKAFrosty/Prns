use tokio::sync::mpsc::{self, UnboundedReceiver};

use crate::engine::RequestResponseTimeout;
use crate::engine::Settlement;
use crate::manifold::compression;
use crate::manifold::driver::{HostCommand, HostResourceMetadata};
use crate::routing::links::request::{
    parse_response_plaintext, write_packed_binary_header, RequestId, MAX_PACKED_BINARY_HEADER_LEN,
    RESPONSE_WIRE_OVERHEAD,
};
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::runtime::request_endpoints::RespondToken;
use crate::units::{ByteLimit, DurationMillis, RttMillis};

use super::super::PrnsNodeHandle;
use super::{RequestOptions, RESPONSE_PACKET_CEILING};
use prns_core::rncp::parse_file_metadata;

static MULTI_SEGMENT_STATIC_RESPONSE: [u8; super::STATIC_RESPONSE_SEGMENT_BYTES * 2 + 33_333] =
    [0x42; super::STATIC_RESPONSE_SEGMENT_BYTES * 2 + 33_333];

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
    assert_eq!(request.maximum_response_bytes, ByteLimit::Unlimited);
    request
        .completion
        .send(Ok((b"pong".to_vec(), RttMillis::new(42))))
        .unwrap();

    let (data, rtt) = requesting.await.unwrap().unwrap();
    assert_eq!(data, b"pong");
    assert_eq!(rtt, RttMillis::new(42));
}

#[tokio::test]
async fn request_options_preserve_the_response_ceiling() {
    let (handle, mut command_rx) = handle();
    let link = LinkId::new([7; 16]);
    let path_hash = RequestPathHash::new([0x46; 16]);
    let options = RequestOptions {
        response_timeout: RequestResponseTimeout::Exact(DurationMillis(45_000)),
        maximum_response_bytes: ByteLimit::Maximum(8_192),
    };

    let requesting = tokio::spawn(async move {
        handle
            .request_with_options(link, path_hash, b"bounded", options)
            .await
    });
    let HostCommand::RequestAny(request) = command_rx.recv().await.unwrap() else {
        panic!("request issues a RequestAny host command");
    };
    assert_eq!(request.response_timeout, options.response_timeout);
    assert_eq!(
        request.maximum_response_bytes,
        options.maximum_response_bytes
    );
    request
        .completion
        .send(Ok((b"done".to_vec(), RttMillis::new(42))))
        .unwrap();
    assert_eq!(requesting.await.unwrap().unwrap().0, b"done");
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
async fn respond_packed_returns_the_links_round_trip() {
    let (handle, _command_rx) = handle();
    let token = RespondToken {
        link_id: LinkId::new([1; 16]),
        request_id: RequestId([2; 16]),
        rtt: RttMillis::new(99),
    };
    assert_eq!(
        handle.respond_packed(token, b"answer"),
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
    assert_eq!(
        handle.respond_packed(token, &body),
        Some(RttMillis::new(50))
    );
    let Some(HostCommand::RespondAny(respond)) = command_rx.recv().await else {
        panic!("expected a RespondAny command");
    };
    let (enclosed_request, enclosed_body) =
        parse_response_plaintext(respond.packed.as_slice()).expect("stock RNS response envelope");
    assert_eq!(enclosed_request, token.request_id);
    assert_eq!(enclosed_body, body);
    assert_eq!(
        respond
            .compressed_candidate
            .as_ref()
            .map(|candidate| candidate.as_slice().to_vec()),
        compression::compress_if_smaller(respond.packed.as_slice()),
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
    handle.respond_packed(token, &body);
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
    let responding =
        tokio::spawn(async move { handle.respond_owned_packed_settled(token, body).await });

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
        .send(Settlement::Respond(Ok(())))
        .expect("route awaits completion");
    assert_eq!(responding.await.unwrap().unwrap(), RttMillis::new(33));
}

#[tokio::test]
async fn respond_bytes_adds_message_pack_binary_framing() {
    let (handle, mut command_rx) = handle();
    let token = RespondToken {
        link_id: LinkId::new([9; 16]),
        request_id: RequestId([10; 16]),
        rtt: RttMillis::new(34),
    };
    assert_eq!(
        handle.respond_bytes(token, b"hello"),
        Some(RttMillis::new(34))
    );
    let Some(HostCommand::RespondAny(respond)) = command_rx.recv().await else {
        panic!("expected a RespondAny command");
    };
    assert_eq!(
        respond.packed.as_slice(),
        &[0xC4, 5, b'h', b'e', b'l', b'l', b'o']
    );
}

#[tokio::test]
async fn respond_bytes_streaming_reads_a_bounded_source_into_a_response_resource() {
    let (handle, mut command_rx) = handle();
    let token = RespondToken {
        link_id: LinkId::new([11; 16]),
        request_id: RequestId([12; 16]),
        rtt: RttMillis::new(35),
    };
    let bytes = std::vec![0xA5; RESPONSE_PACKET_CEILING + 1];
    let byte_len = bytes.len() as u64;
    let responding = tokio::spawn(async move {
        handle
            .respond_bytes_streaming(token, byte_len, std::io::Cursor::new(bytes))
            .await
    });
    let Some(HostCommand::SendResourceSegment(segment)) = command_rx.recv().await else {
        panic!("expected a response resource segment");
    };
    let mut binary_header = [0u8; MAX_PACKED_BINARY_HEADER_LEN];
    let binary_header_len =
        write_packed_binary_header(RESPONSE_PACKET_CEILING + 1, &mut binary_header).unwrap();
    assert_eq!(segment.request_id, Some(token.request_id));
    assert_eq!(
        segment.data.len(),
        RESPONSE_WIRE_OVERHEAD + binary_header_len + RESPONSE_PACKET_CEILING + 1
    );
    let (request_id, packed) = parse_response_plaintext(segment.data.as_slice()).unwrap();
    assert_eq!(request_id, token.request_id);
    assert_eq!(
        &packed[..binary_header_len],
        &binary_header[..binary_header_len]
    );
    assert!(packed[binary_header_len..].iter().all(|byte| *byte == 0xA5));
    segment
        .completion
        .send(Settlement::Respond(Ok(())))
        .unwrap();
    assert_eq!(responding.await.unwrap().unwrap(), token.rtt);
}

#[tokio::test]
async fn static_file_response_waits_for_each_proof_and_bounds_each_window() {
    let (handle, mut command_rx) = handle();
    let token = RespondToken {
        link_id: LinkId::new([13; 16]),
        request_id: RequestId([14; 16]),
        rtt: RttMillis::new(36),
    };
    let bytes: &'static [u8] = &MULTI_SEGMENT_STATIC_RESPONSE;
    let responding = tokio::spawn(async move {
        handle
            .respond_static_file_settled(token, "source.zip", bytes)
            .await
    });

    let mut expected_index = 1u64;
    let mut total_segments = None;
    loop {
        let Some(HostCommand::SendResourceSegment(segment)) = command_rx.recv().await else {
            panic!("expected a response resource segment");
        };
        assert_eq!(segment.request_id, Some(token.request_id));
        assert_eq!(segment.segment_index, expected_index);
        assert!(
            segment.data.len() <= super::STATIC_RESPONSE_SEGMENT_BYTES,
            "one live plaintext window remains bounded"
        );
        assert!(
            segment.data.as_slice().iter().all(|byte| *byte == 0x42),
            "every window carries bare file bytes with no envelope or binary header"
        );
        if expected_index == 1 {
            let HostResourceMetadata::Packed(metadata) = &segment.metadata else {
                panic!("first segment carries filename metadata");
            };
            assert_eq!(
                parse_file_metadata(metadata.as_slice()).unwrap(),
                b"source.zip"
            );
            total_segments = Some(segment.total_segments);
            assert!(segment.total_segments > 2);
        } else {
            assert!(matches!(
                segment.metadata,
                HostResourceMetadata::SentInFirstSegment { .. }
            ));
            assert_eq!(Some(segment.total_segments), total_segments);
        }
        assert!(
            command_rx.try_recv().is_err(),
            "the next window is not queued before this proof"
        );
        segment
            .completion
            .send(Settlement::Respond(Ok(())))
            .unwrap();
        if Some(expected_index) == total_segments {
            break;
        }
        expected_index += 1;
    }

    assert_eq!(responding.await.unwrap().unwrap(), token.rtt);
}

#[tokio::test]
async fn an_open_file_response_streams_from_its_handle_with_filename_metadata() {
    let (handle, mut command_rx) = handle();
    let token = RespondToken {
        link_id: LinkId::new([17; 16]),
        request_id: RequestId([18; 16]),
        rtt: RttMillis::new(51),
    };
    let expected = std::fs::read("Cargo.toml").unwrap();
    let source = std::fs::File::open("Cargo.toml").unwrap();
    let byte_len = expected.len() as u64;
    let responding = tokio::spawn(async move {
        handle
            .respond_open_file_settled(token, "Cargo.toml", source, byte_len)
            .await
    });

    let Some(HostCommand::SendResourceSegment(segment)) = command_rx.recv().await else {
        panic!("expected an open-file response resource segment");
    };
    assert_eq!(segment.request_id, Some(token.request_id));
    assert_eq!(segment.segment_index, 1);
    assert_eq!(segment.total_segments, 1);
    assert_eq!(segment.data.as_slice(), expected.as_slice());
    let HostResourceMetadata::Packed(metadata) = &segment.metadata else {
        panic!("the segment carries filename metadata");
    };
    assert_eq!(
        parse_file_metadata(metadata.as_slice()).unwrap(),
        b"Cargo.toml"
    );
    segment
        .completion
        .send(Settlement::Respond(Ok(())))
        .unwrap();

    assert_eq!(responding.await.unwrap().unwrap(), token.rtt);
}
