use super::*;
use crate::interfaces::usb_auto::UsbControllerEnrollment;
use crate::remote_control::{RemoteControlControllerAuthority, RemoteControlRequestSet};

fn hex(value: &str) -> std::vec::Vec<u8> {
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let digit = |byte| char::from(byte).to_digit(16).unwrap() as u8;
            (digit(pair[0]) << 4) | digit(pair[1])
        })
        .collect()
}

#[test]
fn serial_enrollment_matches_shared_cross_client_vectors() {
    let vectors: serde_json::Value =
        serde_json::from_str(include_str!("enrollment-vectors.json")).unwrap();
    for vector in vectors["vectors"].as_array().unwrap() {
        let payload = hex(vector["payload_hex"].as_str().unwrap());
        let framed = hex(vector["framed_hex"].as_str().unwrap());
        let message = decode_message(&payload).unwrap();
        let mut output = [0; MAX_FRAMED_BYTES];
        let len = message.write_framed(&mut output).unwrap();
        assert_eq!(&output[..len], framed, "{}", vector["name"]);
        let mut decoder = Decoder::new();
        let mut count = 0;
        for byte in framed {
            if let Some(frame) = decoder.feed(byte).unwrap() {
                if !frame.is_empty() {
                    assert_eq!(frame, payload);
                    count += 1;
                }
            }
        }
        assert_eq!(count, 1);
        if matches!(
            message,
            Message::ControllerEnroll(_)
                | Message::ControllerEnrollmentStatusQuery { .. }
                | Message::ControllerEnrollmentStatus { .. }
        ) {
            assert!(matches!(react_to(Ok(message)), InboundReaction::Ignore));
            assert!(matches!(host_react(Ok(message)), HostInbound::Ignore));
        }
    }
    let capability = Capabilities::none().with_controller_enrollment();
    assert!(capability.supports_controller_enrollment());
    assert!(!capability.supports_host_lane());
    assert!(!Capabilities::none().supports_controller_enrollment());
    const { assert!(MAX_MESSAGE_BYTES > SERIAL_CONTROLLER_ENROLL_REQUEST_BYTES) };
}

#[test]
fn enrollment_control_lengths_and_nonce_are_exact() {
    for (kind, body_len) in [
        (4, SERIAL_CONTROLLER_ENROLL_REQUEST_BYTES),
        (5, 4),
        (6, SERIAL_CONTROLLER_ENROLL_STATUS_BYTES),
    ] {
        for len in 0..body_len {
            let mut payload = std::vec![kind];
            payload.extend(std::iter::repeat_n(0, len));
            assert_eq!(
                decode_message(&payload),
                Err(MalformedMessage::InvalidEnrollmentLength)
            );
        }
        let mut oversized = std::vec![0; body_len + 2];
        oversized[0] = kind;
        assert_eq!(
            decode_message(&oversized),
            Err(MalformedMessage::InvalidEnrollmentLength)
        );
    }
    assert_eq!(
        decode_message(&[5, 0, 0, 0, 0]),
        Err(MalformedMessage::InvalidEnrollmentNonce)
    );
    assert_eq!(
        Message::ControllerEnrollmentStatusQuery { nonce: 0 }.write_payload(&mut [0; 5]),
        Err(WriteError::InvalidEnrollmentNonce)
    );
    let mut reply = [0; 1 + SERIAL_CONTROLLER_ENROLL_STATUS_BYTES];
    reply[0] = 6;
    reply[5] = 4;
    assert_eq!(
        decode_message(&reply),
        Err(MalformedMessage::InvalidEnrollmentStatus)
    );
    reply[5] = 0;
    reply[6] = 1;
    assert_eq!(
        decode_message(&reply),
        Err(MalformedMessage::InvalidEnrollmentStatus)
    );
}

#[test]
fn serial_authorization_binds_the_full_target_and_validates_controller_keys() {
    let vectors: serde_json::Value =
        serde_json::from_str(include_str!("enrollment-vectors.json")).unwrap();
    let target: [u8; 64] = hex(vectors["target_public_key_hex"].as_str().unwrap())
        .try_into()
        .unwrap();
    let controller = hex(vectors["controller_public_key_hex"].as_str().unwrap());
    let mut request = [0; SERIAL_CONTROLLER_ENROLL_REQUEST_BYTES];
    request[..4].copy_from_slice(&0x7d7e1234u32.to_le_bytes());
    request[4..68].copy_from_slice(&controller);
    request[68..].copy_from_slice(&target);
    let parsed = UsbControllerEnrollment::decode_serial(&request, &target).unwrap();
    assert_eq!(parsed.transaction, 0x7d7e1234);
    assert_eq!(
        parsed.grant.authority(),
        RemoteControlControllerAuthority::Administrator
    );
    assert_eq!(
        *parsed.grant.permitted_requests(),
        RemoteControlRequestSet::all()
    );
    for index in 0..64 {
        let mut other = target;
        other[index] ^= 1;
        assert!(UsbControllerEnrollment::decode_serial(&request, &other).is_none());
    }
    assert!(UsbControllerEnrollment::decode_serial(&request[..131], &target).is_none());
    request[4..36].fill(0); // Non-contributory X25519 identity.
    assert!(UsbControllerEnrollment::decode_serial(&request, &target).is_none());
    request[4..68].copy_from_slice(&controller);
    request[36..68].fill(0); // Weak Ed25519 identity.
    assert!(UsbControllerEnrollment::decode_serial(&request, &target).is_none());
}
