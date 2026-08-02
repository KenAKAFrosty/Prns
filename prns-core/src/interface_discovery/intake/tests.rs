use core::cell::Cell;

use super::*;
use crate::interface_discovery::{
    encode_encrypted_envelope, encode_plaintext_envelope, AutoConnectPolicy, DiscoverySourcePolicy,
    HeapDiscoveryValidationCache, RNS_VALIDATION_CACHE_CAPACITY,
};

const PYTHON_BACKBONE: &str = "8b00b14261636b626f6e65496e7465726661636501c3ccfec41000112233445566778899aabbccddeeffccffaf5075626c6963204261636b626f6e6503cb402900000000000004cbc04120000000000005cb405ec0000000000002ae726f757465722e6578616d706c6506cd109207a46d65736808a6736563726574";

fn bytes_from_hex(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(core::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn bytes_from_hex_array<const N: usize>(hex: &str) -> [u8; N] {
    let bytes = bytes_from_hex(hex);
    match bytes.try_into() {
        Ok(bytes) => bytes,
        Err(bytes) => panic!("expected {N} bytes, got {}", bytes.len()),
    }
}

fn stamp_cost(value: u16) -> StampCost {
    match StampCost::new(value) {
        Ok(value) => value,
        Err(error) => panic!("unexpected stamp cost error: {error}"),
    }
}

fn policy(cost: u16, sources: Vec<IdentityHash>) -> InterfaceDiscoveryPolicy {
    InterfaceDiscoveryPolicy::enabled(
        stamp_cost(cost),
        DiscoverySourcePolicy::from_sources(sources),
        AutoConnectPolicy::from_maximum(0),
        crate::interfaces::InterfaceGravity::ZERO,
    )
}

fn observation<'a>(source: IdentityHash, app_data: &'a [u8]) -> AnnounceObservation<'a> {
    AnnounceObservation {
        destination: discovery_destination_hash(&source),
        announced_identity: source,
        hops: HopCount(3),
        source_interface: InterfaceId::new([0x44; 8]),
        arrived_at: InstantMillis(9_000),
        app_data,
        is_path_response: false,
    }
}

fn reference_payload() -> Vec<u8> {
    let packed = bytes_from_hex(PYTHON_BACKBONE);
    let mut stamp = [0u8; super::super::STAMP_SIZE];
    stamp[31] = 0xb6;
    encode_plaintext_envelope(&packed, &stamp)
}

#[test]
fn a_reference_payload_becomes_a_typed_discovery_with_exact_provenance() {
    let source = IdentityHash::new([0x31; 16]);
    let payload = reference_payload();
    let outcome = ingest_discovery_announce(
        &policy(8, Vec::new()),
        observation(source, &payload),
        |_| Err(DiscoveryDecryptionError::NetworkIdentityUnavailable),
    );
    let DiscoveryIntake::Discovered(discovered) = outcome else {
        panic!("reference payload should be discovered: {outcome:?}");
    };
    assert_eq!(discovered.name, "Public Backbone");
    assert_eq!(
        discovered.id.as_bytes(),
        &bytes_from_hex_array("76d4621425d989d36b677a573bfb37b6c38e15432fd8f132a054617d9b38e616"),
    );
    assert_eq!(discovered.stamp_value.get(), 8);
    assert_eq!(
        discovered.provenance,
        DiscoveryProvenance {
            announced_by: source,
            hops: HopCount(3),
            received_on: InterfaceId::new([0x44; 8]),
            received_at: InstantMillis(9_000),
            envelope_security: DiscoveryEnvelopeSecurity::Plaintext,
            signed_flag: false,
        }
    );
    assert_eq!(
        discovered.origin(),
        InterfaceOrigin::Discovered(discovered.provenance)
    );
    assert_eq!(discovered.origin().kind(), InterfaceOriginKind::Discovered);
}

