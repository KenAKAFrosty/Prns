#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used
)]

use super::*;

// Public keys from RFC 7748 and RFC 8032. The literal interchange vectors are
// independent of this codec's encoder and usable by other controller clients.
const CONTROLLER_KEY_HEX: &str = concat!(
    "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a",
    "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
);
const TARGET_KEY_HEX: &str = concat!(
    "de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f",
    "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c"
);
const CONTROLLER_VECTOR: &str = concat!(
    "prns-rc:5243500101",
    "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a",
    "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
);
const ACCESS_VECTOR: &str = concat!(
    "prns-rc:5243500102",
    "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a",
    "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
    "de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f",
    "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
    "0103010c23"
);

fn keys(hex: &str) -> IdentityPublicKeys {
    let mut bytes = [0; IDENTITY_PUBLIC_KEY_LEN];
    for (pair, byte) in hex.as_bytes().as_chunks::<2>().0.iter().zip(&mut bytes) {
        let digit = |value: u8| char::from(value).to_digit(16).unwrap() as u8;
        *byte = (digit(pair[0]) << 4) | digit(pair[1]);
    }
    PublicIdentityMaterial::from_bytes(bytes).public_keys()
}

fn controller() -> RemoteControlControllerIdentity {
    RemoteControlControllerIdentity::new(keys(CONTROLLER_KEY_HEX))
}

fn access(
    authority: RemoteControlControllerAuthority,
    requests: RemoteControlRequestSet,
) -> RemoteControlTargetAccessCard {
    RemoteControlTargetAccessCard::new(
        controller(),
        RemoteControlTargetAccess::new(
            RemoteControlTargetIdentity::new(keys(TARGET_KEY_HEX)),
            authority,
            requests,
        )
        .unwrap(),
    )
    .unwrap()
}

fn operator_card() -> RemoteControlTargetAccessCard {
    RemoteControlTargetAccessCard::decode_text(ACCESS_VECTOR).unwrap()
}

fn binary(card: &RemoteControlTargetAccessCard) -> std::vec::Vec<u8> {
    let mut output = std::vec![0; card.encoded_len()];
    assert_eq!(card.encode(&mut output), Ok(output.len()));
    output
}

#[test]
fn controller_and_target_cards_match_independent_text_vectors() {
    let card = RemoteControlControllerCard::new(controller()).unwrap();
    let mut text = [0; RemoteControlControllerCard::MAX_TEXT_LEN];
    let len = card.encode_text(&mut text).unwrap();
    assert_eq!(&text[..len], CONTROLLER_VECTOR.as_bytes());
    assert_eq!(
        RemoteControlControllerCard::decode_text(CONTROLLER_VECTOR),
        Ok(card)
    );
    assert_eq!(card.encoded_len(), 69);
    assert_eq!(card.text_len(), 146);

    let mut requests = RemoteControlRequestSet::only(RemoteControlRequestKind::InspectRadio);
    assert!(requests.insert(RemoteControlRequestKind::Describe));
    assert!(requests.insert(RemoteControlRequestKind::DescribeBuild));
    let card = access(RemoteControlControllerAuthority::Operator, requests);
    let mut text = [0; RemoteControlTargetAccessCard::MAX_TEXT_LEN];
    let len = card.encode_text(&mut text).unwrap();
    assert_eq!(&text[..len], ACCESS_VECTOR.as_bytes());
    assert_eq!(
        RemoteControlTargetAccessCard::decode_text(ACCESS_VECTOR),
        Ok(card)
    );
}

#[test]
fn target_access_requires_the_full_intended_recipient_key() {
    let card = operator_card();
    assert_eq!(card.recipient(), &controller());
    let access = card.into_access_for(&controller()).unwrap();
    assert_eq!(access.target().public_keys(), &keys(TARGET_KEY_HEX));

    let mut changed = controller().public_keys().public_key_bytes();
    // Retaining the signing key is insufficient; the entire identity is bound.
    changed[..32].copy_from_slice(&keys(TARGET_KEY_HEX).public_key_bytes()[..32]);
    let other = RemoteControlControllerIdentity::new(
        PublicIdentityMaterial::from_bytes(changed).public_keys(),
    );
    assert_eq!(
        operator_card().into_access_for(&other),
        Err(RemoteControlPublicCardError::RecipientMismatch)
    );
}

