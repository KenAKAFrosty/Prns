use alloc::string::String;

use crate::identity::IdentityHash;
use crate::interface_discovery::{
    frame_discovery_publication, prepare_discovery_publication, AdvertisedInterfaceType,
    AdvertisedTransport, AdvertisementDetails, AutoConnectPolicy, DiscoveredConnectionTable,
    DiscoveredEndpointSet, DiscoveredInterface, DiscoveredInterfaceId, DiscoveryAdvertisement,
    DiscoveryCatalogSeed, DiscoveryCatalogTable, DiscoveryEnvelopeSecurity,
    DiscoveryObservationCount, DiscoveryProvenance, DiscoveryPublicationPreparation,
    DiscoveryPublicationSecurity, DiscoveryRecord, DiscoverySourcePolicy,
    FixedDiscoveryValidationCache, GeographicLocation, StampCost, StampValue,
    DISCOVERED_INTERFACE_DETACH_AFTER,
};
use crate::routing::announce::AnnounceObservation;
use crate::storage::TablePushError;
use crate::units::HopCount;
use crate::wire::{DestinationHash, TransportId};

use super::*;

#[derive(Default)]
struct CapacitylessCatalogTable;

impl DiscoveryCatalogTable for CapacitylessCatalogTable {
    type Records<'a> = core::iter::Empty<&'a DiscoveryRecord>;

    fn len(&self) -> usize {
        0
    }

    fn get(&self, _: DiscoveredInterfaceId) -> Option<&DiscoveryRecord> {
        None
    }

    fn get_mut(&mut self, _: DiscoveredInterfaceId) -> Option<&mut DiscoveryRecord> {
        None
    }

    fn try_insert(
        &mut self,
        _: DiscoveredInterfaceId,
        _: DiscoveryRecord,
    ) -> Result<Option<DiscoveryRecord>, TablePushError> {
        Err(TablePushError::TableFull)
    }

    fn remove(&mut self, _: DiscoveredInterfaceId) -> Option<DiscoveryRecord> {
        None
    }

    fn records(&self) -> Self::Records<'_> {
        core::iter::empty()
    }
}

#[derive(Default)]
struct CapacitylessConnectionTable;

impl DiscoveredConnectionTable for CapacitylessConnectionTable {
    type Connections<'a> = core::iter::Empty<&'a ActiveDiscoveredInterface>;

    fn len(&self) -> usize {
        0
    }

    fn get_mut(&mut self, _: InterfaceId) -> Option<&mut ActiveDiscoveredInterface> {
        None
    }

    fn contains_interface(&self, _: InterfaceId) -> bool {
        false
    }

    fn contains_endpoint(&self, _: DiscoveredConnectionEndpointId) -> bool {
        false
    }

    fn try_insert(
        &mut self,
        _: ActiveDiscoveredInterface,
    ) -> Result<Option<ActiveDiscoveredInterface>, TablePushError> {
        Err(TablePushError::TableFull)
    }

    fn remove(&mut self, _: InterfaceId) -> Option<ActiveDiscoveredInterface> {
        None
    }

    fn connections(&self) -> Self::Connections<'_> {
        core::iter::empty()
    }
}

#[derive(Default)]
struct CapacitylessEndpointSet;

impl DiscoveredEndpointSet for CapacitylessEndpointSet {
    type Endpoints<'a> = core::iter::Empty<DiscoveredConnectionEndpointId>;

    fn try_insert(&mut self, _: DiscoveredConnectionEndpointId) -> Result<bool, TablePushError> {
        Err(TablePushError::TableFull)
    }

    fn endpoints(&self) -> Self::Endpoints<'_> {
        core::iter::empty()
    }
}

struct CapacitylessDiscoveryStorage;

impl InterfaceDiscoveryStorage for CapacitylessDiscoveryStorage {
    type ValidationCache = FixedDiscoveryValidationCache<0, 0, 0, 1, 1>;
    type Catalog = CapacitylessCatalogTable;
    type Connections = CapacitylessConnectionTable;
    type ReservedEndpoints = CapacitylessEndpointSet;
}

fn enabled_policy(maximum: usize) -> InterfaceDiscoveryPolicy {
    InterfaceDiscoveryPolicy::enabled(
        StampCost::new(1).expect("one is a valid stamp cost"),
        DiscoverySourcePolicy::from_sources(Vec::new()),
        AutoConnectPolicy::from_maximum(maximum),
    )
}