#[test]
fn source_authorization_precedes_envelope_parsing_and_decryption() {
    let authorized = IdentityHash::new([0x41; 16]);
    let denied = IdentityHash::new([0x42; 16]);
    let decrypt_calls = Cell::new(0);
    let outcome = ingest_discovery_announce(
        &policy(8, vec![authorized]),
        observation(denied, &[0x02, 0xaa]),
        |_| {
            decrypt_calls.set(decrypt_calls.get() + 1);
            Ok(Vec::new())
        },
    );
    assert_eq!(
        outcome,
        DiscoveryIntake::Rejected(DiscoveryRejection::UnauthorizedSource { source: denied })
    );
    assert_eq!(decrypt_calls.get(), 0);
}

#[test]
fn encrypted_payloads_use_the_injected_network_identity_operation() {
    let source = IdentityHash::new([0x51; 16]);
    let plaintext = reference_payload()[1..].to_vec();
    let ciphertext = [0xa5; 64];
    let payload = encode_encrypted_envelope(&ciphertext);
    let outcome = ingest_discovery_announce(
        &policy(8, Vec::new()),
        observation(source, &payload),
        |received| {
            assert_eq!(received, ciphertext);
            Ok(plaintext)
        },
    );
    let DiscoveryIntake::Discovered(discovered) = outcome else {
        panic!("decrypted reference payload should be discovered: {outcome:?}");
    };
    assert_eq!(
        discovered.provenance.envelope_security,
        DiscoveryEnvelopeSecurity::NetworkEncrypted
    );
}

#[test]
fn cached_encrypted_payloads_skip_decryption_but_refresh_provenance() {
    let source = IdentityHash::new([0x53; 16]);
    let plaintext = reference_payload()[1..].to_vec();
    let ciphertext = [0xa6; 64];
    let payload = encode_encrypted_envelope(&ciphertext);
    let decrypt_calls = Cell::new(0);
    let mut cache = HeapDiscoveryValidationCache::default();

    let first = ingest_discovery_announce_cached(
        &policy(8, Vec::new()),
        observation(source, &payload),
        |_| {
            decrypt_calls.set(decrypt_calls.get() + 1);
            Ok(plaintext.clone())
        },
        &mut cache,
    );
    let mut refreshed = observation(source, &payload);
    refreshed.hops = HopCount(4);
    refreshed.source_interface = InterfaceId::new([0x45; 8]);
    refreshed.arrived_at = InstantMillis(10_000);
    let second = ingest_discovery_announce_cached(
        &policy(8, Vec::new()),
        refreshed,
        |_| {
            decrypt_calls.set(decrypt_calls.get() + 1);
            Ok(plaintext)
        },
        &mut cache,
    );

    assert_eq!(decrypt_calls.get(), 1);
    let DiscoveryIntake::Discovered(first) = first else {
        panic!("first encrypted payload should be discovered: {first:?}");
    };
    let DiscoveryIntake::Discovered(second) = second else {
        panic!("cached encrypted payload should be discovered: {second:?}");
    };
    assert_eq!(first.id, second.id);
    assert_eq!(second.provenance.hops, HopCount(4));
    assert_eq!(second.provenance.received_on, InterfaceId::new([0x45; 8]));
    assert_eq!(second.provenance.received_at, InstantMillis(10_000));
}

#[test]
fn cached_insufficient_stamps_skip_repeated_decryption() {
    let source = IdentityHash::new([0x54; 16]);
    let plaintext = reference_payload()[1..].to_vec();
    let payload = encode_encrypted_envelope(&[0xa7; 64]);
    let decrypt_calls = Cell::new(0);
    let mut cache = HeapDiscoveryValidationCache::default();

    let first = ingest_discovery_announce_cached(
        &policy(9, Vec::new()),
        observation(source, &payload),
        |_| {
            decrypt_calls.set(decrypt_calls.get() + 1);
            Ok(plaintext.clone())
        },
        &mut cache,
    );
    let second = ingest_discovery_announce_cached(
        &policy(9, Vec::new()),
        observation(source, &payload),
        |_| {
            decrypt_calls.set(decrypt_calls.get() + 1);
            Ok(plaintext)
        },
        &mut cache,
    );

    assert_eq!(decrypt_calls.get(), 1);
    assert_eq!(second, first);
    assert!(matches!(
        second,
        DiscoveryIntake::Rejected(DiscoveryRejection::StampBelowCost { .. })
    ));
}

