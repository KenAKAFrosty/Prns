use super::*;
use crate::interfaces::bluetooth_auto::{BLE_HW_MTU, BLE_WIRE_FRAME_LEN};

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
