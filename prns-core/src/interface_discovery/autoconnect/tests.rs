use alloc::string::String;

use super::*;
use crate::identity::IdentityHash;
use crate::interface_discovery::{
    generate_stamp, AutoConnectPolicy, AutoConnectRoutingPolicy, DiscoveryAdvertisement,
    DiscoveryEnvelopeSecurity, DiscoverySourcePolicy, GeographicLocation,
    HeapDiscoveredEndpointSet, PublishedIfac, StampCost, StampGeneration, DEFAULT_STAMP_COST,
    DISCOVERY_UNKNOWN_AFTER,
};
use crate::units::HopCount;

fn bytes_from_hex<const N: usize>(hex: &str) -> [u8; N] {
    let mut bytes = [0u8; N];
    assert_eq!(hex.len(), N * 2);
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16)
            .expect("the test vector is hexadecimal");
    }
    bytes
}

fn stamp_value() -> StampValue {
    let hash = super::super::AdvertisementHash::from_hash([0x33; 32]);
    let mut nonce = 0u64;
    let cost = StampCost::new(1).expect("one is a valid stamp cost");
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
    interface_type: AdvertisedInterfaceType,
    transport_enabled: bool,
    host: &str,
    received_at: InstantMillis,
) -> super::super::DiscoveredInterface {
    let details = match interface_type {
        AdvertisedInterfaceType::Backbone | AdvertisedInterfaceType::TcpServer => {
            AdvertisementDetails::Reachable {
                host: String::from(host),
                port: 4242,
            }
        }
        AdvertisedInterfaceType::TcpClient => AdvertisementDetails::None,
        AdvertisedInterfaceType::I2p => AdvertisementDetails::I2p {
            address: String::from(host),
        },
        AdvertisedInterfaceType::RNode => AdvertisementDetails::RNode {
            frequency_hz: 915_000_000,
            bandwidth_hz: 125_000,
            spreading_factor: 8,
            coding_rate: 5,
        },
        AdvertisedInterfaceType::Weave => AdvertisementDetails::Weave {
            frequency_hz: 915_000_000,
            bandwidth_hz: 125_000,
            channel: 1,
            modulation: String::from("LoRa"),
        },
        AdvertisedInterfaceType::Kiss => AdvertisementDetails::Kiss {
            frequency_hz: 915_000_000,
            bandwidth_hz: 125_000,
            modulation: String::from("LoRa"),
        },
    };
    super::super::DiscoveredInterface {
        id: DiscoveredInterfaceId::from_bytes([id; 32]),
        name: alloc::format!("Interface {id}"),
        advertisement: DiscoveryAdvertisement {
            interface_type,
            transport: AdvertisedTransport::from_wire(
                transport_enabled,
                TransportId::new([id; 16]),
            ),
            name: None,
            location: GeographicLocation::UNKNOWN,
            details,
            published_ifac: None,
        },
        stamp_value: stamp_value(),
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

fn policy(maximum: usize) -> InterfaceDiscoveryPolicy {
    InterfaceDiscoveryPolicy::enabled(
        DEFAULT_STAMP_COST,
        DiscoverySourcePolicy::from_sources(Vec::new()),
        AutoConnectPolicy::from_maximum(maximum),
        AutoConnectRoutingPolicy {
            gravity: crate::interfaces::InterfaceGravity::new(-14),
            announces_to_internal: true,
        },
    )
}

fn registry_with(endpoints: &[DiscoveredConnectionEndpointId]) -> DiscoveredConnectionRegistry {
    let mut registry = DiscoveredConnectionRegistry::new();
    for (index, endpoint) in endpoints.iter().copied().enumerate() {
        let byte = u8::try_from(index + 1).expect("the test registry is tiny");
        assert_eq!(
            registry.register(ActiveDiscoveredInterface::new(
                DiscoveredInterfaceId::from_bytes([byte; 32]),
                endpoint,
                InterfaceId::new([byte; 8]),
            )),
            Ok(())
        );
    }
    registry
}

fn endpoint_set(endpoints: &[DiscoveredConnectionEndpointId]) -> HeapDiscoveredEndpointSet {
    let mut set = HeapDiscoveredEndpointSet::default();
    for endpoint in endpoints {
        assert_eq!(set.try_insert(*endpoint), Ok(true));
    }
    set
}

#[test]
fn endpoint_identity_matches_the_reference_hash_material() {
    assert_eq!(
        DiscoveredConnectionEndpointId::for_endpoint("router.example", 4242).as_bytes(),
        &bytes_from_hex::<32>("b6d56f2aab60b83497177d443f3c6b38aba418be87ec04a2add07794d853b4bd")
    );
}

#[test]
fn startup_selection_is_bounded_ranked_deduplicated_and_type_safe() {
    let day = 24 * 60 * 60 * 1_000;
    let now = InstantMillis(10 * day);
    let mut first = discovered(
        1,
        AdvertisedInterfaceType::Backbone,
        true,
        "one.example",
        now,
    );
    first.advertisement.published_ifac = Some(PublishedIfac {
        network_name: Some(String::from("mesh")),
        passphrase: Some(String::from("secret")),
    });
    let mut catalog = DiscoveryCatalog::new();
    for interface in [
        first,
        discovered(
            2,
            AdvertisedInterfaceType::TcpServer,
            true,
            "two.example",
            InstantMillis(now.0 - DISCOVERY_UNKNOWN_AFTER.0 - 1),
        ),
        discovered(
            3,
            AdvertisedInterfaceType::RNode,
            true,
            "radio.example",
            now,
        ),
        discovered(
            4,
            AdvertisedInterfaceType::Backbone,
            false,
            "four.example",
            now,
        ),
        discovered(
            5,
            AdvertisedInterfaceType::Backbone,
            true,
            "one.example",
            now,
        ),
    ] {
        catalog
            .observe(interface)
            .expect("the growable catalog accepts every test record");
    }

    let active = DiscoveredConnectionRegistry::new();
    let plans = plan_discovered_connections(
        &catalog,
        &policy(2),
        DiscoveredConnectionSelection::Startup,
        now,
        &active,
        &endpoint_set(&[]),
    );
    assert_eq!(plans.len(), 2);
    assert_eq!(
        plans
            .iter()
            .map(DiscoveredConnectionPlan::discovery_id)
            .collect::<Vec<_>>(),
        vec![
            DiscoveredInterfaceId::from_bytes([1; 32]),
            DiscoveredInterfaceId::from_bytes([2; 32]),
        ]
    );
    assert_eq!(
        plans[0].access(),
        &DiscoveredConnectionAccess::PublishedIfac {
            network_name: Some(String::from("mesh")),
            passphrase: Some(String::from("secret")),
        }
    );
    assert_eq!(plans[0].endpoint().host(), "one.example");
    assert_eq!(plans[0].endpoint().port(), 4242);
    assert_eq!(plans[0].transport_id(), TransportId::new([1; 16]));
    assert_eq!(
        plans[0].gravity(),
        crate::interfaces::InterfaceGravity::new(-14)
    );
    assert!(plans[0].announces_to_internal());
    assert_eq!(
        plans[0].connection_kind(),
        DiscoveredConnectionKind::BackboneClient
    );
    assert_eq!(
        plans[1].connection_kind(),
        DiscoveredConnectionKind::TcpClient
    );
}

#[test]
fn configured_and_active_endpoints_block_duplicate_connections() {
    let now = InstantMillis(50_000);
    let mut catalog = DiscoveryCatalog::new();
    catalog
        .observe(discovered(
            1,
            AdvertisedInterfaceType::Backbone,
            true,
            "one.example",
            now,
        ))
        .expect("the growable catalog accepts the test record");
    catalog
        .observe(discovered(
            2,
            AdvertisedInterfaceType::TcpServer,
            true,
            "two.example",
            now,
        ))
        .expect("the growable catalog accepts the test record");
    let active_endpoints = [DiscoveredConnectionEndpointId::for_endpoint(
        "one.example",
        4242,
    )];
    let active = registry_with(&active_endpoints);
    let occupied = endpoint_set(&[DiscoveredConnectionEndpointId::for_endpoint(
        "two.example",
        4242,
    )]);

    assert!(plan_discovered_connections(
        &catalog,
        &policy(3),
        DiscoveredConnectionSelection::Startup,
        now,
        &active,
        &occupied,
    )
    .is_empty());
    let no_active = DiscoveredConnectionRegistry::new();
    assert!(plan_discovered_connections(
        &catalog,
        &InterfaceDiscoveryPolicy::Disabled,
        DiscoveredConnectionSelection::Startup,
        now,
        &no_active,
        &endpoint_set(&[]),
    )
    .is_empty());
}

#[test]
fn refill_keeps_reserved_capacity_and_only_uses_available_records() {
    let now = InstantMillis(DISCOVERY_UNKNOWN_AFTER.0 + 10_000);
    let mut catalog = DiscoveryCatalog::new();
    catalog
        .observe(discovered(
            1,
            AdvertisedInterfaceType::Backbone,
            true,
            "available.example",
            now,
        ))
        .expect("the growable catalog accepts the test record");
    catalog
        .observe(discovered(
            2,
            AdvertisedInterfaceType::Backbone,
            true,
            "unknown.example",
            InstantMillis(1),
        ))
        .expect("the growable catalog accepts the test record");
    let two_active_endpoints = [
        DiscoveredConnectionEndpointId::from_bytes([0xa1; 32]),
        DiscoveredConnectionEndpointId::from_bytes([0xa2; 32]),
    ];
    let three_active_endpoints = [
        DiscoveredConnectionEndpointId::from_bytes([0xa1; 32]),
        DiscoveredConnectionEndpointId::from_bytes([0xa2; 32]),
        DiscoveredConnectionEndpointId::from_bytes([0xa3; 32]),
    ];
    let two_active = registry_with(&two_active_endpoints);
    let three_active = registry_with(&three_active_endpoints);

    let refill = plan_discovered_connections(
        &catalog,
        &policy(4),
        DiscoveredConnectionSelection::Refill,
        now,
        &two_active,
        &endpoint_set(&[]),
    );
    assert_eq!(refill.len(), 1);
    assert_eq!(
        refill[0].discovery_id(),
        DiscoveredInterfaceId::from_bytes([1; 32])
    );
    assert!(plan_discovered_connections(
        &catalog,
        &policy(4),
        DiscoveredConnectionSelection::Refill,
        now,
        &three_active,
        &endpoint_set(&[]),
    )
    .is_empty());
    let available = endpoint_set(&[DiscoveredConnectionEndpointId::for_endpoint(
        "available.example",
        4242,
    )]);
    assert!(plan_discovered_connections(
        &catalog,
        &policy(4),
        DiscoveredConnectionSelection::Refill,
        now,
        &two_active,
        &available,
    )
    .is_empty());
}

#[test]
fn newly_observed_selection_targets_only_that_record() {
    let now = InstantMillis(20_000);
    let mut catalog = DiscoveryCatalog::new();
    for id in [1, 2] {
        catalog
            .observe(discovered(
                id,
                AdvertisedInterfaceType::Backbone,
                true,
                &alloc::format!("{id}.example"),
                now,
            ))
            .expect("the growable catalog accepts every test record");
    }

    let active = DiscoveredConnectionRegistry::new();
    let plans = plan_discovered_connections(
        &catalog,
        &policy(2),
        DiscoveredConnectionSelection::NewlyObserved(DiscoveredInterfaceId::from_bytes([2; 32])),
        now,
        &active,
        &endpoint_set(&[]),
    );
    assert_eq!(plans.len(), 1);
    assert_eq!(
        plans[0].discovery_id(),
        DiscoveredInterfaceId::from_bytes([2; 32])
    );
}

#[test]
fn connection_registry_enforces_uniqueness_and_detaches_at_the_exact_threshold() {
    let first_interface = InterfaceId::new([1; 8]);
    let first_endpoint = DiscoveredConnectionEndpointId::from_bytes([0x11; 32]);
    let mut registry = DiscoveredConnectionRegistry::new();
    assert_eq!(
        registry.register(ActiveDiscoveredInterface::new(
            DiscoveredInterfaceId::from_bytes([1; 32]),
            first_endpoint,
            first_interface,
        )),
        Ok(())
    );
    assert_eq!(
        registry.register(ActiveDiscoveredInterface::new(
            DiscoveredInterfaceId::from_bytes([2; 32]),
            DiscoveredConnectionEndpointId::from_bytes([0x22; 32]),
            first_interface,
        )),
        Err(
            DiscoveredConnectionRegistrationError::InterfaceAlreadyTracked {
                interface: first_interface,
            }
        )
    );
    assert_eq!(
        registry.register(ActiveDiscoveredInterface::new(
            DiscoveredInterfaceId::from_bytes([3; 32]),
            first_endpoint,
            InterfaceId::new([3; 8]),
        )),
        Err(
            DiscoveredConnectionRegistrationError::EndpointAlreadyTracked {
                endpoint: first_endpoint,
            }
        )
    );

    assert_eq!(
        registry.observe_health(
            first_interface,
            DiscoveredConnectionHealth::Offline,
            InstantMillis(1_000),
        ),
        DiscoveredConnectionTransition::Disconnected {
            discovery: DiscoveredInterfaceId::from_bytes([1; 32]),
            interface: first_interface,
            since: InstantMillis(1_000),
        }
    );
    assert_eq!(
        registry.observe_health(
            first_interface,
            DiscoveredConnectionHealth::Offline,
            InstantMillis(12_999),
        ),
        DiscoveredConnectionTransition::Unchanged
    );
    assert_eq!(
        registry.observe_health(
            first_interface,
            DiscoveredConnectionHealth::Online,
            InstantMillis(13_000),
        ),
        DiscoveredConnectionTransition::Reconnected {
            discovery: DiscoveredInterfaceId::from_bytes([1; 32]),
            interface: first_interface,
        }
    );
    assert_eq!(
        registry.observe_health(
            first_interface,
            DiscoveredConnectionHealth::Offline,
            InstantMillis(14_000),
        ),
        DiscoveredConnectionTransition::Disconnected {
            discovery: DiscoveredInterfaceId::from_bytes([1; 32]),
            interface: first_interface,
            since: InstantMillis(14_000),
        }
    );
    assert_eq!(
        registry.observe_health(
            first_interface,
            DiscoveredConnectionHealth::Offline,
            InstantMillis(26_000),
        ),
        DiscoveredConnectionTransition::Detach(ActiveDiscoveredInterface {
            discovery_id: DiscoveredInterfaceId::from_bytes([1; 32]),
            endpoint_id: first_endpoint,
            interface_id: first_interface,
            disconnected_since: Some(InstantMillis(14_000)),
        })
    );
    assert_eq!(registry.active.len(), 0);
    assert_eq!(
        registry.observe_health(
            first_interface,
            DiscoveredConnectionHealth::Online,
            InstantMillis(26_001),
        ),
        DiscoveredConnectionTransition::Untracked {
            interface: first_interface,
        }
    );
}
