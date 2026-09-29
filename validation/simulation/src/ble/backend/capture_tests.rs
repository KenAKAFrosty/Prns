use super::*;
use crate::ble::{BleWireCapture, BleWireChannel, BleWireValue};
use personal_rns::interfaces::bluetooth_auto::{BleSink, BleSource};
use std::future::{poll_fn, Future};
use std::task::Poll;

fn links() -> (VirtualBleLink, VirtualBleLink, BleWireCapture) {
    let capture = BleWireCapture::new(
        NonZeroUsize::new(64).unwrap_or_else(|| unreachable!("nonzero capacity")),
    );
    let (first, second) = links_with_capture(&capture);
    (first, second, capture)
}

fn links_with_capture(capture: &BleWireCapture) -> (VirtualBleLink, VirtualBleLink) {
    let first = BleAddress::new([1; 6]);
    let second = BleAddress::new([2; 6]);
    let config = VirtualBleLinkConfig::new(
        1,
        1,
        BLE_HW_MTU,
        VirtualGattConfig::new(CONTROL_MAX_LEN, 20)
            .unwrap_or_else(|error| unreachable!("GATT: {error}")),
    )
    .unwrap_or_else(|error| unreachable!("link: {error}"));
    link_pair(
        first,
        second,
        config,
        config,
        -40,
        -50,
        Arc::new(
            Connection::new(first, second).with_wire_capture(
                capture
                    .bind()
                    .unwrap_or_else(|error| unreachable!("capture: {error:?}")),
            ),
        ),
    )
}

#[tokio::test]
async fn retained_old_links_cannot_capture_or_deliver_into_replacement_incarnations() {
    let (mut old_sender, mut old_receiver, capture) = links();
    old_sender
        .send_control_value(&[255])
        .await
        .unwrap_or_else(|error| unreachable!("old queue: {error}"));
    assert!(old_sender.endpoint.connection.close());
    let (mut replacement_sender, mut replacement_receiver) = links_with_capture(&capture);
    replacement_sender
        .send_control_value(&[255])
        .await
        .unwrap_or_else(|error| unreachable!("replacement queue: {error}"));
    let snapshot = capture.snapshot();
    assert_eq!(snapshot.discarded_values, 0);
    assert_eq!(snapshot.values.len(), 2);
    assert_ne!(snapshot.values[0].connection, snapshot.values[1].connection);
    assert_eq!(
        snapshot.values[0],
        BleWireValue {
            connection: snapshot.values[0].connection,
            ..snapshot.values[1].clone()
        }
    );
    assert_eq!(
        old_receiver.control_recv().await,
        Err(VirtualBleError::LinkClosed)
    );
    assert_eq!(
        old_sender.send_control_value(&[3, 0]).await,
        Err(VirtualBleError::LinkClosed)
    );
    assert_eq!(
        replacement_receiver.control_recv().await,
        Err(VirtualBleError::ControlParse(
            ControlParseError::UnknownKind(255)
        ))
    );
    assert_eq!(capture.snapshot(), snapshot);
    drop(old_sender);
    drop(old_receiver);
    replacement_receiver
        .send_control_value(&[255])
        .await
        .unwrap_or_else(|error| unreachable!("reverse replacement: {error}"));
    assert_eq!(
        replacement_sender.control_recv().await,
        Err(VirtualBleError::ControlParse(
            ControlParseError::UnknownKind(255)
        ))
    );
    let values = capture.snapshot().values;
    assert_eq!(
        values[2],
        BleWireValue {
            connection: values[1].connection,
            from: values[1].to,
            to: values[1].from,
            channel: BleWireChannel::Control,
            bytes: vec![255],
        }
    );
}

