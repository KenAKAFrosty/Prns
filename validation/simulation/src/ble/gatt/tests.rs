use std::error::Error;
use std::future::{poll_fn, Future};
use std::task::Poll;

use super::*;
use crate::ble::connection::Connection;
use crate::ble::BleAddress;

fn endpoints() -> (Arc<ConnectionEndpoint>, Arc<ConnectionEndpoint>) {
    let connection = Arc::new(Connection::new(
        BleAddress::new([1; 6]),
        BleAddress::new([2; 6]),
    ));
    (
        Arc::new(ConnectionEndpoint {
            connection: connection.clone(),
        }),
        Arc::new(ConnectionEndpoint { connection }),
    )
}

fn data_pair(value_limit: usize) -> (VirtualBleSink, VirtualBleSource) {
    let (first, second) = endpoints();
    let (sender, receiver) = mpsc::channel(1);
    (
        VirtualBleSink::new(sender, BLE_HW_MTU, value_limit, first),
        VirtualBleSource::new(receiver, second),
    )
}

#[test]
fn characteristic_limits_reject_invalid_whole_values() {
    assert_eq!(
        VirtualGattConfig::new(0, 20),
        Err(VirtualGattConfigError::ZeroControlValueLimit)
    );
    for requested in 0..=FRAGMENT_HEADER_LEN {
        assert_eq!(
            VirtualGattConfig::new(CONTROL_MAX_LEN, requested),
            Err(VirtualGattConfigError::DataValueLimitTooSmall {
                requested,
                minimum: FRAGMENT_HEADER_LEN + 1
            })
        );
    }
    assert!(VirtualGattConfig::new(CONTROL_MAX_LEN, FRAGMENT_HEADER_LEN + 1).is_ok());
}

#[tokio::test]
async fn transmitted_fragments_match_wire_goldens() -> Result<(), Box<dyn Error>> {
    let (mut sink, mut source) = data_pair(10);
    let (sent, values) = tokio::join!(sink.send_frame(b"hello-world"), async {
        let mut values = Vec::new();
        for _ in 0..3 {
            values.push(
                source
                    .receiver
                    .recv()
                    .await
                    .ok_or(VirtualBleError::LinkClosed)?,
            );
        }
        Ok::<_, VirtualBleError>(values)
    });
    sent?;
    assert_eq!(
        values?,
        vec![
            vec![1, 0, 0, 0, 3, b'h', b'e', b'l', b'l', b'o'],
            vec![2, 0, 1, 0, 3, b'-', b'w', b'o', b'r', b'l'],
            vec![3, 0, 2, 0, 3, b'd'],
        ]
    );
    Ok(())
}

#[tokio::test]
async fn maximum_frame_reassembles_at_minimum_and_typical_value_limits(
) -> Result<(), Box<dyn Error>> {
    let frame: Vec<_> = (0..BLE_HW_MTU).map(|byte| byte as u8).collect();
    for value_limit in [
        FRAGMENT_HEADER_LEN + 1,
        20,
        120,
        180,
        BLE_HW_MTU + FRAGMENT_HEADER_LEN,
    ] {
        let (mut sink, mut source) = data_pair(value_limit);
        let mut received = [0; BLE_HW_MTU];
        let (sent, read) = tokio::join!(sink.send_frame(&frame), source.recv_frame(&mut received));
        sent?;
        assert_eq!(read, Ok(BLE_HW_MTU));
        assert_eq!(received.as_slice(), frame);
    }
    Ok(())
}

