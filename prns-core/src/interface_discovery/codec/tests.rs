use super::*;

const PYTHON_BACKBONE: &str = "8b00b14261636b626f6e65496e7465726661636501c3ccfec41000112233445566778899aabbccddeeffccffaf5075626c6963204261636b626f6e6503cb402900000000000004cbc04120000000000005cb405ec0000000000002ae726f757465722e6578616d706c6506cd109207a46d65736808a6736563726574";

fn backbone() -> DiscoveryAdvertisement {
    DiscoveryAdvertisement {
        interface_type: AdvertisedInterfaceType::Backbone,
        transport: AdvertisedTransport::Enabled(TransportId::new([
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ])),
        name: Some(String::from("Public Backbone")),
        location: GeographicLocation {
            latitude: Some(12.5),
            longitude: Some(-34.25),
            height: Some(123.0),
        },
        details: AdvertisementDetails::Reachable {
            host: String::from("router.example"),
            port: 4242,
        },
        published_ifac: Some(PublishedIfac {
            network_name: Some(String::from("mesh")),
            passphrase: Some(String::from("secret")),
        }),
    }
}

fn bytes_from_hex(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(core::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn encoded_backbone() -> Vec<u8> {
    match encode_advertisement(&backbone()) {
        Ok(encoded) => encoded,
        Err(error) => panic!("unexpected encode error: {error}"),
    }
}

#[test]
fn python_rns_1_3_5_advertisement_is_byte_identical_and_decodes() {
    let packed = bytes_from_hex(PYTHON_BACKBONE);
    assert_eq!(encode_advertisement(&backbone()), Ok(packed.clone()));
    assert_eq!(decode_advertisement(&packed), Ok(backbone()));
}

#[test]
fn every_supported_detail_shape_round_trips() {
    let details = [
        (
            AdvertisedInterfaceType::TcpClient,
            AdvertisementDetails::None,
        ),
        (
            AdvertisedInterfaceType::I2p,
            AdvertisementDetails::I2p {
                address: String::from("peer.b32.i2p"),
            },
        ),
        (
            AdvertisedInterfaceType::RNode,
            AdvertisementDetails::RNode {
                frequency_hz: 867_200_000,
                bandwidth_hz: 125_000,
                spreading_factor: 9,
                coding_rate: 5,
            },
        ),
        (
            AdvertisedInterfaceType::Weave,
            AdvertisementDetails::Weave {
                frequency_hz: 915_000_000,
                bandwidth_hz: 250_000,
                channel: 3,
                modulation: String::from("GFSK"),
            },
        ),
        (
            AdvertisedInterfaceType::Kiss,
            AdvertisementDetails::Kiss {
                frequency_hz: 144_800_000,
                bandwidth_hz: 12_500,
                modulation: String::from("AFSK"),
            },
        ),
    ];
    for (interface_type, details) in details {
        let advertisement = DiscoveryAdvertisement {
            interface_type,
            transport: AdvertisedTransport::Disabled(TransportId::new([0x33; 16])),
            name: None,
            location: GeographicLocation::UNKNOWN,
            details,
            published_ifac: None,
        };
        let encoded = match encode_advertisement(&advertisement) {
            Ok(encoded) => encoded,
            Err(error) => panic!("unexpected encode error: {error}"),
        };
        assert_eq!(decode_advertisement(&encoded), Ok(advertisement));
    }
}

#[test]
fn mismatched_details_and_duplicate_known_fields_are_rejected() {
    let advertisement = DiscoveryAdvertisement {
        interface_type: AdvertisedInterfaceType::Backbone,
        transport: AdvertisedTransport::Disabled(TransportId::new([0x33; 16])),
        name: None,
        location: GeographicLocation::UNKNOWN,
        details: AdvertisementDetails::None,
        published_ifac: None,
    };
    assert_eq!(
        encode_advertisement(&advertisement),
        Err(DiscoveryEncodeError::DetailsDoNotMatchInterfaceType),
    );

    let mut duplicate = encoded_backbone();
    duplicate[0] = 0x8c;
    duplicate.push(INTERFACE_TYPE as u8);
    duplicate.push(0xb1);
    duplicate.extend_from_slice(b"BackboneInterface");
    assert_eq!(
        decode_advertisement(&duplicate),
        Err(DiscoveryDecodeError::DuplicateField(
            DiscoveryField::InterfaceType,
        )),
    );
}

#[test]
fn required_bool_is_not_coerced_from_an_integer() {
    let mut encoded = encoded_backbone();
    let transport_value = encoded
        .windows(2)
        .position(|pair| pair == [TRANSPORT as u8, 0xc3]);
    let Some(transport_value) = transport_value else {
        panic!("transport field not found");
    };
    encoded[transport_value + 1] = 1;
    assert_eq!(
        decode_advertisement(&encoded),
        Err(DiscoveryDecodeError::InvalidField(
            DiscoveryField::Transport,
        )),
    );
}

#[test]
fn independently_published_ifac_fields_match_rns_receiver_tolerance() {
    let mut advertisement = backbone();
    advertisement.published_ifac = None;
    let mut encoded = match encode_advertisement(&advertisement) {
        Ok(encoded) => encoded,
        Err(error) => panic!("unexpected encode error: {error}"),
    };
    encoded[0] += 1;
    encoded.extend_from_slice(&[IFAC_NETNAME as u8, 0xa4, b'm', b'e', b's', b'h']);
    advertisement.published_ifac = Some(PublishedIfac {
        network_name: Some(String::from("mesh")),
        passphrase: None,
    });
    assert_eq!(decode_advertisement(&encoded), Ok(advertisement));
}

#[test]
fn trailing_data_and_unknown_nested_values_are_handled_explicitly() {
    let mut trailing = encoded_backbone();
    trailing.push(0);
    assert_eq!(
        decode_advertisement(&trailing),
        Err(DiscoveryDecodeError::TrailingData),
    );

    let mut extended = encoded_backbone();
    extended[0] = 0x8d;
    extended.extend_from_slice(&[0x20, 0x91, 0x81, 0xa1, b'x', 0xc3]);
    extended.extend_from_slice(&[0xd0, 0xff, 0xc0]);
    assert_eq!(decode_advertisement(&extended), Ok(backbone()));
}

#[test]
fn plaintext_and_encrypted_envelopes_preserve_their_boundaries() {
    let packed = encoded_backbone();
    let stamp = [0x5a; STAMP_SIZE];
    let plaintext = encode_plaintext_envelope(&packed, &stamp);
    assert_eq!(
        decode_envelope(&plaintext),
        Ok(DiscoveryEnvelope {
            signed: false,
            body: DiscoveryEnvelopeBody::Plaintext {
                packed_advertisement: &packed,
                stamp: &stamp,
            },
        }),
    );

    let ciphertext = [0xa5; 96];
    let encrypted = encode_encrypted_envelope(&ciphertext);
    assert_eq!(
        decode_envelope(&encrypted),
        Ok(DiscoveryEnvelope {
            signed: false,
            body: DiscoveryEnvelopeBody::Encrypted {
                ciphertext: &ciphertext,
            },
        }),
    );
}

#[test]
fn envelope_rejects_missing_plaintext_without_rejecting_opaque_encrypted_data() {
    assert_eq!(
        decode_envelope(&[]),
        Err(DiscoveryEnvelopeError::MissingFlags),
    );
    assert_eq!(
        decode_envelope(&[0]),
        Err(DiscoveryEnvelopeError::MissingPlaintextOrStamp),
    );
    assert_eq!(
        decode_envelope(&[FLAG_ENCRYPTED]),
        Ok(DiscoveryEnvelope {
            signed: false,
            body: DiscoveryEnvelopeBody::Encrypted { ciphertext: &[] },
        }),
    );
}
