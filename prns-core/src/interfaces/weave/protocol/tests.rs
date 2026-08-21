#![allow(clippy::unwrap_used)]

use super::{
    DeviceEvent, EndpointId, MultipathDeduplicator, SwitchId, WeaveHostIdentity, BROADCAST_SWITCH,
};
use crate::interfaces::rns_serial_framing::{self, RnsSerialDecoder};
use crate::interfaces::weave::{WDCL_MAX_CHUNK, WEAVE_MAX_WIRE_PACKET};

fn decoded(encoded: &[u8]) -> Vec<u8> {
    let mut decoder = RnsSerialDecoder::<WDCL_MAX_CHUNK>::new();
    let mut frames = Vec::new();
    decoder.feed_slice(encoded, |frame| frames.push(frame.to_vec()));
    assert_eq!(frames.len(), 1);
    frames.remove(0)
}

fn from_hex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16).unwrap() as u8;
            let low = (pair[1] as char).to_digit(16).unwrap() as u8;
            (high << 4) | low
        })
        .collect()
}

#[test]
fn discovery_broadcast_contains_the_host_switch_id() {
    let identity = WeaveHostIdentity::from_signing_secret([0x11; 32]);
    let mut output = [0u8; 64];
    let written = super::encode_discovery(&identity, &mut output).unwrap();
    let frame = decoded(&output[..written]);
    assert_eq!(&frame[..SwitchId::LEN], BROADCAST_SWITCH.as_bytes());
    assert_eq!(frame[SwitchId::LEN], super::TYPE_DISCOVER);
    assert_eq!(&frame[SwitchId::LEN + 1..], identity.switch_id().as_bytes());
    assert_eq!(
        &output[..written],
        from_hex("7effffffff00c97787377e").as_slice()
    );
}

#[test]
fn handshake_matches_the_rns_1_4_2_python_oracle() {
    let identity = WeaveHostIdentity::from_signing_secret([0x11; 32]);
    let remote = SwitchId::new([0x34, 0x55, 0xa4, 0xf0]);
    let mut output = [0u8; 256];
    let written = super::encode_handshake(&identity, remote, &mut output).unwrap();
    assert_eq!(
        &output[..written],
        from_hex(
            "7e3455a4f001d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c977873714b47546507ad58f938b7bfbcc6c077aab9fb0bd0c62eb5b1d89f6f9c4e34735f9b40e7d5e4cdfbd0778c5d5ca766fdc7703c21053c77c21be3dae1c569f12760b7e"
        )
        .as_slice()
    );
}

#[test]
fn discovery_response_verification_matches_the_rns_1_4_2_python_oracle() {
    let frame = from_hex(
        "c977873700a09aa5f47a6759802ff955f8dc2d2a14a5c99d23be97f864127ff9383455a4f0d6d59bd10d8d43ad27e2c66ffe4be08f4c15408d75698ec87dfcf7e68430efac79a655716f2ea93a60cdf1cc0d6bc2d74310bf067c52d1b6405bdff9f9998a08",
    );
    assert_eq!(
        super::decode_device_frame(&frame, SwitchId::new([0xc9, 0x77, 0x87, 0x37])),
        Ok(DeviceEvent::Discovered {
            switch_id: SwitchId::new([0x34, 0x55, 0xa4, 0xf0]),
            signing_public_key: crate::crypto::Ed25519PublicKey([
                0xa0, 0x9a, 0xa5, 0xf4, 0x7a, 0x67, 0x59, 0x80, 0x2f, 0xf9, 0x55, 0xf8, 0xdc, 0x2d,
                0x2a, 0x14, 0xa5, 0xc9, 0x9d, 0x23, 0xbe, 0x97, 0xf8, 0x64, 0x12, 0x7f, 0xf9, 0x38,
                0x34, 0x55, 0xa4, 0xf0,
            ]),
        })
    );
    let device = WeaveHostIdentity::from_signing_secret([0x22; 32]);
    let mut encoded = [0u8; 256];
    let written = super::encode_discovery_response(
        &device,
        SwitchId::new([0xc9, 0x77, 0x87, 0x37]),
        &mut encoded,
    )
    .unwrap();
    assert_eq!(decoded(&encoded[..written]), frame);
}

