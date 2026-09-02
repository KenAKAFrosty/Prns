use tokio::sync::mpsc::{self, UnboundedReceiver};

use crate::engine::{SendResourceFailure, Settlement};
use crate::manifold::compression;
use crate::manifold::driver::{HostCommand, ResourceInbound};
use crate::routing::links::resources::{ResourceHash, MAX_EFFICIENT_SIZE};
use crate::routing::links::LinkId;

use super::super::PrnsNodeHandle;
use super::{
    ResourceReceipt, ResourceReceiveError, ResourceSendError, SegmentCompression,
    ENGINE_SEGMENT_LANES,
};

fn handle() -> (PrnsNodeHandle, UnboundedReceiver<HostCommand>) {
    let (commands, command_rx) = mpsc::unbounded_channel();
    (PrnsNodeHandle::over(commands), command_rx)
}

const LINK: LinkId = LinkId::new([5; 16]);

#[tokio::test]
async fn send_resource_keeps_full_leading_segments_and_balances_a_tiny_tail() {
    let (prns, mut command_rx) = handle();
    let total_len = 2 * MAX_EFFICIENT_SIZE as u64 + 100;
    let payload: std::vec::Vec<u8> = (0..total_len).map(|i| i as u8).collect();

    let drainer = tokio::spawn(async move {
        let mut got = std::vec::Vec::new();
        loop {
            let Some(HostCommand::SendResourceSegment(seg)) = command_rx.recv().await else {
                panic!("expected a SendResourceSegment command");
            };
            let last = seg.segment_index == seg.total_segments;
            if seg.segment_index == 1 {
                assert!(
                    seg.compressed_candidate.is_some(),
                    "a compressible split segment carries its bz2 candidate",
                );
            }
            got.push((
                seg.segment_index,
                seg.total_segments,
                seg.data.as_slice().to_vec(),
            ));
            seg.completion
                .send(Settlement::SendResource(Ok(())))
                .expect("the awaiter is still parked");
            if last {
                break;
            }
        }
        got
    });

    prns.send_resource(LINK, total_len, &payload[..])
        .await
        .expect("the stream completes");
    let got = drainer.await.unwrap();

    assert_eq!(got.len(), 3, "the payload needs three protocol segments");
    assert_eq!((got[0].0, got[0].1), (1, 3));
    assert_eq!((got[1].0, got[1].1), (2, 3));
    assert_eq!((got[2].0, got[2].1), (3, 3));
    assert_eq!(got[0].2.len(), MAX_EFFICIENT_SIZE);
    assert_eq!(got[1].2.len(), (MAX_EFFICIENT_SIZE + 100).div_ceil(2));
    assert_eq!(got[2].2.len(), (MAX_EFFICIENT_SIZE + 100) / 2);
    let mut reassembled = got[0].2.clone();
    reassembled.extend_from_slice(&got[1].2);
    reassembled.extend_from_slice(&got[2].2);
    assert_eq!(
        reassembled, payload,
        "the segments reassemble to the source"
    );
}

#[tokio::test]
async fn a_small_send_resource_is_one_unsplit_segment() {
    let (prns, mut command_rx) = handle();
    let payload = std::vec![3u8; 500];
    let drainer = tokio::spawn(async move {
        let Some(HostCommand::SendResourceSegment(seg)) = command_rx.recv().await else {
            panic!("expected a SendResourceSegment command");
        };
        let placement = (
            seg.segment_index,
            seg.total_segments,
            seg.data.as_slice().len(),
        );
        seg.completion
            .send(Settlement::SendResource(Ok(())))
            .expect("the awaiter is still parked");
        placement
    });
    prns.send_resource(LINK, 500, &payload[..])
        .await
        .expect("the single segment completes");
    assert_eq!(
        drainer.await.unwrap(),
        (1, 1, 500),
        "a sub-segment payload crosses as one unsplit resource",
    );
}

#[tokio::test]
async fn a_resource_length_that_overflows_with_metadata_is_rejected() {
    let (prns, mut command_rx) = handle();
    let error = prns
        .send_resource_with_metadata(LINK, u64::MAX, &[][..], &[0x81])
        .await
        .unwrap_err();
    assert!(matches!(error, ResourceSendError::UnrepresentableLength));
    assert!(command_rx.try_recv().is_err());
}

#[test]
fn a_split_resource_claim_cannot_raise_the_per_segment_inflate_bound() {
    assert_eq!(
        prns_runtime::resource_compression::resource_decompression_bound(u64::MAX),
        MAX_EFFICIENT_SIZE as u64,
    );
    assert_eq!(
        prns_runtime::resource_compression::resource_decompression_bound(4096),
        4096,
    );
}