#[test]
fn administrator_round_trip_preserves_explicit_requests_without_expansion() {
    let card = access(
        RemoteControlControllerAuthority::Administrator,
        RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
    );
    assert!(card
        .access()
        .permits(RemoteControlRequestKind::AuthorizeController));
    let restored = RemoteControlTargetAccessCard::decode(&binary(&card)).unwrap();
    assert_eq!(
        restored.access().authority(),
        RemoteControlControllerAuthority::Administrator
    );
    assert_eq!(restored.access().permitted_requests().len(), 1);
    assert!(restored
        .access()
        .permits(RemoteControlRequestKind::AuthorizeController));
    assert!(!restored
        .access()
        .permits(RemoteControlRequestKind::ConfigureRadio));
    assert_eq!(restored, card);
}

#[test]
fn maximal_known_request_set_fits_declared_bounds() {
    let card = access(
        RemoteControlControllerAuthority::Administrator,
        RemoteControlRequestSet::all(),
    );
    assert_eq!(
        card.encoded_len(),
        RemoteControlTargetAccessCard::MAX_ENCODED_LEN
    );
    assert_eq!(card.text_len(), RemoteControlTargetAccessCard::MAX_TEXT_LEN);
    let mut output = [0; RemoteControlTargetAccessCard::MAX_TEXT_LEN];
    let len = card.encode_text(&mut output).unwrap();
    assert_eq!(len, output.len());
    assert_eq!(
        RemoteControlTargetAccessCard::decode_text(core::str::from_utf8(&output).unwrap()),
        Ok(card)
    );
}

#[test]
fn short_output_is_rejected_without_partial_writes_and_tail_is_untouched() {
    let card = operator_card();
    let mut short = std::vec![0xab; card.encoded_len() - 1];
    assert_eq!(
        card.encode(&mut short),
        Err(RemoteControlPublicCardError::BufferTooShort)
    );
    assert!(short.iter().all(|byte| *byte == 0xab));
    let mut short_text = std::vec![0xab; card.text_len() - 1];
    assert_eq!(
        card.encode_text(&mut short_text),
        Err(RemoteControlPublicCardError::BufferTooShort)
    );
    assert!(short_text.iter().all(|byte| *byte == 0xab));
    let mut longer = std::vec![0xab; card.text_len() + 4];
    let len = card.encode_text(&mut longer).unwrap();
    assert_eq!(&longer[len..], &[0xab; 4]);

    let card = RemoteControlControllerCard::new(controller()).unwrap();
    let mut short = [0xab; RemoteControlControllerCard::MAX_ENCODED_LEN - 1];
    assert_eq!(
        card.encode(&mut short),
        Err(RemoteControlPublicCardError::BufferTooShort)
    );
    assert!(short.iter().all(|byte| *byte == 0xab));
    let mut short_text = [0xab; RemoteControlControllerCard::MAX_TEXT_LEN - 1];
    assert_eq!(
        card.encode_text(&mut short_text),
        Err(RemoteControlPublicCardError::BufferTooShort)
    );
    assert!(short_text.iter().all(|byte| *byte == 0xab));
}

#[test]
fn truncation_and_trailing_data_are_never_accepted() {
    let bytes = binary(&operator_card());
    for len in 0..bytes.len() {
        assert!(RemoteControlTargetAccessCard::decode(&bytes[..len]).is_err());
    }
    let mut extra = bytes;
    extra.push(0);
    assert_eq!(
        RemoteControlTargetAccessCard::decode(&extra),
        Err(RemoteControlPublicCardError::InvalidLength)
    );
    let card = RemoteControlControllerCard::new(controller()).unwrap();
    let mut bytes = [0; RemoteControlControllerCard::MAX_ENCODED_LEN];
    card.encode(&mut bytes).unwrap();
    for len in 0..bytes.len() {
        assert_eq!(
            RemoteControlControllerCard::decode(&bytes[..len]),
            Err(RemoteControlPublicCardError::InvalidLength)
        );
    }
    let mut extra = bytes.to_vec();
    extra.push(0);
    assert_eq!(
        RemoteControlControllerCard::decode(&extra),
        Err(RemoteControlPublicCardError::InvalidLength)
    );
}

