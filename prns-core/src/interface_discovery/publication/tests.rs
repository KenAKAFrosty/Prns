use alloc::string::String;

use super::*;
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::{IdentitySigner, RemoteIdentity};
use crate::interface_discovery::{
    ingest_discovery_announce, AdvertisedInterfaceType, AdvertisedTransport, AdvertisementDetails,
    AutoConnectPolicy, AutoConnectRoutingPolicy, DiscoveryDecryptionError, DiscoveryIntake,
    DiscoverySourcePolicy, GeographicLocation, InterfaceDiscoveryPolicy, PublishedIfac,
};
use crate::routing::announce::AnnounceObservation;
use crate::units::HopCount;
use crate::wire::TransportId;

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

fn stamp_cost(value: u16) -> StampCost {
    match StampCost::new(value) {
        Ok(value) => value,
        Err(error) => panic!("unexpected stamp cost error: {error}"),
    }
}

fn prepared(security: DiscoveryPublicationSecurity) -> PreparedDiscoveryAdvertisement {
    let mut candidate = 0u8;
    match prepare_discovery_publication(
        &backbone(),
        stamp_cost(8),
        security,
        |bytes| {
            bytes.fill(0);
            bytes[31] = candidate;
            candidate = candidate.wrapping_add(1);
            Ok::<_, core::convert::Infallible>(())
        },
        || false,
    ) {
        DiscoveryPublicationPreparation::Prepared(prepared) => prepared,
        other => panic!("unexpected publication preparation: {other:?}"),
    }
}

#[test]
fn deterministic_stamping_reproduces_the_reference_payload() {
    let prepared = prepared(DiscoveryPublicationSecurity::Plaintext);
    assert_eq!(prepared.stamp()[31], 0xb6);
    assert_eq!(prepared.stamp_value().get(), 8);
    assert_eq!(prepared.stamp_attempts(), 183);
    let app_data = match frame_discovery_publication(&prepared, |_| {
        Err(DiscoveryPublicationEncryptionError::NetworkIdentityUnavailable)
    }) {
        Ok(app_data) => app_data,
        Err(error) => panic!("unexpected frame error: {error:?}"),
    };
    assert_eq!(app_data[0], 0);
    assert_eq!(
        &app_data[1..1 + prepared.packed_advertisement().len()],
        prepared.packed_advertisement(),
    );
    assert_eq!(&app_data[app_data.len() - STAMP_SIZE..], prepared.stamp());
}

#[test]
fn a_cached_stamp_is_revalidated_and_reused_without_entropy_work() {
    let advertisement = backbone();
    let first = prepare_discovery_publication(
        &advertisement,
        stamp_cost(1),
        DiscoveryPublicationSecurity::Plaintext,
        |candidate| {
            candidate.fill(0x42);
            Ok::<_, core::convert::Infallible>(())
        },
        || false,
    );
    let DiscoveryPublicationPreparation::Prepared(first) = first else {
        panic!("the first stamp should prepare");
    };
    let cached = *first.stamp();
    let entropy_calls = core::cell::Cell::new(0);
    let second = prepare_discovery_publication_with_stamp_cache(
        &advertisement,
        stamp_cost(1),
        DiscoveryPublicationSecurity::Plaintext,
        |hash| {
            assert_eq!(hash.as_bytes(), first.advertisement_hash().as_bytes());
            Some(cached)
        },
        |_| {
            entropy_calls.set(entropy_calls.get() + 1);
            Ok::<_, core::convert::Infallible>(())
        },
        || false,
    );
    let DiscoveryPublicationPreparation::Prepared(second) = second else {
        panic!("the cached stamp should prepare");
    };
    assert_eq!(second.stamp(), &cached);
    assert_eq!(second.stamp_value(), first.stamp_value());
    assert_eq!(second.stamp_attempts(), 0);
    assert_eq!(entropy_calls.get(), 0);
}

#[test]
fn network_encryption_is_injected_and_never_confused_with_plaintext() {
    let prepared = prepared(DiscoveryPublicationSecurity::NetworkEncrypted);
    let expected_body = prepared.plaintext_body();
    let app_data = match frame_discovery_publication(&prepared, |body| {
        assert_eq!(body, expected_body);
        Ok(vec![
            0xa5;
            crate::identity::ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN
                + sealed_len(body.len())
        ])
    }) {
        Ok(app_data) => app_data,
        Err(error) => panic!("unexpected frame error: {error:?}"),
    };
    assert_eq!(
        app_data,
        encode_encrypted_envelope(&vec![
            0xa5;
            crate::identity::ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN
                + sealed_len(expected_body.len())
        ])
    );
}

#[test]
fn malformed_encryption_output_cannot_be_framed_as_a_discovery_announce() {
    let prepared = prepared(DiscoveryPublicationSecurity::NetworkEncrypted);
    let expected = crate::identity::ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN
        + sealed_len(prepared.plaintext_body().len());
    assert_eq!(
        frame_discovery_publication(&prepared, |_| Ok(Vec::new())),
        Err(DiscoveryPublicationFrameError::EncryptionOutputLength {
            actual: 0,
            expected,
        })
    );
}