#[tokio::test]
async fn whole_frame_copy_checks_capacity_and_keeps_the_next_frame_after_refusal(
) -> Result<(), Box<dyn Error>> {
    const CANARY: u8 = 0xCC;
    let frame = [0x17; BLE_HW_MTU];
    for capacity in [0, 1, BLE_HW_MTU - 1, BLE_HW_MTU, BLE_HW_MTU + 1] {
        let (mut sink, mut source) = data_pair(20);
        let mut output = [CANARY; BLE_HW_MTU + 1];
        let (sent, received) = tokio::join!(
            sink.send_frame(&frame),
            source.recv_frame(&mut output[..capacity])
        );
        sent?;
        let mut expected_output = [CANARY; BLE_HW_MTU + 1];
        let expected = if capacity < frame.len() {
            Err(VirtualBleError::ReceiveBufferTooSmall {
                frame: frame.len(),
                buffer: capacity,
            })
        } else {
            expected_output[..frame.len()].copy_from_slice(&frame);
            Ok(frame.len())
        };
        assert_eq!((received, output), (expected, expected_output));

        let mut next = [CANARY; 6];
        let (sent, received) =
            tokio::join!(sink.send_frame(b"fresh"), source.recv_frame(&mut next));
        sent?;
        assert_eq!(
            (received, next),
            (Ok(5), [b'f', b'r', b'e', b's', b'h', CANARY])
        );
    }
    Ok(())
}

#[tokio::test]
async fn cancelling_receive_preserves_partial_reassembly() -> Result<(), Box<dyn Error>> {
    let (mut sink, mut source) = data_pair(10);
    let mut sending = std::pin::pin!(sink.send_frame(b"hello-world"));
    poll_fn(|cx| {
        assert!(sending.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    assert_eq!(source.receiver.len(), 1);
    let mut received = [0; 11];
    {
        let mut receiving = std::pin::pin!(source.recv_frame(&mut received));
        poll_fn(|cx| {
            assert!(receiving.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }
    let (sent, read) = tokio::join!(sending, source.recv_frame(&mut received));
    sent?;
    assert_eq!(read, Ok(11));
    assert_eq!(&received, b"hello-world");
    Ok(())
}

#[tokio::test]
async fn next_frame_replaces_a_cancelled_partial_send() -> Result<(), Box<dyn Error>> {
    let (mut sink, mut source) = data_pair(10);
    {
        let mut sending = std::pin::pin!(sink.send_frame(b"abandoned"));
        poll_fn(|cx| {
            assert!(sending.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }
    assert_eq!(source.receiver.len(), 1);
    let mut received = [0; 6];
    let (sent, read) = tokio::join!(sink.send_frame(b"fresh!"), source.recv_frame(&mut received));
    sent?;
    assert_eq!(read, Ok(6));
    assert_eq!(&received, b"fresh!");
    Ok(())
}

#[tokio::test]
async fn disconnect_mid_frame_wakes_sender_and_never_delivers_partial_data() {
    let (mut sink, mut source) = data_pair(10);
    let connection = sink.endpoint.connection.clone();
    let mut sending = std::pin::pin!(sink.send_frame(b"hello-world"));
    poll_fn(|cx| {
        assert!(sending.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    assert!(connection.close());
    assert_eq!(sending.await, Err(VirtualBleError::LinkClosed));
    let mut output = [0xCC; 11];
    assert_eq!(
        source.recv_frame(&mut output).await,
        Err(VirtualBleError::LinkClosed)
    );
    assert_eq!(output, [0xCC; 11]);
}

#[tokio::test]
async fn invalid_frames_have_no_wire_effect_and_small_buffers_are_not_truncated(
) -> Result<(), Box<dyn Error>> {
    let (mut sink, mut source) = data_pair(10);
    assert_eq!(sink.send_frame(&[]).await, Err(VirtualBleError::EmptyFrame));
    assert_eq!(
        sink.send_frame(&[0; BLE_HW_MTU + 1]).await,
        Err(VirtualBleError::FrameTooLong {
            length: BLE_HW_MTU + 1,
            maximum: BLE_HW_MTU
        })
    );
    assert_eq!(source.receiver.len(), 0);
    let mut output = [0xCC; 10];
    let (sent, read) = tokio::join!(
        sink.send_frame(b"hello-world"),
        source.recv_frame(&mut output)
    );
    sent?;
    assert_eq!(
        read,
        Err(VirtualBleError::ReceiveBufferTooSmall {
            frame: 11,
            buffer: 10
        })
    );
    assert_eq!(output, [0xCC; 10]);
    Ok(())
}
