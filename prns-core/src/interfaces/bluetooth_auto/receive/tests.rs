use super::*;
use crate::interfaces::bluetooth_auto::{BLE_HW_MTU, BLE_WIRE_FRAME_LEN};
use embassy_futures::block_on;

#[derive(Debug, PartialEq, Eq)]
struct SourceFailure(u8);

struct ReportingSource(Result<usize, SourceFailure>);

impl BleSource for ReportingSource {
    type Error = SourceFailure;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, Self::Error> {
        out.fill(0x17);
        match &self.0 {
            Ok(length) => Ok(*length),
            Err(SourceFailure(code)) => Err(SourceFailure(*code)),
        }
    }
}

#[test]
fn checked_receive_exposes_only_whole_valid_frames_and_preserves_source_errors() {
    const CANARY: u8 = 0xA5;
    for length in (0..=BLE_WIRE_FRAME_LEN + 1).chain([usize::MAX]) {
        for capacity in [0, 1, BLE_HW_MTU, BLE_WIRE_FRAME_LEN - 1, BLE_WIRE_FRAME_LEN] {
            let mut buffer = [CANARY; BLE_WIRE_FRAME_LEN + 1];
            let mut expected_buffer = buffer;
            expected_buffer[..capacity].fill(0x17);
            let mut source = ReportingSource(Ok(length));
            let result =
                block_on(receive_frame(&mut source, &mut buffer[..capacity])).map(<[u8]>::to_vec);
            let expected = if length <= capacity {
                Ok(vec![0x17; length])
            } else {
                Err(BleFrameReceiveError::Length(
                    BleReceiveError::BufferTooSmall { length, capacity },
                ))
            };
            assert_eq!((result, buffer), (expected, expected_buffer));
        }
    }
    let mut source = ReportingSource(Err(SourceFailure(9)));
    let mut buffer = [CANARY; 3];
    assert_eq!(
        block_on(receive_frame(&mut source, &mut buffer)),
        Err(BleFrameReceiveError::Source(SourceFailure(9)))
    );
    assert_eq!(buffer, [0x17; 3]);
}

struct ResumableSource {
    consumed_prefix: bool,
}

impl BleSource for ResumableSource {
    type Error = core::convert::Infallible;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, Self::Error> {
        if !self.consumed_prefix {
            self.consumed_prefix = true;
            core::future::pending::<()>().await;
        }
        out[..3].copy_from_slice(b"abc");
        Ok(3)
    }
}

#[test]
fn checked_receive_cancellation_preserves_the_sources_partial_progress() {
    use core::future::Future;
    use core::task::{Context, Waker};

    let mut source = ResumableSource {
        consumed_prefix: false,
    };
    let mut buffer = [0xA5; 4];
    {
        let mut receive = core::pin::pin!(receive_frame(&mut source, &mut buffer));
        assert!(receive
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending());
    }
    assert!(source.consumed_prefix);
    assert_eq!(
        block_on(receive_frame(&mut source, &mut buffer)),
        Ok(b"abc".as_slice())
    );
    assert_eq!(buffer, [b'a', b'b', b'c', 0xA5]);
}

#[test]
fn all_frame_lengths_through_authentication_headroom_are_copied_whole_or_refused() {
    const CANARY: u8 = 0xA5;
    const PAYLOAD: u8 = 0x17;
    let frame = [PAYLOAD; BLE_WIRE_FRAME_LEN + 2];
    for length in 0..=frame.len() {
        for capacity in [0, 1, BLE_HW_MTU, BLE_WIRE_FRAME_LEN - 1, BLE_WIRE_FRAME_LEN] {
            let mut out = [CANARY; BLE_WIRE_FRAME_LEN + 2];
            let mut expected = out;
            let outcome = if length > capacity {
                Err(BleReceiveError::BufferTooSmall { length, capacity })
            } else {
                expected[..length].fill(PAYLOAD);
                Ok(length)
            };
            assert_eq!(
                (
                    copy_received_frame(&frame[..length], &mut out[..capacity]),
                    out
                ),
                (outcome, expected)
            );
        }
    }
}

#[test]
fn stream_length_validation_refuses_oversize_prefixes_without_requiring_the_body() {
    for length in [BLE_WIRE_FRAME_LEN + 1, u16::MAX as usize, usize::MAX] {
        assert_eq!(
            validate_received_frame_length(length, BLE_WIRE_FRAME_LEN),
            Err(BleReceiveError::BufferTooSmall {
                length,
                capacity: BLE_WIRE_FRAME_LEN,
            })
        );
    }
    assert_eq!(
        validate_received_frame_length(BLE_WIRE_FRAME_LEN, BLE_HW_MTU),
        Err(BleReceiveError::BufferTooSmall {
            length: BLE_WIRE_FRAME_LEN,
            capacity: BLE_HW_MTU,
        })
    );
    assert_eq!(
        validate_received_frame_length(BLE_WIRE_FRAME_LEN, BLE_WIRE_FRAME_LEN),
        Ok(())
    );
}

#[test]
fn ble_wire_capacity_matches_the_manifold_packet_and_ifac_contract() {
    let descriptor = crate::interfaces::bluetooth_auto::descriptor(
        crate::interfaces::InterfaceId::new([1; 8]),
        crate::interfaces::bluetooth_auto::BLE_BITRATE_GUESS_BPS,
    );
    assert_eq!(
        BLE_WIRE_FRAME_LEN,
        crate::interfaces::frame_cap_for(&descriptor)
    );
}

#[test]
fn checked_stream_receive_rejects_a_prefix_early_and_preserves_complete_frames_for_retry() {
    use crate::interfaces::bluetooth_auto::StreamDeframer;
    let mut stream = StreamDeframer::<16>::new();
    let mut small = [0xA5; 2];
    assert!(stream.absorb(&[0]));
    assert_eq!(stream.next_frame_checked(&mut small), Ok(None));
    assert!(stream.absorb(&[3]));
    let refused = Err(BleReceiveError::BufferTooSmall {
        length: 3,
        capacity: 2,
    });
    assert_eq!(stream.next_frame_checked(&mut small), refused);
    assert_eq!(small, [0xA5; 2]);
    assert!(stream.absorb(&[1, 2, 3, 0, 1, 9]));
    assert_eq!(stream.next_frame_checked(&mut small), refused);
    let mut enough = [0xA5; 4];
    assert_eq!(stream.next_frame_checked(&mut enough), Ok(Some(3)));
    assert_eq!(enough, [1, 2, 3, 0xA5]);
    assert_eq!(stream.next_frame_checked(&mut small), Ok(Some(1)));
    assert_eq!(small, [9, 0xA5]);
    assert_eq!(stream.next_frame_checked(&mut small), Ok(None));
}
