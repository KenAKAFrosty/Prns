use alloc::{format, vec};

use super::*;
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::{IdentitySigner, IDENTITY_SECRET_KEY_LEN};

#[test]
fn credentials_derive_authentication_and_transport_authority_together() {
    let secret = [0x42; IDENTITY_SECRET_KEY_LEN];
    let identity = InMemoryNodeIdentity::from_secret_key_bytes(&secret);
    let credentials = SharedInstanceCredentials::from_identity_secret(&secret);

    assert_eq!(
        credentials.rpc_key().as_bytes(),
        crate::crypto::sha256(&secret)
    );
    assert_eq!(
        credentials.transport_identity_hash(),
        identity.identity_hash()
    );
    assert!(!format!("{:?}", credentials.rpc_key()).contains("4242"));
}

#[test]
fn frame_headers_use_cpython_short_and_wide_forms() {
    let short = EncodedRpcFrameHeader::new(1_024).unwrap();
    assert_eq!(short.as_bytes(), &1_024i32.to_be_bytes());
    let RpcFrameHeaderPrefix::Complete(short_length) =
        RpcFrameHeaderPrefix::decode(short.as_bytes().try_into().unwrap()).unwrap()
    else {
        panic!("short header carries its complete length");
    };
    assert_eq!(short_length.as_usize(), 1_024);

    let wide_length = i32::MAX as usize + 1;
    let wide = EncodedRpcFrameHeader::new(wide_length).unwrap();
    assert_eq!(&wide.as_bytes()[..4], &(-1i32).to_be_bytes());
    assert_eq!(
        RpcFrameHeaderPrefix::decode(wide.as_bytes()[..4].try_into().unwrap()),
        Ok(RpcFrameHeaderPrefix::WideLengthFollows)
    );
    assert_eq!(
        RpcFrameHeaderPrefix::decode_wide(wide.as_bytes()[4..].try_into().unwrap())
            .unwrap()
            .as_usize(),
        wide_length
    );
}

#[test]
fn negative_short_frame_lengths_are_rejected() {
    assert_eq!(
        RpcFrameHeaderPrefix::decode((-2i32).to_be_bytes()),
        Err(RpcFrameLengthDecodeError::NegativeShortLength)
    );
}

#[test]
fn modern_server_challenge_accepts_sha256_and_legacy_md5_responses() {
    let key = RpcAuthenticationKey::new(vec![0x5a; 32]);
    let challenge = RpcServerChallenge::new(RpcChallengeNonce::new([0x11; 40]));
    let message = challenge
        .wire_payload()
        .strip_prefix(b"#CHALLENGE#")
        .unwrap();

    let mut modern = b"{sha256}".to_vec();
    modern.extend_from_slice(
        &RpcDigest::Sha256
            .message_authentication_code(&key, message)
            .unwrap(),
    );
    assert_eq!(
        challenge.authenticate_response(&key, &modern),
        Ok(RpcAuthenticationVerdict::Authenticated)
    );

    let legacy = RpcDigest::Md5
        .message_authentication_code(&key, message)
        .unwrap();
    assert_eq!(
        challenge.authenticate_response(&key, &legacy),
        Ok(RpcAuthenticationVerdict::Authenticated)
    );
}

#[test]
fn authentication_matches_cpython_reference_vectors() {
    let key = RpcAuthenticationKey::new(vec![0x5a; 32]);
    let challenge = RpcServerChallenge::new(RpcChallengeNonce::new([0x11; 40]));
    let mut sha256_response = b"{sha256}".to_vec();
    sha256_response.extend_from_slice(&[
        0x49, 0xba, 0xf8, 0x4c, 0x6a, 0x22, 0x32, 0x53, 0x90, 0x38, 0x1d, 0xf0, 0x9d, 0x94, 0x93,
        0xfe, 0x9a, 0x77, 0xcf, 0x05, 0x0d, 0xde, 0x03, 0x92, 0x22, 0x14, 0x0e, 0x17, 0xe9, 0xd8,
        0x80, 0x90,
    ]);
    assert_eq!(
        challenge.authenticate_response(&key, &sha256_response),
        Ok(RpcAuthenticationVerdict::Authenticated)
    );
    assert_eq!(
        challenge.authenticate_response(
            &key,
            &[
                0x1f, 0xb3, 0xcb, 0x62, 0xba, 0xc0, 0xe1, 0x81, 0x60, 0x61, 0xcb, 0xcb, 0xc6, 0x2b,
                0x20, 0xae,
            ],
        ),
        Ok(RpcAuthenticationVerdict::Authenticated)
    );

    let tagged = RpcClientChallenge::parse(b"#CHALLENGE#{md5}client nonce")
        .unwrap()
        .response(&key)
        .unwrap();
    assert_eq!(
        tagged.wire_payload(),
        &[
            b'{', b'm', b'd', b'5', b'}', 0x32, 0xdd, 0x49, 0x9b, 0x5a, 0xd5, 0x54, 0x93, 0xa2,
            0x76, 0xbe, 0x95, 0x9b, 0xe0, 0x07, 0x11,
        ]
    );
    let legacy = RpcClientChallenge::parse(&[
        b'#', b'C', b'H', b'A', b'L', b'L', b'E', b'N', b'G', b'E', b'#', 0x22, 0x22, 0x22, 0x22,
        0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22,
        0x22,
    ])
    .unwrap()
    .response(&key)
    .unwrap();
    assert_eq!(
        legacy.wire_payload(),
        &[
            0x4b, 0x29, 0x15, 0x08, 0xfe, 0x87, 0x8d, 0xf0, 0x25, 0xc0, 0x31, 0xe6, 0xa5, 0x32,
            0xfc, 0x17,
        ]
    );
}

#[test]
fn client_challenge_answers_the_negotiated_digest() {
    let key = RpcAuthenticationKey::new(vec![0x5a; 32]);
    let mut tagged = b"#CHALLENGE#{md5}".to_vec();
    tagged.extend_from_slice(b"client nonce");
    let response = RpcClientChallenge::parse(&tagged)
        .unwrap()
        .response(&key)
        .unwrap();
    assert!(response.wire_payload().starts_with(b"{md5}"));

    let mut legacy = b"#CHALLENGE#".to_vec();
    legacy.extend_from_slice(&[0x22; LEGACY_MD5_MESSAGE_LENGTH]);
    let response = RpcClientChallenge::parse(&legacy)
        .unwrap()
        .response(&key)
        .unwrap();
    assert_eq!(response.wire_payload().len(), LEGACY_MD5_DIGEST_LENGTH);
}

#[test]
fn unsupported_digests_and_bad_macs_are_rejected() {
    let key = RpcAuthenticationKey::new(vec![0x5a; 32]);
    let challenge = RpcServerChallenge::new(RpcChallengeNonce::new([0x11; 40]));
    let mut bad_mac = b"{sha256}".to_vec();
    bad_mac.extend_from_slice(&[0; 32]);

    assert_eq!(
        challenge.authenticate_response(&key, &bad_mac),
        Ok(RpcAuthenticationVerdict::Rejected)
    );
    assert_eq!(
        RpcClientChallenge::parse(b"#CHALLENGE#{sha1}payload"),
        Err(RpcAuthenticationError::UnsupportedDigest)
    );
}
