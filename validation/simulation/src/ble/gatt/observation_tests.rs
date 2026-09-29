use std::future::{poll_fn, Future};
use std::task::Poll;

use super::tests::data_pair;
use super::*;
use crate::ble::{BleDataCounters, BleDataSendObservation};
use personal_rns::wire::{WireContext, WirePacketHeader};

#[tokio::test]
async fn latest_header_and_baseline_replace_without_retaining_payload_or_aliasing_snapshots() {
    let (mut sink, mut source) = data_pair(20);
    let connection = sink.endpoint.connection.clone();
    assert_eq!(connection.data_snapshot().dialer_last_send, None);
    let mut frame = vec![0; 64];
    frame[2..18].fill(0x71);
    frame[18] = WireContext::Response.to_byte();
    frame[19..].fill(0xa5);
    let header = WirePacketHeader::parse(&frame).unwrap().0;
    assert_eq!(header.context, WireContext::Response);
    let mut output = [0; 64];
    let (sent, received) = tokio::join!(sink.send_frame(&frame), source.recv_frame(&mut output));
    assert_eq!((sent, received), (Ok(()), Ok(64)));
    assert_eq!(output.as_slice(), frame);
    let old = connection.data_snapshot();
    let expected = BleDataSendObservation {
        header: Some(header),
        frame_length: 64,
        before: BleDataCounters::default(),
    };
    assert_eq!(old.dialer_last_send, Some(expected.clone()));
    assert_eq!(old.listener_last_send, None);

    assert_eq!(sink.send_frame(&[]).await, Err(VirtualBleError::EmptyFrame));
    assert_eq!(connection.data_snapshot(), old);
    assert!(sink.send_frame(&[0; BLE_HW_MTU + 1]).await.is_err());
    assert_eq!(connection.data_snapshot(), old);

    let before = old.dialer_to_listener;
    let (sent, received) = tokio::join!(sink.send_frame(b"short"), source.recv_frame(&mut output));
    assert_eq!((sent, received), (Ok(()), Ok(5)));
    assert_eq!(
        connection.data_snapshot().dialer_last_send,
        Some(BleDataSendObservation {
            header: None,
            frame_length: 5,
            before,
        })
    );
    assert_eq!(old.dialer_last_send, Some(expected));
}

#[tokio::test]
async fn cancelled_partial_send_keeps_attempt_metadata_until_the_next_send() {
    let (mut sink, _source) = data_pair(10);
    let connection = sink.endpoint.connection.clone();
    {
        let mut sending = std::pin::pin!(sink.send_frame(b"partial cancelled frame"));
        poll_fn(|cx| {
            assert!(sending.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }
    let snapshot = connection.data_snapshot();
    let expected = BleDataSendObservation {
        header: WirePacketHeader::parse(b"partial cancelled frame")
            .ok()
            .map(|(header, _)| header),
        frame_length: 23,
        before: BleDataCounters::default(),
    };
    assert_eq!(snapshot.dialer_last_send, Some(expected));
    assert_eq!(
        snapshot.dialer_to_listener,
        BleDataCounters {
            sends_started: 1,
            fragments_queued: 1,
            ..BleDataCounters::default()
        }
    );
    assert!(connection.close());
    assert_eq!(connection.data_snapshot(), snapshot);
}