fn discovery_app_data(host: &str, port: u16) -> Vec<u8> {
    let advertisement = DiscoveryAdvertisement {
        interface_type: AdvertisedInterfaceType::Backbone,
        transport: AdvertisedTransport::Enabled(TransportId::new([0x44; 16])),
        name: Some(String::from("Public backbone")),
        location: GeographicLocation::UNKNOWN,
        details: AdvertisementDetails::Reachable {
            host: String::from(host),
            port,
        },
        published_ifac: None,
    };
    let mut nonce = 0u64;
    let prepared = match prepare_discovery_publication(
        &advertisement,
        StampCost::new(1).expect("one is a valid stamp cost"),
        DiscoveryPublicationSecurity::Plaintext,
        |candidate| {
            candidate.fill(0);
            candidate[..8].copy_from_slice(&nonce.to_be_bytes());
            nonce = nonce.saturating_add(1);
            Ok::<(), ()>(())
        },
        || false,
    ) {
        DiscoveryPublicationPreparation::Prepared(prepared) => prepared,
        DiscoveryPublicationPreparation::Cancelled
        | DiscoveryPublicationPreparation::EncodeFailed(_)
        | DiscoveryPublicationPreparation::InvalidReachableOn { .. }
        | DiscoveryPublicationPreparation::EntropyFailed(())
        | DiscoveryPublicationPreparation::AppDataTooLong { .. } => {
            panic!("the deterministic discovery advertisement prepares")
        }
    };
    frame_discovery_publication(&prepared, |_| {
        Err(super::super::DiscoveryPublicationEncryptionError::NetworkIdentityUnavailable)
    })
    .expect("plaintext framing does not ask for encryption")
}

fn observation<'a>(identity: IdentityHash, app_data: &'a [u8]) -> AnnounceObservation<'a> {
    AnnounceObservation {
        destination: discovery_destination_hash(&identity),
        announced_identity: identity,
        hops: HopCount(2),
        source_interface: InterfaceId::new([0x55; 8]),
        arrived_at: InstantMillis(10_000),
        app_data,
        is_path_response: false,
    }
}

fn restored_interface(id: u8, stamp_value: u16) -> DiscoveredInterface {
    DiscoveredInterface {
        id: DiscoveredInterfaceId::from_bytes([id; 32]),
        name: alloc::format!("Restored {id}"),
        advertisement: DiscoveryAdvertisement {
            interface_type: AdvertisedInterfaceType::Backbone,
            transport: AdvertisedTransport::Enabled(TransportId::new([id; 16])),
            name: None,
            location: GeographicLocation::UNKNOWN,
            details: AdvertisementDetails::Reachable {
                host: alloc::format!("restored-{id}.example"),
                port: 4242,
            },
            published_ifac: None,
        },
        stamp_value: StampValue::new(stamp_value).expect("test stamp value is attainable"),
        provenance: DiscoveryProvenance {
            announced_by: IdentityHash::new([id; 16]),
            hops: HopCount(1),
            received_on: InterfaceId::new([id; 8]),
            received_at: InstantMillis(10_000),
            envelope_security: DiscoveryEnvelopeSecurity::Plaintext,
            signed_flag: false,
        },
    }
}

fn plaintext_decrypt(_: &[u8]) -> Result<Vec<u8>, DiscoveryDecryptionError> {
    Err(DiscoveryDecryptionError::NetworkIdentityUnavailable)
}

fn only_attachment(outputs: Vec<DiscoveryCoordinatorOutput>) -> DiscoveredConnectionPlan {
    let mut attachments = outputs.into_iter().filter_map(|output| match output {
        DiscoveryCoordinatorOutput::Action(DiscoveryCoordinatorAction::Attach(plan)) => Some(plan),
        DiscoveryCoordinatorOutput::Event(_) | DiscoveryCoordinatorOutput::Action(_) => None,
    });
    let plan = attachments
        .next()
        .expect("the discovery produces an attachment action");
    assert!(attachments.next().is_none());
    plan
}

#[test]
fn ingress_filter_centralizes_candidate_classification() {
    let identity = IdentityHash::new([0x22; 16]);
    let accepted = observation(identity, &[]);
    let enabled = DiscoveryIngressFilter::from_policy(&enabled_policy(0));

    assert_eq!(
        enabled.classify(&AnnounceObservation {
            destination: DestinationHash::new([0x99; 16]),
            ..accepted
        }),
        DiscoveryIngressEligibility::NotDiscovery
    );
    assert_eq!(
        enabled.classify(&AnnounceObservation {
            is_path_response: true,
            ..accepted
        }),
        DiscoveryIngressEligibility::NotDiscovery
    );
    assert_eq!(
        enabled.classify(&accepted),
        DiscoveryIngressEligibility::Candidate
    );
    assert_eq!(
        DiscoveryIngressFilter::from_policy(&InterfaceDiscoveryPolicy::Disabled)
            .classify(&accepted),
        DiscoveryIngressEligibility::Disabled
    );
}