#[test]
fn headers_and_text_do_not_accept_a_different_format_or_version() {
    for (offset, value, expected) in [
        (0, b'X', RemoteControlPublicCardError::InvalidMagic),
        (
            3,
            2,
            RemoteControlPublicCardError::UnsupportedVersion { found: 2 },
        ),
        (
            4,
            CONTROLLER_ROLE,
            RemoteControlPublicCardError::UnexpectedRole {
                found: CONTROLLER_ROLE,
            },
        ),
        (
            4,
            99,
            RemoteControlPublicCardError::UnexpectedRole { found: 99 },
        ),
    ] {
        let mut bytes = binary(&operator_card());
        bytes[offset] = value;
        assert_eq!(RemoteControlTargetAccessCard::decode(&bytes), Err(expected));
    }
    assert!(RemoteControlControllerCard::decode_text(ACCESS_VECTOR).is_err());
    assert!(RemoteControlTargetAccessCard::decode_text(CONTROLLER_VECTOR).is_err());
    for text in [
        "",
        "prns://rc",
        "prns-rc:",
        "prns-rc:0",
        "prns-rc:zz",
        " prns-rc:5243500101",
    ] {
        assert!(RemoteControlControllerCard::decode_text(text).is_err());
        assert!(RemoteControlTargetAccessCard::decode_text(text).is_err());
    }
    for suffix in [" ", "\n", "00", "é"] {
        assert!(RemoteControlControllerCard::decode_text(&std::format!(
            "{CONTROLLER_VECTOR}{suffix}"
        ))
        .is_err());
        assert!(RemoteControlTargetAccessCard::decode_text(&std::format!(
            "{ACCESS_VECTOR}{suffix}"
        ))
        .is_err());
    }
    let uppercase = std::format!("prns-rc:{}", CONTROLLER_VECTOR[8..].to_ascii_uppercase());
    assert_eq!(
        RemoteControlControllerCard::decode_text(&uppercase)
            .unwrap()
            .controller(),
        &controller()
    );
}

#[test]
fn unknown_duplicate_out_of_order_and_authority_incompatible_requests_fail() {
    const AUTHORITY: usize = HEADER_LEN + 2 * IDENTITY_PUBLIC_KEY_LEN;
    const COUNT: usize = AUTHORITY + 1;
    const REQUESTS: usize = COUNT + 1;
    for (offset, value, expected) in [
        (
            AUTHORITY,
            99,
            RemoteControlPublicCardError::UnknownAuthority { found: 99 },
        ),
        (COUNT, 0, RemoteControlPublicCardError::InvalidLength),
        (COUNT, 255, RemoteControlPublicCardError::InvalidLength),
        (
            REQUESTS,
            0xff,
            RemoteControlPublicCardError::UnknownRequest { found: 0xff },
        ),
        (
            REQUESTS + 1,
            1,
            RemoteControlPublicCardError::DuplicateRequest {
                request: RemoteControlRequestKind::Describe,
            },
        ),
        (
            REQUESTS,
            0x23,
            RemoteControlPublicCardError::NonCanonicalRequestOrder,
        ),
        (
            REQUESTS + 1,
            0x0f,
            RemoteControlPublicCardError::InvalidTargetAccess(
                RemoteControlTargetAccessError::AdministratorRequestRequiresAuthority {
                    request: RemoteControlRequestKind::AuthorizeController,
                },
            ),
        ),
    ] {
        let mut bytes = binary(&operator_card());
        bytes[offset] = value;
        assert_eq!(RemoteControlTargetAccessCard::decode(&bytes), Err(expected));
    }
    let mut empty = binary(&operator_card());
    empty.truncate(REQUESTS);
    empty[COUNT] = 0;
    assert_eq!(
        RemoteControlTargetAccessCard::decode(&empty),
        Err(RemoteControlPublicCardError::InvalidTargetAccess(
            RemoteControlTargetAccessError::NoPermittedRequests
        ))
    );
}