#[tokio::test]
async fn control_capture_is_queue_acceptance_not_parsing_or_attempts() {
    let (mut first, mut second, capture) = links();
    first
        .send_control_value(&[255])
        .await
        .unwrap_or_else(|error| unreachable!("send: {error}"));
    let accepted = capture.snapshot();
    assert_eq!(
        accepted.values,
        [BleWireValue {
            connection: accepted.values[0].connection,
            from: BleAddress::new([1; 6]),
            to: BleAddress::new([2; 6]),
            channel: BleWireChannel::Control,
            bytes: vec![255],
        }]
    );
    {
        let mut pending = std::pin::pin!(first.send_control_value(&[3, 0]));
        poll_fn(|cx| {
            assert!(pending.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }
    assert!(first
        .send_control_value(&[0; CONTROL_MAX_LEN + 1])
        .await
        .is_err());
    assert_eq!(capture.snapshot(), accepted);
    assert_eq!(
        second.control_recv().await,
        Err(VirtualBleError::ControlParse(
            ControlParseError::UnknownKind(255)
        ))
    );
    second
        .send_control_value(&[3, 0])
        .await
        .unwrap_or_else(|error| unreachable!("send: {error}"));
    assert_eq!(
        capture.snapshot().values[1],
        BleWireValue {
            connection: accepted.values[0].connection,
            from: BleAddress::new([2; 6]),
            to: BleAddress::new([1; 6]),
            channel: BleWireChannel::Control,
            bytes: vec![3, 0],
        }
    );
    drop(second);
    let before = capture.snapshot();
    assert_eq!(
        first.send_control_value(&[3, 0]).await,
        Err(VirtualBleError::LinkClosed)
    );
    assert_eq!(capture.snapshot(), before);
}

#[tokio::test]
async fn data_capture_contains_exact_queued_fragments_including_partial_cancellation() {
    let (first, mut second, capture) = links();
    let (_source, mut sink) = first.into_data();
    let payload = [42; 96];
    let (sent, queued) = tokio::join!(sink.send_frame(&payload), async {
        let mut queued = Vec::new();
        // Drain until the fragmenter has accepted the whole frame, without reconstructing bytes.
        let count = personal_rns::interfaces::bluetooth_auto::fragments_of(&payload, 20).count();
        for _ in 0..count {
            queued.push(
                second
                    .data_rx
                    .recv()
                    .await
                    .unwrap_or_else(|| unreachable!("queued fragment")),
            );
        }
        queued
    });
    sent.unwrap_or_else(|error| unreachable!("send: {error}"));
    let values = capture.snapshot().values;
    assert!(values.len() > 1);
    assert_eq!(
        values
            .iter()
            .map(|value| value.bytes.clone())
            .collect::<Vec<_>>(),
        queued
    );
    assert!(values
        .iter()
        .all(|value| value.channel == BleWireChannel::Data
            && value.from == BleAddress::new([1; 6])
            && value.to == BleAddress::new([2; 6])
            && value.bytes.len() <= 20));
    let before = values.len();
    {
        let mut pending = std::pin::pin!(sink.send_frame(&payload));
        poll_fn(|cx| {
            assert!(pending.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }
    let after = capture.snapshot();
    assert_eq!(after.values.len(), before + 1);
    assert_eq!(
        after.values[before].bytes,
        second
            .data_rx
            .recv()
            .await
            .unwrap_or_else(|| unreachable!("queued fragment"))
    );
    assert!(second.data_rx.try_recv().is_err());
    assert_eq!(after.discarded_values, 0);
}

#[tokio::test]
async fn captured_fragmentation_still_delivers_the_whole_frame() {
    let (first, second, capture) = links();
    let (_source, mut sink) = first.into_data();
    let (mut source, _sink) = second.into_data();
    let payload = [7; 256];
    let mut output = [0; BLE_HW_MTU];
    let (sent, received) = tokio::join!(sink.send_frame(&payload), source.recv_frame(&mut output));
    sent.unwrap_or_else(|error| unreachable!("send: {error}"));
    assert_eq!(
        &output[..received.unwrap_or_else(|error| unreachable!("receive: {error}"))],
        &payload
    );
    assert!(capture.snapshot().values.len() > 1);
}