#[test]
fn accepted_observation_updates_catalog_and_plans_from_one_owner() {
    let mut coordinator = DiscoveryCoordinator::new(enabled_policy(1));
    let identity = IdentityHash::new([0x22; 16]);
    let app_data = discovery_app_data("router.example", 4242);
    let outputs = coordinator.observe_announce(
        observation(identity, &app_data),
        InstantMillis(10_005),
        plaintext_decrypt,
    );

    assert!(matches!(
        outputs.first(),
        Some(DiscoveryCoordinatorOutput::Event(
            DiscoveryCoordinatorEvent::CatalogUpdated(DiscoveryCatalogUpdate::Added { .. })
        ))
    ));
    let plan = only_attachment(outputs);
    assert_eq!(plan.endpoint().host(), "router.example");
    assert_eq!(plan.endpoint().port(), 4242);
    let record = coordinator
        .catalog()
        .get(plan.discovery_id())
        .expect("the planned discovery remains in the catalog");
    assert_eq!(record.interface().provenance.announced_by, identity);
    assert_eq!(record.interface().provenance.hops, HopCount(2));
    assert_eq!(record.interface().name, "Public backbone");
    assert_eq!(
        record.interface().provenance.envelope_security,
        DiscoveryEnvelopeSecurity::Plaintext
    );
}

#[test]
fn seeding_discards_records_below_the_effective_stamp_policy() {
    let mut default_catalog = DiscoveryCatalog::new();
    default_catalog
        .restore(DiscoveryCatalogSeed {
            interface: restored_interface(1, 14),
            first_heard: InstantMillis(9_000),
            observation_count: DiscoveryObservationCount::FIRST,
        })
        .expect("the old record restores before policy filtering");
    default_catalog
        .restore(DiscoveryCatalogSeed {
            interface: restored_interface(2, 16),
            first_heard: InstantMillis(9_000),
            observation_count: DiscoveryObservationCount::FIRST,
        })
        .expect("the current record restores before policy filtering");
    let mut upgraded = DiscoveryCoordinator::new(InterfaceDiscoveryPolicy::enabled(
        StampCost::new(16).expect("sixteen is a valid stamp cost"),
        DiscoverySourcePolicy::from_sources(Vec::new()),
        AutoConnectPolicy::from_maximum(1),
    ));
    upgraded.seed_catalog(default_catalog);
    assert_eq!(upgraded.catalog().len(), 1);
    assert!(upgraded
        .catalog()
        .get(DiscoveredInterfaceId::from_bytes([1; 32]))
        .is_none());
    assert!(upgraded
        .catalog()
        .get(DiscoveredInterfaceId::from_bytes([2; 32]))
        .is_some());

    let mut custom_catalog = DiscoveryCatalog::new();
    custom_catalog
        .restore(DiscoveryCatalogSeed {
            interface: restored_interface(3, 14),
            first_heard: InstantMillis(9_000),
            observation_count: DiscoveryObservationCount::FIRST,
        })
        .expect("the custom-cost record restores");
    let mut custom = DiscoveryCoordinator::new(InterfaceDiscoveryPolicy::enabled(
        StampCost::new(14).expect("fourteen is a valid stamp cost"),
        DiscoverySourcePolicy::from_sources(Vec::new()),
        AutoConnectPolicy::from_maximum(1),
    ));
    custom.seed_catalog(custom_catalog);
    assert_eq!(custom.catalog().len(), 1);
}

#[test]
fn reserved_endpoint_blocks_attachment_without_blocking_catalogue() {
    let mut coordinator = DiscoveryCoordinator::new(enabled_policy(1));
    assert_eq!(
        coordinator.reserve_network_endpoint("router.example", 4242),
        Ok(DiscoveryEndpointReservation::Added)
    );
    assert_eq!(
        coordinator.reserve_network_endpoint("router.example", 4242),
        Ok(DiscoveryEndpointReservation::AlreadyReserved)
    );
    let identity = IdentityHash::new([0x22; 16]);
    let app_data = discovery_app_data("router.example", 4242);
    let outputs = coordinator.observe_announce(
        observation(identity, &app_data),
        InstantMillis(10_005),
        plaintext_decrypt,
    );

    assert_eq!(coordinator.catalog().len(), 1);
    assert_eq!(outputs.len(), 1);
    assert!(matches!(
        outputs.first(),
        Some(DiscoveryCoordinatorOutput::Event(
            DiscoveryCoordinatorEvent::CatalogUpdated(DiscoveryCatalogUpdate::Added { .. })
        ))
    ));
}