#[test]
fn network_encrypted_publication_round_trips_through_the_shared_identity_crypto() {
    let identity = InMemoryNodeIdentity::from_secret_key_bytes(&[0x37; 64]);
    let remote = RemoteIdentity::from_public_keys(
        identity.encryption_public_key(),
        identity.signing_public_key(),
    );
    let prepared = prepared(DiscoveryPublicationSecurity::NetworkEncrypted);
    let app_data = match frame_discovery_publication(&prepared, |body| {
        let required =
            crate::identity::ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN + sealed_len(body.len());
        let mut ciphertext = vec![0u8; required];
        let written = remote
            .encrypt(
                &crate::crypto::X25519SecretKey::new([0x45; 32]),
                &[0x56; crate::identity::ENCRYPTION_IV_LEN],
                body,
                &mut ciphertext,
            )
            .map_err(DiscoveryPublicationEncryptionError::Identity)?;
        ciphertext.truncate(written);
        Ok(ciphertext)
    }) {
        Ok(app_data) => app_data,
        Err(error) => panic!("unexpected frame error: {error:?}"),
    };
    let policy = InterfaceDiscoveryPolicy::enabled(
        stamp_cost(8),
        DiscoverySourcePolicy::Open,
        AutoConnectPolicy::Disabled,
        AutoConnectRoutingPolicy {
            gravity: crate::interfaces::InterfaceGravity::ZERO,
            announces_to_internal: false,
        },
    );
    let outcome = ingest_discovery_announce(
        &policy,
        AnnounceObservation {
            destination: crate::interface_discovery::discovery_destination_hash(
                &identity.identity_hash(),
            ),
            announced_identity: identity.identity_hash(),
            hops: HopCount(1),
            source_interface: InterfaceId::new([0x91; 8]),
            arrived_at: InstantMillis(8_000),
            app_data: &app_data,
            is_path_response: false,
        },
        |ciphertext| {
            let mut plaintext = vec![0u8; ciphertext.len()];
            let written = identity
                .decrypt(ciphertext, &mut plaintext)
                .map_err(DiscoveryDecryptionError::Identity)?;
            plaintext.truncate(written);
            Ok(plaintext)
        },
    );
    let DiscoveryIntake::Discovered(discovered) = outcome else {
        panic!("encrypted publication should be discovered: {outcome:?}");
    };
    assert_eq!(discovered.name, "Public Backbone");
}

#[test]
fn oversized_payloads_fail_before_stamp_work() {
    let mut advertisement = backbone();
    advertisement.name = Some("x".repeat(MAX_ANNOUNCE_APP_DATA_LEN));
    let entropy_calls = core::cell::Cell::new(0);
    let outcome = prepare_discovery_publication(
        &advertisement,
        stamp_cost(1),
        DiscoveryPublicationSecurity::Plaintext,
        |_| {
            entropy_calls.set(entropy_calls.get() + 1);
            Ok::<_, core::convert::Infallible>(())
        },
        || false,
    );
    assert!(matches!(
        outcome,
        DiscoveryPublicationPreparation::AppDataTooLong { .. }
    ));
    assert_eq!(entropy_calls.get(), 0);
}

#[test]
fn invalid_reachable_addresses_fail_before_stamp_work() {
    let mut advertisement = backbone();
    advertisement.details = AdvertisementDetails::Reachable {
        host: String::from("not a host"),
        port: 4242,
    };
    let entropy_calls = core::cell::Cell::new(0);
    let outcome = prepare_discovery_publication(
        &advertisement,
        stamp_cost(1),
        DiscoveryPublicationSecurity::Plaintext,
        |_| {
            entropy_calls.set(entropy_calls.get() + 1);
            Ok::<_, core::convert::Infallible>(())
        },
        || false,
    );
    assert_eq!(
        outcome,
        DiscoveryPublicationPreparation::InvalidReachableOn {
            value: String::from("not a host"),
        }
    );
    assert_eq!(entropy_calls.get(), 0);
}

#[test]
fn the_schedule_selects_one_most_overdue_interface_deterministically() {
    let first = InterfaceId::new([1; 8]);
    let second = InterfaceId::new([2; 8]);
    let mut schedule = match DiscoveryPublicationSchedule::new([
        DiscoveryPublicationTiming {
            interface: first,
            interval: DurationMillis(100),
        },
        DiscoveryPublicationTiming {
            interface: second,
            interval: DurationMillis(100),
        },
    ]) {
        Ok(schedule) => schedule,
        Err(error) => panic!("unexpected schedule error: {error:?}"),
    };
    assert_eq!(schedule.next_due(InstantMillis(1_000)), Some(first));
    assert_eq!(schedule.record_attempt(first, InstantMillis(1_000)), Ok(()));
    assert_eq!(schedule.next_due(InstantMillis(1_000)), Some(second));
    assert_eq!(
        schedule.record_attempt(second, InstantMillis(1_010)),
        Ok(())
    );
    assert_eq!(schedule.next_due(InstantMillis(1_100)), None);
    assert_eq!(schedule.next_due(InstantMillis(1_101)), Some(first));
    assert_eq!(schedule.record_attempt(first, InstantMillis(1_101)), Ok(()));
    assert_eq!(schedule.next_due(InstantMillis(1_111)), Some(second));
}

#[test]
fn invalid_schedule_shapes_are_rejected_at_construction() {
    let interface = InterfaceId::new([3; 8]);
    assert!(matches!(
        DiscoveryPublicationSchedule::new([DiscoveryPublicationTiming {
            interface,
            interval: DurationMillis(0),
        }]),
        Err(DiscoveryPublicationScheduleError::ZeroInterval { .. })
    ));
    assert!(matches!(
        DiscoveryPublicationSchedule::new([
            DiscoveryPublicationTiming {
                interface,
                interval: DurationMillis(1),
            },
            DiscoveryPublicationTiming {
                interface,
                interval: DurationMillis(2),
            },
        ]),
        Err(DiscoveryPublicationScheduleError::DuplicateInterface { .. })
    ));
}
