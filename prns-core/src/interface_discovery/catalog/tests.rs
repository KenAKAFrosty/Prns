use super::*;
use crate::identity::IdentityHash;
use crate::interface_discovery::{
    generate_stamp, AdvertisedInterfaceType, AdvertisedTransport, AdvertisementDetails,
    AdvertisementHash, DiscoveryAdvertisement, DiscoveryEnvelopeSecurity, DiscoveryProvenance,
    GeographicLocation, StampCost, StampGeneration, StampValue, DISCOVERY_EXPIRES_AFTER,
    DISCOVERY_STALE_AFTER, DISCOVERY_UNKNOWN_AFTER,
};
use crate::interfaces::InterfaceId;
use crate::units::HopCount;
use crate::wire::TransportId;

fn stamp_value(cost: u16) -> StampValue {
    let cost = match StampCost::new(cost) {
        Ok(cost) => cost,
        Err(error) => panic!("unexpected stamp cost: {error}"),
    };
    let hash = AdvertisementHash::from_hash([0x5a; 32]);
    let mut nonce = 0u64;
    match generate_stamp(
        &hash,
        cost,
        |candidate| {
            candidate.fill(0);
            candidate[..8].copy_from_slice(&nonce.to_be_bytes());
            nonce = nonce.saturating_add(1);
            Ok::<(), ()>(())
        },
        || false,
    ) {
        StampGeneration::Generated(generated) => generated.value,
        StampGeneration::Cancelled | StampGeneration::EntropyFailure(()) => {
            panic!("deterministic stamp generation should succeed")
        }
    }
}

fn discovered(
    id: u8,
    received_at: InstantMillis,
    stamp_value: StampValue,
    host: &str,
) -> DiscoveredInterface {
    DiscoveredInterface {
        id: DiscoveredInterfaceId::from_bytes([id; 32]),
        name: alloc::format!("Interface {id}"),
        advertisement: DiscoveryAdvertisement {
            interface_type: AdvertisedInterfaceType::Backbone,
            transport: AdvertisedTransport::Enabled(TransportId::new([id; 16])),
            name: None,
            location: GeographicLocation::UNKNOWN,
            details: AdvertisementDetails::Reachable {
                host: String::from(host),
                port: 4242,
            },
            published_ifac: None,
        },
        stamp_value,
        provenance: DiscoveryProvenance {
            announced_by: IdentityHash::new([id; 16]),
            hops: HopCount(id),
            received_on: InterfaceId::new([id; 8]),
            received_at,
            envelope_security: DiscoveryEnvelopeSecurity::Plaintext,
            signed_flag: false,
        },
    }
}

#[test]
fn observations_preserve_history_refresh_content_and_reject_time_regression() {
    let value = stamp_value(1);
    let mut catalog = DiscoveryCatalog::new();
    let id = DiscoveredInterfaceId::from_bytes([1; 32]);

    assert_eq!(
        catalog.observe(discovered(1, InstantMillis(1_000), value, "one.example")),
        Ok(DiscoveryCatalogUpdate::Added { id })
    );
    assert_eq!(
        catalog.observe(discovered(1, InstantMillis(2_000), value, "one.example")),
        Ok(DiscoveryCatalogUpdate::Refreshed {
            id,
            refresh: DiscoveryCatalogRefresh::AdvertisementUnchanged,
        })
    );
    assert_eq!(
        catalog.observe(discovered(1, InstantMillis(3_000), value, "two.example")),
        Ok(DiscoveryCatalogUpdate::Refreshed {
            id,
            refresh: DiscoveryCatalogRefresh::AdvertisementChanged,
        })
    );
    assert_eq!(
        catalog.observe(discovered(1, InstantMillis(2_999), value, "old.example")),
        Ok(DiscoveryCatalogUpdate::IgnoredOutOfOrder {
            id,
            received_at: InstantMillis(2_999),
            last_heard: InstantMillis(3_000),
        })
    );

    let record = catalog.get(id).expect("the observed interface remains");
    assert_eq!(record.first_heard(), InstantMillis(1_000));
    assert_eq!(record.last_heard(), InstantMillis(3_000));
    assert_eq!(record.observation_count().get(), 3);
    assert_eq!(
        record.interface().advertisement.details,
        AdvertisementDetails::Reachable {
            host: String::from("two.example"),
            port: 4242,
        }
    );
}

#[test]
fn ranking_is_status_then_stamp_then_recency_then_stable_id() {
    let day = 24 * 60 * 60 * 1_000;
    let now = InstantMillis(10 * day);
    let low = stamp_value(1);
    let high = stamp_value(8);
    assert!(high > low);

    let mut catalog = DiscoveryCatalog::new();
    for interface in [
        discovered(6, InstantMillis(now.0 - 200), high, "six.example"),
        discovered(5, InstantMillis(now.0 - 100), low, "five.example"),
        discovered(
            2,
            InstantMillis(now.0 - DISCOVERY_UNKNOWN_AFTER.0 - 1),
            high,
            "two.example",
        ),
        discovered(
            3,
            InstantMillis(now.0 - DISCOVERY_STALE_AFTER.0 - 1),
            high,
            "three.example",
        ),
        discovered(
            4,
            InstantMillis(now.0 - DISCOVERY_EXPIRES_AFTER.0 - 1),
            high,
            "four.example",
        ),
        discovered(1, InstantMillis(now.0 - 200), high, "one.example"),
    ] {
        catalog
            .observe(interface)
            .expect("the growable catalog accepts every test record");
    }

    assert_eq!(
        catalog
            .ranked_records(now)
            .into_iter()
            .map(DiscoveryRecord::id)
            .collect::<Vec<_>>(),
        vec![
            DiscoveredInterfaceId::from_bytes([1; 32]),
            DiscoveredInterfaceId::from_bytes([6; 32]),
            DiscoveredInterfaceId::from_bytes([5; 32]),
            DiscoveredInterfaceId::from_bytes([2; 32]),
            DiscoveredInterfaceId::from_bytes([3; 32]),
            DiscoveredInterfaceId::from_bytes([4; 32]),
        ]
    );
}

#[test]
fn expiry_removes_only_after_the_reference_boundary() {
    let value = stamp_value(1);
    let mut catalog = DiscoveryCatalog::new();
    catalog
        .observe(discovered(1, InstantMillis(1_000), value, "one.example"))
        .expect("the growable catalog accepts the test record");

    assert!(catalog
        .remove_expired(InstantMillis(1_000 + DISCOVERY_EXPIRES_AFTER.0))
        .is_empty());
    let removed = catalog.remove_expired(InstantMillis(1_000 + DISCOVERY_EXPIRES_AFTER.0 + 1));
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].id(), DiscoveredInterfaceId::from_bytes([1; 32]));
    assert!(catalog.is_empty());
}