#[test]
fn capacityless_storage_is_zero_sized_and_rejects_each_retained_state_kind() {
    assert_eq!(core::mem::size_of::<CapacitylessCatalogTable>(), 0);
    assert_eq!(core::mem::size_of::<CapacitylessConnectionTable>(), 0);
    assert_eq!(core::mem::size_of::<CapacitylessEndpointSet>(), 0);

    let mut coordinator =
        DiscoveryCoordinator::<CapacitylessDiscoveryStorage>::with_storage(enabled_policy(1));
    let endpoint = DiscoveredConnectionEndpointId::for_endpoint("router.example", 4242);
    assert_eq!(
        coordinator.reserve_endpoint(endpoint),
        Err(DiscoveryEndpointReservationError::CapacityReached(endpoint))
    );

    let identity = IdentityHash::new([0x22; 16]);
    let app_data = discovery_app_data("router.example", 4242);
    let outputs = coordinator.observe_announce(
        observation(identity, &app_data),
        InstantMillis(10_005),
        plaintext_decrypt,
    );
    assert!(matches!(
        outputs.as_slice(),
        [DiscoveryCoordinatorOutput::Event(
            DiscoveryCoordinatorEvent::CatalogStoreRejected(
                DiscoveryCatalogStoreError::CapacityReached(_)
            )
        )]
    ));

    let interface = InterfaceId::new([0x77; 8]);
    let mut connections = DiscoveredConnectionRegistry::with_table(CapacitylessConnectionTable);
    assert_eq!(
        connections.register(ActiveDiscoveredInterface::new(
            DiscoveredInterfaceId::from_bytes([0x33; 32]),
            endpoint,
            interface,
        )),
        Err(DiscoveredConnectionRegistrationError::CapacityReached { interface })
    );
}

#[test]
fn attachment_health_and_detachment_are_coordinated_as_ordered_outputs() {
    let mut coordinator = DiscoveryCoordinator::new(enabled_policy(1));
    let identity = IdentityHash::new([0x22; 16]);
    let app_data = discovery_app_data("router.example", 4242);
    let plan = only_attachment(coordinator.observe_announce(
        observation(identity, &app_data),
        InstantMillis(10_005),
        plaintext_decrypt,
    ));
    let discovery = plan.discovery_id();
    let interface = InterfaceId::new([0x77; 8]);

    assert!(matches!(
        coordinator
            .attachment_succeeded(plan, interface)
            .expect("the first attachment registers"),
        DiscoveryCoordinatorEvent::ConnectionAttached {
            interface: attached,
            ..
        } if attached == interface
    ));

    let disconnected_at = InstantMillis(20_000);
    assert_eq!(
        coordinator.maintain(
            disconnected_at,
            [(interface, DiscoveredConnectionHealth::Offline)],
        ),
        vec![DiscoveryCoordinatorOutput::Event(
            DiscoveryCoordinatorEvent::ConnectionDisconnected {
                discovery,
                interface,
                since: disconnected_at,
            }
        )]
    );
    assert_eq!(
        coordinator.maintain(
            InstantMillis(disconnected_at.0 + 1),
            [(interface, DiscoveredConnectionHealth::Online)],
        ),
        vec![DiscoveryCoordinatorOutput::Event(
            DiscoveryCoordinatorEvent::ConnectionReconnected {
                discovery,
                interface,
            }
        )]
    );

    let second_disconnect = InstantMillis(30_000);
    let _ = coordinator.maintain(
        second_disconnect,
        [(interface, DiscoveredConnectionHealth::Offline)],
    );
    assert_eq!(
        coordinator.maintain(
            second_disconnect.saturating_add(DISCOVERED_INTERFACE_DETACH_AFTER),
            [(interface, DiscoveredConnectionHealth::Offline)],
        ),
        vec![
            DiscoveryCoordinatorOutput::Action(DiscoveryCoordinatorAction::Detach { interface }),
            DiscoveryCoordinatorOutput::Event(DiscoveryCoordinatorEvent::ConnectionDetached {
                discovery,
                interface,
            }),
        ]
    );
}
