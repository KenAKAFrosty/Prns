use prns_core::interface_discovery::{
    discovery_destination_hash, frame_discovery_publication, prepare_discovery_publication,
    AdvertisedInterfaceType, AdvertisedTransport, AdvertisementDetails, AutoConnectPolicy,
    AutoConnectRoutingPolicy, DiscoveryAdvertisement, DiscoveryPublicationPreparation,
    DiscoveryPublicationSecurity, DiscoverySourcePolicy, GeographicLocation, StampCost,
};
use prns_core::interfaces::InterfaceOriginKind;
use prns_core::wire::TransportId;

use super::*;

fn enabled_policy(maximum: usize) -> InterfaceDiscoveryPolicy {
    InterfaceDiscoveryPolicy::enabled(
        StampCost::new(1).expect("one is a valid stamp cost"),
        DiscoverySourcePolicy::from_sources(Vec::new()),
        AutoConnectPolicy::from_maximum(maximum),
        AutoConnectRoutingPolicy {
            gravity: prns_core::interfaces::InterfaceGravity::new(15),
            announces_to_internal: false,
        },
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
        _ => panic!("the deterministic discovery advertisement prepares"),
    };
    frame_discovery_publication(&prepared, |_| {
        Err(prns_core::interface_discovery::DiscoveryPublicationEncryptionError::NetworkIdentityUnavailable)
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

#[test]
fn ingress_only_copies_discovery_aspect_announces_when_enabled() {
    let (mut service, ingress) = TokioInterfaceDiscovery::new(enabled_policy(0), None);
    let identity = IdentityHash::new([0x22; 16]);
    let app_data = discovery_app_data("router.example", 4242);
    let accepted = observation(identity, &app_data);

    assert_eq!(
        ingress.observe(AnnounceObservation {
            destination: DestinationHash::new([0x99; 16]),
            ..accepted
        }),
        DiscoveryIngressOutcome::NotDiscovery
    );
    assert_eq!(
        ingress.observe(AnnounceObservation {
            is_path_response: true,
            ..accepted
        }),
        DiscoveryIngressOutcome::NotDiscovery
    );
    assert_eq!(ingress.observe(accepted), DiscoveryIngressOutcome::Queued);
    assert!(service.observations.try_recv().is_ok());

    let (_disabled, disabled_ingress) =
        TokioInterfaceDiscovery::new(InterfaceDiscoveryPolicy::Disabled, None);
    assert_eq!(
        disabled_ingress.observe(accepted),
        DiscoveryIngressOutcome::Disabled
    );
}

#[test]
fn auto_connect_capacity_reports_only_when_auto_connect_is_enabled() {
    let (mut service, _) = TokioInterfaceDiscovery::new(enabled_policy(2), None);
    let mut capacities = Vec::new();
    service.report_auto_connect_capacity(&mut |event| {
        if let TokioDiscoveryEvent::AutoConnectCapacity { online, maximum } = event {
            capacities.push((online, maximum));
        }
    });
    let interface = InterfaceId::new([0x35; 8]);
    service.statuses.insert(
        interface,
        TokioInterfaceStatus::new_unaccounted(
            interface,
            prns_core::interfaces::ConnectionState::Connected,
        ),
    );
    service.report_auto_connect_capacity(&mut |event| {
        if let TokioDiscoveryEvent::AutoConnectCapacity { online, maximum } = event {
            capacities.push((online, maximum));
        }
    });

    assert_eq!(capacities, vec![(0, 2), (1, 2)]);

    let (disabled, _) = TokioInterfaceDiscovery::new(
        InterfaceDiscoveryPolicy::enabled(
            StampCost::new(1).expect("one is a valid stamp cost"),
            DiscoverySourcePolicy::from_sources(Vec::new()),
            AutoConnectPolicy::Disabled,
            AutoConnectRoutingPolicy {
                gravity: prns_core::interfaces::InterfaceGravity::ZERO,
                announces_to_internal: false,
            },
        ),
        None,
    );
    let mut reported = false;
    disabled.report_auto_connect_capacity(&mut |_| reported = true);
    assert!(!reported);
}

#[test]
fn accepted_observations_update_the_generic_catalog_with_full_provenance() {
    let (mut service, _ingress) = TokioInterfaceDiscovery::new(enabled_policy(0), None);
    let identity = IdentityHash::new([0x22; 16]);
    let app_data = discovery_app_data("router.example", 4242);
    let owned = OwnedAnnounceObservation::from_borrowed(observation(identity, &app_data));
    let outputs = service.ingest_observation(owned, InstantMillis(10_000), &[]);
    let updates = outputs
        .iter()
        .filter_map(|output| match output {
            DiscoveryCoordinatorOutput::Event(DiscoveryCoordinatorEvent::CatalogUpdated(
                update,
            )) => Some(*update),
            DiscoveryCoordinatorOutput::Event(_) | DiscoveryCoordinatorOutput::Action(_) => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(updates.len(), 1);
    assert!(matches!(updates[0], DiscoveryCatalogUpdate::Added { .. }));
    let record = service
        .catalog()
        .records()
        .next()
        .expect("the accepted discovery is catalogued");
    assert_eq!(record.interface().provenance.announced_by, identity);
    assert_eq!(record.interface().provenance.hops, HopCount(2));
    assert_eq!(record.interface().name, "Public backbone");
}

#[test]
fn dial_targets_bracket_ipv6_without_changing_dns_or_ipv4() {
    assert_eq!(dial_target("2001:db8::1", 4242), "[2001:db8::1]:4242");
    assert_eq!(dial_target("192.0.2.1", 4242), "192.0.2.1:4242");
    assert_eq!(dial_target("router.example", 4242), "router.example:4242");
}

#[tokio::test]
async fn an_eligible_discovery_stands_up_a_real_backbone_client() {
    use prns_runtime::routing::{BlackholeExpiry, BlackholedIdentity};
    use prns_runtime::runtime::{
        IdentityBlackholeControl, ManuallyAttached, NoPersistence, PreConfiguredDestination,
        PrnsNode, PrnsNodeRecipe,
    };
    use prns_runtime::storage::GrowableHeap;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binds an ephemeral discovery target");
    let address = listener.local_addr().expect("the listener has an address");
    let (service, ingress) = TokioInterfaceDiscovery::new(enabled_policy(1), None);
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: std::iter::empty::<PreConfiguredDestination<'static>>(),
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: prns_runtime::request_endpoints![],
        remote_control: prns_runtime::remote_control::RemoteControlService::Unavailable,
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: |_event, _state: &()| {},
    })
    .with_timeline_origin(InstantMillis(10_000));
    let handle = node.handle();
    let clock = node.clock();
    let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
    let service_task = tokio::spawn(
        service.run(handle.clone(), clock, move |event| match event {
            TokioDiscoveryEvent::ConnectionAttached { interface, .. } => {
                let _ = events_tx.send((true, interface));
            }
            TokioDiscoveryEvent::ConnectionDetached { interface, .. } => {
                let _ = events_tx.send((false, interface));
            }
            _ => {}
        }),
    );
    let scenario = async move {
        let identity = IdentityHash::new([0x22; 16]);
        let app_data = discovery_app_data("127.0.0.1", address.port());
        assert_eq!(
            ingress.observe(observation(identity, &app_data)),
            DiscoveryIngressOutcome::Queued
        );
        let (attached, interface) = tokio::time::timeout(Duration::from_secs(2), events_rx.recv())
            .await
            .expect("the discovery service attaches promptly")
            .expect("the discovery event lane remains open");
        assert!(attached);
        let (_socket, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("the discovered client dials promptly")
            .expect("the listener accepts the discovered client");
        assert_eq!(
            interface.kind(),
            Some(prns_core::interfaces::InterfaceKind::BackboneClient)
        );
        let inventory = handle.interface_inventory();
        let attached = inventory
            .iter()
            .find(|entry| entry.snapshot.id == interface)
            .expect("the discovered interface is registered for inspection");
        assert_eq!(
            (attached.name.as_deref(), attached.origin),
            (Some("Public backbone"), InterfaceOriginKind::Discovered)
        );
        assert_eq!(
            attached.snapshot.gravity,
            prns_core::interfaces::InterfaceGravity::new(15)
        );

        handle
            .blackhole_identity(BlackholedIdentity {
                identity: IdentityHash::new([0x44; 16]),
                source: IdentityHash::new([0x99; 16]),
                expiry: BlackholeExpiry::Indefinite,
                reason: None,
            })
            .await
            .expect("the advertised transport identity is blackholed");
        assert_eq!(
            ingress.observe(observation(identity, &app_data)),
            DiscoveryIngressOutcome::Queued
        );
        let (attached, detached_interface) =
            tokio::time::timeout(Duration::from_secs(2), events_rx.recv())
                .await
                .expect("blackhole reconciliation detaches promptly")
                .expect("the discovery event lane remains open");
        assert!(!attached);
        assert_eq!(detached_interface, interface);
        tokio::time::timeout(Duration::from_secs(2), async {
            while handle
                .interface_inventory()
                .iter()
                .any(|entry| entry.snapshot.id == interface)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the detached interface leaves runtime inventory promptly");

        drop(ingress);
        service_task
            .await
            .expect("closing discovery ingress stops the service");
    };
    tokio::select! {
        result = node.run() => panic!("the node stays up for the discovery scenario: {result:?}"),
        () = scenario => {}
    }
}