#[test]
fn discovery_rejects_a_signature_for_a_different_host_switch() {
    let device = WeaveHostIdentity::from_signing_secret([0x22; 32]);
    let mut encoded = [0u8; 256];
    let written = super::encode_discovery_response(
        &device,
        SwitchId::new([0xc9, 0x77, 0x87, 0x37]),
        &mut encoded,
    )
    .unwrap();
    let mut frame = decoded(&encoded[..written]);
    frame[0] ^= 1;

    assert_eq!(
        super::decode_device_frame(&frame, SwitchId::new([0xc9, 0x77, 0x87, 0x37])),
        Err(super::DecodeError::InvalidDiscoverySignature)
    );
}

#[test]
fn endpoint_command_carries_the_typed_destination_and_packet() {
    let switch = SwitchId::new([1, 2, 3, 4]);
    let endpoint = EndpointId::new([5, 6, 7, 8, 9, 10, 11, 12]);
    let mut raw = [0u8; 64];
    let mut output = [0u8; 128];
    let written =
        super::encode_endpoint_packet(switch, endpoint, b"packet", &mut raw, &mut output).unwrap();
    let frame = decoded(&output[..written]);
    assert_eq!(&frame[..4], switch.as_bytes());
    assert_eq!(frame[4], super::TYPE_COMMAND);
    assert_eq!(&frame[5..7], &super::COMMAND_ENDPOINT_PACKET.to_be_bytes());
    assert_eq!(&frame[7..15], endpoint.as_bytes());
    assert_eq!(&frame[15..], b"packet");
}

#[test]
fn endpoint_command_retains_ifac_headroom_beyond_the_fixed_mtu() {
    let payload = vec![0x42; WEAVE_MAX_WIRE_PACKET];
    let mut raw = vec![0u8; SwitchId::LEN + 1 + 2 + EndpointId::LEN + payload.len()];
    let mut output = vec![0u8; rns_serial_framing::max_encoded_len(raw.len())];
    let written = super::encode_endpoint_packet(
        SwitchId::new([1, 2, 3, 4]),
        EndpointId::new([5, 6, 7, 8, 9, 10, 11, 12]),
        &payload,
        &mut raw,
        &mut output,
    )
    .unwrap();
    let frame = decoded(&output[..written]);

    assert_eq!(&frame[15..], payload);
}

#[test]
fn endpoint_packet_exposes_source_and_payload_without_copying() {
    let source = EndpointId::new([0x22; 8]);
    let mut frame = Vec::from([0x11, 0x12, 0x13, 0x14, super::TYPE_ENDPOINT_PACKET]);
    frame.extend_from_slice(b"packet");
    frame.extend_from_slice(source.as_bytes());
    assert_eq!(
        super::decode_device_frame(&frame, SwitchId::new([0x11, 0x12, 0x13, 0x14])),
        Ok(DeviceEvent::EndpointPacket {
            source,
            payload: b"packet",
        })
    );
}

#[test]
fn endpoint_packet_for_another_switch_is_not_delivered() {
    let source = EndpointId::new([0x22; 8]);
    let mut frame = Vec::from([0x11, 0x12, 0x13, 0x14, super::TYPE_ENDPOINT_PACKET]);
    frame.extend_from_slice(b"packet");
    frame.extend_from_slice(source.as_bytes());

    assert_eq!(
        super::decode_device_frame(&frame, SwitchId::new([0x21, 0x22, 0x23, 0x24])),
        Ok(DeviceEvent::Ignored)
    );
}

#[test]
fn multipath_duplicate_expires_at_the_reference_deadline() {
    let mut deduplicator = MultipathDeduplicator::new();
    assert!(deduplicator.accepts(b"packet", 1_000));
    assert!(!deduplicator.accepts(b"packet", 1_749));
    assert!(deduplicator.accepts(b"packet", 1_750));
    assert!(deduplicator.accepts(b"different", 1_750));
}
