use alloc::vec::Vec;

use crate::wire::DestinationHash;
use proptest::prelude::*;

use super::path::{DestinationSelection, HopSelection};
use super::*;

#[test]
fn status_request_matches_python_truth_equality() {
    assert_eq!(
        RnsRemoteStatusRequest::InterfaceStats
            .encode_message_pack()
            .unwrap(),
        [0x91, 0xc2]
    );
    assert_eq!(
        RnsRemoteStatusRequest::InterfaceStatsAndLinkCount
            .encode_message_pack()
            .unwrap(),
        [0x91, 0xc3]
    );
    assert_eq!(
        decode_remote_status_request(&[0x91, 0xc3]),
        Ok(RnsRemoteStatusRequest::InterfaceStatsAndLinkCount)
    );
    assert_eq!(
        decode_remote_status_request(&[0x91, 0x01]),
        Ok(RnsRemoteStatusRequest::InterfaceStatsAndLinkCount)
    );
    assert_eq!(
        decode_remote_status_request(&[0x91, 0xcb, 0x3f, 0xf0, 0, 0, 0, 0, 0, 0]),
        Ok(RnsRemoteStatusRequest::InterfaceStatsAndLinkCount)
    );
    assert_eq!(
        decode_remote_status_request(&[0x91, 0xc2]),
        Ok(RnsRemoteStatusRequest::InterfaceStats)
    );
    assert_eq!(
        decode_remote_status_request(&[0x90]),
        Err(RnsRemoteRequestDecodeError::InvalidShape)
    );
}

#[test]
fn path_request_decodes_stock_table_and_rate_shapes() {
    let destination = DestinationHash::new([0x42; 16]);
    let table = bytes_from_hex("93a57461626c65c4104242424242424242424242424242424203");
    assert_eq!(
        decode_remote_path_request(&table),
        Ok(RnsRemotePathRequest::Table(RnsRemotePathTableRequest {
            destination: DestinationSelection::Exact(destination),
            hops: HopSelection::AtMost(3),
        }))
    );
    let rates = bytes_from_hex("92a57261746573c0");
    assert_eq!(
        decode_remote_path_request(&rates),
        Ok(RnsRemotePathRequest::Rates(RnsRemoteRateTableRequest {
            destination: DestinationSelection::All,
        }))
    );
}

#[test]
fn malformed_or_trailing_values_fail_without_building_a_value_tree() {
    assert_eq!(
        decode_remote_path_request(&[0x91, 0xa5, b't', b'a']),
        Err(RnsRemoteRequestDecodeError::InvalidMessagePack)
    );
    assert_eq!(
        decode_remote_status_request(&[0x91, 0xc2, 0x00]),
        Err(RnsRemoteRequestDecodeError::InvalidMessagePack)
    );
    assert_eq!(
        decode_remote_path_request(&[0x91, 0xa4, b'n', b'o', b'p', b'e']),
        Err(RnsRemoteRequestDecodeError::UnsupportedCommand)
    );
    assert_eq!(
        decode_remote_path_request(&[0x92, 0x01, 0x91]),
        Err(RnsRemoteRequestDecodeError::InvalidMessagePack)
    );
    assert_eq!(
        decode_remote_path_request(&[0x94, 0xa5, b't', b'a', b'b', b'l', b'e', 0xc0, 0xc3, 0x91,]),
        Err(RnsRemoteRequestDecodeError::InvalidMessagePack)
    );
}

#[test]
fn rates_ignore_the_optional_hop_slot_just_like_the_reference() {
    let request = decode_remote_path_request(&[
        0x93, 0xa5, b'r', b'a', b't', b'e', b's', 0xc0, 0x81, 0xa1, b'x', 0x01,
    ]);
    assert_eq!(
        request,
        Ok(RnsRemotePathRequest::Rates(RnsRemoteRateTableRequest {
            destination: DestinationSelection::All,
        }))
    );
}

proptest! {
    #[test]
    fn arbitrary_remote_management_input_is_a_total_decode(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        let _ = decode_remote_status_request(&bytes);
        let _ = decode_remote_path_request(&bytes);
    }
}

fn bytes_from_hex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let Some(high) = hex_digit(pair[0]) else {
                panic!("test fixture contains non-hex input");
            };
            let Some(low) = hex_digit(pair[1]) else {
                panic!("test fixture contains non-hex input");
            };
            high << 4 | low
        })
        .collect()
}

const fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}