#[tokio::test]
async fn send_resource_compresses_a_compressible_segment() {
    let (prns, mut command_rx) = handle();
    let payload = std::vec![7u8; 8192];
    let drainer = tokio::spawn(async move {
        let Some(HostCommand::SendResourceSegment(seg)) = command_rx.recv().await else {
            panic!("expected a SendResourceSegment command");
        };
        let candidate = seg
            .compressed_candidate
            .as_ref()
            .map(|c| c.as_slice().to_vec());
        seg.completion
            .send(Settlement::SendResource(Ok(())))
            .expect("the awaiter is still parked");
        candidate
    });
    prns.send_resource(LINK, payload.len() as u64, &payload[..])
        .await
        .expect("the single segment completes");
    let candidate = drainer.await.unwrap();
    assert_eq!(
        candidate,
        compression::compress_if_smaller(&payload),
        "the segment rides a bz2 candidate matching the codec",
    );
    assert!(
        candidate.is_some_and(|c| c.len() < payload.len()),
        "a run of one byte compresses far below its length",
    );
}

#[tokio::test]
async fn send_resource_declines_to_compress_incompressible_data() {
    let (prns, mut command_rx) = handle();
    let mut x = 0x9e37_79b9_7f4a_7c15u64;
    let payload: std::vec::Vec<u8> = (0..8192)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    let drainer = tokio::spawn(async move {
        let Some(HostCommand::SendResourceSegment(seg)) = command_rx.recv().await else {
            panic!("expected a SendResourceSegment command");
        };
        let compressed = seg.compressed_candidate.is_some();
        seg.completion
            .send(Settlement::SendResource(Ok(())))
            .expect("the awaiter is still parked");
        compressed
    });
    prns.send_resource(LINK, payload.len() as u64, &payload[..])
        .await
        .expect("the single segment completes");
    assert!(
        !drainer.await.unwrap(),
        "high-entropy bytes carry no candidate, so the transfer stays uncompressed",
    );
}

#[tokio::test]
async fn never_compression_ships_a_compressible_segment_uncompressed() {
    let (prns, mut command_rx) = handle();
    let payload = std::vec![7u8; 8192];
    let drainer = tokio::spawn(async move {
        let Some(HostCommand::SendResourceSegment(seg)) = command_rx.recv().await else {
            panic!("expected a SendResourceSegment command");
        };
        let compressed = seg.compressed_candidate.is_some();
        seg.completion
            .send(Settlement::SendResource(Ok(())))
            .expect("the awaiter is still parked");
        compressed
    });
    prns.send_resource_with_compression(
        LINK,
        payload.len() as u64,
        &payload[..],
        SegmentCompression::Never,
    )
    .await
    .expect("the single segment completes");
    assert!(
        !drainer.await.unwrap(),
        "RNS auto_compress=False: no attempt, even on a run that would compress",
    );
}

#[tokio::test]
async fn a_segment_past_the_attempt_ceiling_ships_uncompressed() {
    let (prns, mut command_rx) = handle();
    let payload = std::vec![7u8; 8192];
    let drainer = tokio::spawn(async move {
        let Some(HostCommand::SendResourceSegment(seg)) = command_rx.recv().await else {
            panic!("expected a SendResourceSegment command");
        };
        let compressed = seg.compressed_candidate.is_some();
        seg.completion
            .send(Settlement::SendResource(Ok(())))
            .expect("the awaiter is still parked");
        compressed
    });
    prns.send_resource_with_compression(
        LINK,
        payload.len() as u64,
        &payload[..],
        SegmentCompression::Attempt {
            up_to_byte_len: payload.len() as u64 - 1,
        },
    )
    .await
    .expect("the single segment completes");
    assert!(
        !drainer.await.unwrap(),
        "RNS auto_compress=<int>: a segment over the ceiling is never attempted",
    );
}

#[tokio::test]
async fn send_resource_surfaces_a_segment_rejection_and_stops() {
    let (prns, mut command_rx) = handle();
    let total_len = 2 * MAX_EFFICIENT_SIZE as u64 + 100;
    let payload = std::vec![7u8; total_len as usize];
    let drainer = tokio::spawn(async move {
        let mut issued = 0u32;
        while let Some(command) = command_rx.recv().await {
            let HostCommand::SendResourceSegment(seg) = command else {
                panic!("expected a SendResourceSegment command");
            };
            issued += 1;
            let _ = seg.completion.send(Settlement::SendResource(Err(
                SendResourceFailure::RejectedByPeer,
            )));
        }
        issued
    });

    let result = prns.send_resource(LINK, total_len, &payload[..]).await;
    assert!(matches!(
        result,
        Err(ResourceSendError::Rejected(
            SendResourceFailure::RejectedByPeer
        )),
    ));
    drop(prns);
    assert_eq!(
            drainer.await.unwrap(),
            ENGINE_SEGMENT_LANES as u32,
            "a rejected first segment stops the stream — only its already-staged follower ever issued, the third never does",
        );
}