#[test]
fn validation_caches_evict_the_oldest_entry_at_the_reference_capacity() {
    let mut cache = HeapDiscoveryValidationCache::default();
    let stamp_value = StampValue::new(16).unwrap();
    for index in 0..=RNS_VALIDATION_CACHE_CAPACITY {
        let mut payload_hash = [0u8; 32];
        payload_hash[..8].copy_from_slice(&(index as u64).to_be_bytes());
        cache.remember_valid(payload_hash, &[index as u8], stamp_value);
        cache.remember_insufficient(payload_hash, stamp_value);
    }

    let first = [0u8; 32];
    let mut last = [0u8; 32];
    last[..8].copy_from_slice(&(RNS_VALIDATION_CACHE_CAPACITY as u64).to_be_bytes());
    assert_eq!(
        cache.lengths(),
        (RNS_VALIDATION_CACHE_CAPACITY, RNS_VALIDATION_CACHE_CAPACITY),
    );
    assert!(cache.valid(&first).is_none());
    assert!(cache.insufficient(&first).is_none());
    assert!(cache.valid(&last).is_some());
    assert_eq!(cache.insufficient(&last), Some(stamp_value));
}

#[test]
fn a_short_encrypted_envelope_is_rejected_without_decryption() {
    let source = IdentityHash::new([0x52; 16]);
    let decrypt_calls = Cell::new(0);
    let outcome = ingest_discovery_announce(
        &policy(8, Vec::new()),
        observation(source, &[0x02, 0xaa]),
        |_| {
            decrypt_calls.set(decrypt_calls.get() + 1);
            Ok(Vec::new())
        },
    );
    assert_eq!(
        outcome,
        DiscoveryIntake::Rejected(DiscoveryRejection::MalformedEnvelope(
            DiscoveryEnvelopeError::PayloadTooShort
        ))
    );
    assert_eq!(decrypt_calls.get(), 0);
}

#[test]
fn path_responses_and_other_aspects_never_enter_discovery_processing() {
    let source = IdentityHash::new([0x61; 16]);
    let payload = reference_payload();
    let mut path_response = observation(source, &payload);
    path_response.is_path_response = true;
    assert_eq!(
        ingest_discovery_announce(&policy(8, Vec::new()), path_response, |_| Ok(Vec::new())),
        DiscoveryIntake::NotApplicable(DiscoveryNotApplicable::PathResponse),
    );

    let mut other_aspect = observation(source, &payload);
    other_aspect.destination = crate::wire::DestinationHash::new([0x99; 16]);
    assert_eq!(
        ingest_discovery_announce(&policy(8, Vec::new()), other_aspect, |_| Ok(Vec::new())),
        DiscoveryIntake::NotApplicable(DiscoveryNotApplicable::DifferentAspect),
    );
}

#[test]
fn a_below_cost_stamp_is_rejected_before_messagepack_decode() {
    let source = IdentityHash::new([0x71; 16]);
    let payload = reference_payload();
    let packed = bytes_from_hex(PYTHON_BACKBONE);
    let mut stamp = [0u8; super::super::STAMP_SIZE];
    stamp[31] = 0xb6;
    let value = crate::interface_discovery::stamp_value(
        &AdvertisementHash::for_advertisement(&packed),
        &stamp,
    );
    assert_eq!(
        ingest_discovery_announce(
            &policy(9, Vec::new()),
            observation(source, &payload),
            |_| { Ok(Vec::new()) }
        ),
        DiscoveryIntake::Rejected(DiscoveryRejection::StampBelowCost {
            value,
            required: stamp_cost(9),
        }),
    );
}

#[test]
fn discovered_names_follow_the_reference_ascii_and_edge_sanitizer() {
    assert_eq!(
        sanitize_name("  !!! Púb  lic (Backbone) ***  "),
        "Pb lic (Backbone)"
    );
    assert_eq!(sanitize_name("***"), "");
    assert_eq!(sanitize_name("Node)"), "Node)");
}