#[test]
fn malformed_or_weak_keys_are_rejected_in_every_identity_field() {
    for (offset, role) in [
        (HEADER_LEN, RemoteControlPublicCardIdentityRole::Recipient),
        (
            HEADER_LEN + IDENTITY_PUBLIC_KEY_LEN,
            RemoteControlPublicCardIdentityRole::Target,
        ),
    ] {
        for (component_offset, component, malformed) in [
            (0, RemoteControlPublicCardKeyComponent::Encryption, [0; 32]),
            (32, RemoteControlPublicCardKeyComponent::Signing, [0; 32]),
            (32, RemoteControlPublicCardKeyComponent::Signing, {
                let mut bytes = [0; 32];
                bytes[1] = 3;
                bytes
            }),
        ] {
            let mut bytes = binary(&operator_card());
            let start = offset + component_offset;
            bytes[start..start + 32].copy_from_slice(&malformed);
            assert_eq!(
                RemoteControlTargetAccessCard::decode(&bytes),
                Err(RemoteControlPublicCardError::InvalidPublicKey { role, component })
            );
        }
    }
    for (offset, component) in [
        (0, RemoteControlPublicCardKeyComponent::Encryption),
        (32, RemoteControlPublicCardKeyComponent::Signing),
    ] {
        let mut public_key = controller().public_keys().public_key_bytes();
        public_key[offset..offset + 32].fill(0);
        let invalid = RemoteControlControllerIdentity::new(
            PublicIdentityMaterial::from_bytes(public_key).public_keys(),
        );
        let expected = RemoteControlPublicCardError::InvalidPublicKey {
            role: RemoteControlPublicCardIdentityRole::Controller,
            component,
        };
        assert_eq!(RemoteControlControllerCard::new(invalid), Err(expected));
        let mut bytes = [0; RemoteControlControllerCard::MAX_ENCODED_LEN];
        RemoteControlControllerCard::new(controller())
            .unwrap()
            .encode(&mut bytes)
            .unwrap();
        bytes[HEADER_LEN..].copy_from_slice(&public_key);
        assert_eq!(RemoteControlControllerCard::decode(&bytes), Err(expected));
    }
}

proptest::proptest! {
    #[test]
    fn arbitrary_binary_input_is_bounded_and_every_accepted_card_is_canonical(input in proptest::collection::vec(proptest::num::u8::ANY, 0..1024)) {
        if let Ok(card) = RemoteControlControllerCard::decode(&input) {
            let mut canonical = [0; RemoteControlControllerCard::MAX_ENCODED_LEN];
            let len = card.encode(&mut canonical).unwrap();
            proptest::prop_assert_eq!(&canonical[..len], input.as_slice());
        }
        if let Ok(card) = RemoteControlTargetAccessCard::decode(&input) {
            let canonical = binary(&card);
            proptest::prop_assert_eq!(canonical, input);
        }
    }
}

#[cfg(feature = "alloc")]
#[test]
fn owning_helpers_preserve_vectors_and_typed_validation() {
    let controller = decode_remote_control_controller_card(CONTROLLER_VECTOR.into()).unwrap();
    assert_eq!(
        encode_remote_control_controller_card(controller).unwrap(),
        CONTROLLER_VECTOR
    );
    let access = decode_remote_control_target_access_card(ACCESS_VECTOR.into()).unwrap();
    let (recipient, value) = access.into_parts();
    assert_eq!(
        remote_control_controller_identity_hash(recipient),
        recipient.identity_hash()
    );
    let target = RemoteControlTargetIdentity::new(*value.target().public_keys());
    assert_eq!(
        remote_control_target_identity_hash(target),
        value.target().identity_hash()
    );

    let access = RemoteControlTargetAccessCard::new(recipient, value).unwrap();
    assert_eq!(
        encode_remote_control_target_access_card(access).unwrap(),
        ACCESS_VECTOR
    );
    assert_eq!(
        decode_remote_control_controller_card("invalid".into()),
        Err(RemoteControlPublicCardError::InvalidTextEnvelope)
    );
    assert_eq!(
        decode_remote_control_target_access_card(CONTROLLER_VECTOR.into()),
        Err(RemoteControlPublicCardError::UnexpectedRole { found: 1 })
    );
    let attempt =
        crate::remote_control::RemoteControlPairingAttemptId::from_test_transcript_digest_bytes(
            [0; 32],
        );
    assert_eq!(remote_control_pairing_confirmation_code(attempt), "000000");
}