#[tokio::test]
async fn send_resource_on_a_stopped_node_is_node_stopped() {
    let (prns, command_rx) = handle();
    drop(command_rx);
    let payload = std::vec![0u8; 10];
    assert!(matches!(
        prns.send_resource(LINK, 10, &payload[..]).await,
        Err(ResourceSendError::NodeStopped),
    ));
}

#[tokio::test]
async fn receive_resource_streams_an_inbound_resource_into_the_sink() {
    let (prns, mut command_rx) = handle();
    let original = ResourceHash::new([9; 32]);

    let actor = tokio::spawn(async move {
        let Some(HostCommand::RegisterResourceSink {
            link_id,
            sink,
            ready,
        }) = command_rx.recv().await
        else {
            panic!("expected a RegisterResourceSink command");
        };
        ready.send(()).expect("the receiver awaits registration");
        sink.send(ResourceInbound::Chunk(b"hello ".to_vec()))
            .unwrap();
        sink.send(ResourceInbound::Chunk(b"world".to_vec()))
            .unwrap();
        sink.send(ResourceInbound::Complete {
            original_hash: original,
            total_size_bytes: 11,
        })
        .unwrap();
        link_id
    });

    let mut buf = std::vec::Vec::new();
    let receipt = prns
        .receive_resource(LINK, &mut buf)
        .await
        .expect("the resource arrives");
    assert_eq!(
        actor.await.unwrap(),
        LINK,
        "the sink registered on the link"
    );
    assert_eq!(
        buf, b"hello world",
        "the chunks stream into the sink in order"
    );
    assert_eq!(
        receipt,
        ResourceReceipt {
            original_hash: original,
            total_size_bytes: 11,
            metadata: None,
        },
    );
}

#[tokio::test]
async fn receive_resource_carries_metadata_on_the_receipt() {
    let (prns, mut command_rx) = handle();
    let original = ResourceHash::new([9; 32]);

    let actor = tokio::spawn(async move {
        let Some(HostCommand::RegisterResourceSink { sink, ready, .. }) = command_rx.recv().await
        else {
            panic!("expected a RegisterResourceSink command");
        };
        ready.send(()).expect("the receiver awaits registration");
        sink.send(ResourceInbound::Metadata(b"packed".to_vec()))
            .unwrap();
        sink.send(ResourceInbound::Chunk(b"payload".to_vec()))
            .unwrap();
        sink.send(ResourceInbound::Complete {
            original_hash: original,
            total_size_bytes: 7,
        })
        .unwrap();
    });

    let mut buf = std::vec::Vec::new();
    let receipt = prns
        .receive_resource(LINK, &mut buf)
        .await
        .expect("the resource arrives");
    actor.await.unwrap();
    assert_eq!(buf, b"payload", "the metadata never enters the byte stream");
    assert_eq!(
        receipt,
        ResourceReceipt {
            original_hash: original,
            total_size_bytes: 7,
            metadata: Some(b"packed".to_vec()),
        },
    );
}

#[tokio::test]
async fn receive_resource_surfaces_a_failed_transfer() {
    let (prns, mut command_rx) = handle();
    let actor = tokio::spawn(async move {
        let Some(HostCommand::RegisterResourceSink { sink, ready, .. }) = command_rx.recv().await
        else {
            panic!("expected a RegisterResourceSink command");
        };
        ready.send(()).unwrap();
        sink.send(ResourceInbound::Failed).unwrap();
    });
    let mut buf = std::vec::Vec::new();
    let result = prns.receive_resource(LINK, &mut buf).await;
    actor.await.unwrap();
    assert!(matches!(result, Err(ResourceReceiveError::Failed)));
    assert!(buf.is_empty(), "a failed transfer wrote nothing");
}

#[tokio::test]
async fn receive_resource_on_a_stopped_node_is_node_stopped() {
    let (prns, command_rx) = handle();
    drop(command_rx);
    let mut buf = std::vec::Vec::new();
    assert!(matches!(
        prns.receive_resource(LINK, &mut buf).await,
        Err(ResourceReceiveError::NodeStopped),
    ));
}
