use super::persistence::{
    ratchet_label, try_zeroed_buffer, wall_clock_timeline_origin, MAX_BOOT_RECORD_LEN,
};
use super::*;
use crate::engine::{InstantMillis, MAX_SEND_SINGLE_PACKET_PLAINTEXT_LEN};
use crate::identity::vault::{IdentityLabel, IdentitySecretKey, IdentityVault, Removal};
use crate::identity::IDENTITY_SECRET_KEY_LEN;
use crate::interfaces::ifac::IfacSize;
use crate::interfaces::{InterfaceStatus, InterfaceVitals};
use crate::reactor::driver::{SelfRatchetSnapshot, TokioInterfaceStatus};
use crate::routing::BlackholeIdentityOutcome;

const PEER: DestinationHash = DestinationHash::new([0xAB; 16]);

fn handle() -> (PrnsNodeHandle, UnboundedReceiver<HostCommand>) {
    let (commands, command_rx) = mpsc::unbounded_channel();
    (PrnsNodeHandle::over(commands), command_rx)
}

#[test]
fn an_oversized_persisted_length_is_rejected_before_allocation() {
    assert!(try_zeroed_buffer(MAX_BOOT_RECORD_LEN + 1).is_none());
    assert!(try_zeroed_buffer(usize::MAX).is_none());
}

#[derive(Default)]
struct CountingVault {
    labels: Vec<String>,
}

impl IdentityVault for CountingVault {
    type Error = core::convert::Infallible;

    fn load(&self, _label: &IdentityLabel) -> Result<Option<IdentitySecretKey>, Self::Error> {
        Ok(None)
    }

    fn store(
        &mut self,
        _label: &IdentityLabel,
        _secret: &[u8; IDENTITY_SECRET_KEY_LEN],
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn remove(&mut self, _label: &IdentityLabel) -> Result<Removal, Self::Error> {
        Ok(Removal::NothingStored)
    }

    fn stored_blob_len(&self, _label: &IdentityLabel) -> Result<Option<usize>, Self::Error> {
        Ok(None)
    }

    fn load_blob<'b>(
        &self,
        _label: &IdentityLabel,
        _buf: &'b mut [u8],
    ) -> Result<Option<&'b [u8]>, Self::Error> {
        Ok(None)
    }

    fn store_blob(&mut self, label: &IdentityLabel, _blob: &[u8]) -> Result<(), Self::Error> {
        self.labels.push(label.as_str().to_owned());
        Ok(())
    }
}

#[tokio::test]
async fn one_ratchet_snapshot_stores_one_destination() {
    let (handle, mut command_rx) = handle();
    let destination = DestinationHash::new([0x5A; 16]);
    let snapshotting = tokio::spawn(async move { handle.snapshot_self_ratchet(destination).await });
    let HostCommand::SnapshotSelfRatchet {
        destination: requested,
        reply,
    } = command_rx.recv().await.unwrap()
    else {
        panic!("expected one ratchet snapshot command");
    };
    assert_eq!(requested, destination);
    assert!(reply
        .send(Some(SelfRatchetSnapshot {
            destination,
            sealed: Zeroizing::new(vec![0xA5; 64]),
        }))
        .is_ok());
    let snapshot = snapshotting.await.unwrap().unwrap().unwrap();
    let mut vault = CountingVault::default();
    snapshot.store_into(&mut vault).unwrap();
    assert_eq!(vault.labels, vec![ratchet_label(&destination).to_string()]);
}

#[test]
fn inspection_reads_the_runtime_packet_phy_store() {
    let (handle, _command_rx) = handle();
    let packet_hash = PacketHash::new([0x42; 32]);
    let packet_phy = PacketPhyStats {
        rssi: Some(crate::interfaces::RssiDbm::new(-82)),
        snr: None,
        quality: None,
    };
    handle.store.remember_packet_phy(packet_hash, packet_phy);

    assert_eq!(
        NodeIntrospection::packet_phy(&handle, packet_hash),
        Some(packet_phy)
    );
}

#[test]
fn the_standard_timeline_origin_is_unix_epoch_aligned() {
    let wall_now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let origin = wall_clock_timeline_origin();

    assert!(wall_now.abs_diff(u128::from(origin.0)) < 1_000);
}

#[test]
fn accepted_announce_observers_receive_the_complete_observation() {
    let captured = Arc::new(Mutex::new(None));
    let sink = captured.clone();
    let mut observer: Option<AcceptedAnnounceObserver> =
        Some(Box::new(move |observation: AnnounceObservation<'_>| {
            *sink.lock().unwrap() = Some((
                observation.destination,
                observation.announced_identity,
                observation.hops,
                observation.source_interface,
                observation.arrived_at,
                observation.app_data.to_vec(),
                observation.is_path_response,
            ));
        }));
    let app_data = [0x42, 0x43, 0x44];
    let observation = AnnounceObservation {
        destination: DestinationHash::new([0x11; 16]),
        announced_identity: crate::identity::IdentityHash::new([0x22; 16]),
        hops: crate::units::HopCount(3),
        source_interface: InterfaceId::new([0x33; 8]),
        arrived_at: InstantMillis(4_000),
        app_data: &app_data,
        is_path_response: false,
    };

    notify_accepted_announce(
        &mut observer,
        &Journaled::AnnounceHeard {
            observation,
            rate_accounting: crate::routing::announce::AnnounceRateAccounting::NotApplied,
        },
    );

    assert_eq!(
        *captured.lock().unwrap(),
        Some((
            observation.destination,
            observation.announced_identity,
            observation.hops,
            observation.source_interface,
            observation.arrived_at,
            app_data.to_vec(),
            observation.is_path_response,
        ))
    );
}

#[test]
fn boot_blackholes_seed_against_the_resumed_timeline() {
    let mut prns = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: (),
        storage: crate::storage::GrowableHeap,
        routes: crate::routes![],
        interfaces: Manual,
        on_event: |_event, _state: &()| {},
    })
    .with_timeline_origin(InstantMillis(1_000));
    let identity = crate::identity::IdentityHash::new([0x31; 16]);
    let source = crate::identity::IdentityHash::new([0x41; 16]);

    let report = prns.seed_blackholed_identities([
        BlackholedIdentity {
            identity,
            source,
            expiry: crate::routing::BlackholeExpiry::At(InstantMillis(2_000)),
            reason: Some("active"),
        },
        BlackholedIdentity {
            identity,
            source,
            expiry: crate::routing::BlackholeExpiry::Indefinite,
            reason: Some("duplicate"),
        },
        BlackholedIdentity {
            identity: crate::identity::IdentityHash::new([0x32; 16]),
            source,
            expiry: crate::routing::BlackholeExpiry::At(InstantMillis(999)),
            reason: Some("expired"),
        },
    ]);

    assert_eq!(
        report,
        BlackholeSeedReport {
            seeded_count: 1,
            refused_count: 1,
            dropped_count: 1,
        }
    );
    assert!(prns.node.engine.is_identity_blackholed(&identity));
    assert_eq!(prns.node.engine.blackholed_identity_count(), 1);
}

#[test]
fn new_with_handle_builds_state_from_the_nodes_handle() {
    let prns = PrnsNode::new_with_handle(|handle| PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: handle,
        storage: crate::storage::GrowableHeap,
        routes: crate::routes![],
        interfaces: Manual,
        on_event: |_event, _state: &PrnsNodeHandle| {},
    });

    assert!(Arc::ptr_eq(&prns.handle.ids, &prns.node.state.ids));
}

#[test]
fn a_runtime_destination_registers_only_its_selected_route_types() {
    struct First;
    impl RequestRoute<()> for First {
        const PATH: &'static str = "/first";
        const POLICY: super::super::request_router::RoutePolicy =
            super::super::request_router::RoutePolicy::AllowList(&[]);

        async fn handle(
            _context: super::super::request_router::RequestContext<'_, ()>,
        ) -> Result<(), super::super::request_router::Decline> {
            Ok(())
        }
    }

    struct Second;
    impl RequestRoute<()> for Second {
        const PATH: &'static str = "/second";
        const POLICY: super::super::request_router::RoutePolicy =
            super::super::request_router::RoutePolicy::AllowList(&[]);

        async fn handle(
            _context: super::super::request_router::RequestContext<'_, ()>,
        ) -> Result<(), super::super::request_router::Decline> {
            Ok(())
        }
    }

    let mut prns = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: (),
        storage: crate::storage::GrowableHeap,
        routes: crate::routes![First, Second],
        interfaces: Manual,
        on_event: |_event, _state: &()| {},
    });
    let destination = prns
        .register_preconfigured_destination(PreConfiguredDestination::Single {
            app_name: "typed",
            aspects: &["routes"],
            identity: Zeroizing::new([0x42; IDENTITY_SECRET_KEY_LEN]),
            announce_app_data: &[],
            proof: crate::routing::ProofStrategy::ProveNone,
            link_requests: crate::routing::LinkRequestPolicy::AcceptAll,
            ratchet: crate::engine::RatchetPolicy::NoRatchets,
            resource_strategy: ResourceStrategy::AcceptNone,
            request_handlers: super::super::RequestHandlerRegistration::None,
        })
        .unwrap();
    prns.register_request_route::<First>(&destination).unwrap();
    let identity = crate::identity::IdentityHash::new([0x31; 16]);

    assert_eq!(
        prns.allow_requester(&destination, First::PATH, identity),
        Ok(())
    );
    assert_eq!(
        prns.allow_requester(&destination, Second::PATH, identity),
        Err(RequestHandlerError::NoSuchHandler)
    );
}

#[cfg(feature = "runtime-metrics")]
#[tokio::test]
async fn metrics_snapshots_are_requested_from_the_reactor() {
    let (handle, mut command_rx) = handle();
    let expected = RuntimeMetricsSnapshot {
        taken_at: InstantMillis(42),
        engine: Default::default(),
        egress: Default::default(),
        crypto: None,
        reliability: Default::default(),
    };
    let snapshotting = tokio::spawn(async move { handle.metrics_snapshot().await });

    let HostCommand::SnapshotMetrics { reply } = command_rx.recv().await.unwrap() else {
        panic!("expected a metrics snapshot command");
    };
    reply.send(expected.clone()).unwrap();

    assert_eq!(snapshotting.await.unwrap(), Some(expected));
}

#[tokio::test]
async fn announce_rate_introspection_resolves_its_reactor_snapshot() {
    let (handle, mut command_rx) = handle();
    let expected = std::vec![AnnounceRateSnapshot {
        destination: DestinationHash::new([0x42; 16]),
        last_allowed_announce_at: InstantMillis(20),
        blocked_until: InstantMillis(0),
        rate_violations: 1,
        observed_at: std::vec![InstantMillis(10), InstantMillis(20)],
    }];
    let reading = tokio::spawn(async move { handle.announce_rates().await });

    let HostCommand::NodeIntrospection(NodeIntrospectionRequest::AnnounceRates { reply }) =
        command_rx.recv().await.unwrap()
    else {
        panic!("expected an announce-rate introspection request");
    };
    reply.send(expected.clone()).unwrap();

    assert_eq!(reading.await.unwrap(), expected);
}

#[tokio::test]
async fn routing_controls_resolve_their_typed_reactor_replies() {
    let (handle, mut command_rx) = handle();

    let dropping = tokio::spawn({
        let handle = handle.clone();
        async move { handle.drop_route(PEER).await }
    });
    let HostCommand::DropRoute { destination, reply } = command_rx.recv().await.unwrap() else {
        panic!("expected a route drop command");
    };
    assert_eq!(destination, PEER);
    reply.send(DropRouteOutcome::Dropped).unwrap();
    assert_eq!(dropping.await.unwrap(), Ok(DropRouteOutcome::Dropped));

    let transport = TransportId::new([0x42; 16]);
    let dropping_via = tokio::spawn({
        let handle = handle.clone();
        async move { handle.drop_routes_via(transport).await }
    });
    let HostCommand::DropRoutesVia {
        transport: requested,
        reply,
    } = command_rx.recv().await.unwrap()
    else {
        panic!("expected a transport route drop command");
    };
    assert_eq!(requested, transport);
    reply
        .send(DropRoutesViaOutcome { dropped_routes: 3 })
        .unwrap();
    assert_eq!(
        dropping_via.await.unwrap(),
        Ok(DropRoutesViaOutcome { dropped_routes: 3 })
    );

    let clearing = tokio::spawn(async move { handle.clear_announce_queues().await });
    let HostCommand::ClearAnnounceQueues { reply } = command_rx.recv().await.unwrap() else {
        panic!("expected an announce queue clear command");
    };
    reply
        .send(ClearAnnounceQueuesOutcome {
            dropped_announces: 5,
        })
        .unwrap();
    assert_eq!(
        clearing.await.unwrap(),
        Ok(ClearAnnounceQueuesOutcome {
            dropped_announces: 5,
        })
    );
}

#[tokio::test]
async fn routing_controls_report_a_stopped_reactor() {
    let (handle, command_rx) = handle();
    drop(command_rx);

    assert_eq!(
        handle.drop_route(PEER).await,
        Err(RoutingControlError::NodeStopped)
    );
    assert_eq!(
        handle.drop_routes_via(TransportId::new([0x42; 16])).await,
        Err(RoutingControlError::NodeStopped)
    );
    assert_eq!(
        handle.clear_announce_queues().await,
        Err(RoutingControlError::NodeStopped)
    );
}

#[tokio::test]
async fn identity_blackhole_capabilities_resolve_typed_reactor_replies() {
    let (handle, mut command_rx) = handle();
    let identity = crate::identity::IdentityHash::new([0x31; 16]);
    let source = crate::identity::IdentityHash::new([0x41; 16]);
    let expected = BlackholedIdentity {
        identity,
        source,
        expiry: crate::routing::BlackholeExpiry::Indefinite,
        reason: Some(String::from("operator")),
    };

    let reading = tokio::spawn({
        let handle = handle.clone();
        async move { handle.blackholed_identities().await }
    });
    let HostCommand::IdentityBlackhole(IdentityBlackholeHostCommand::ReadAll { reply }) =
        command_rx.recv().await.unwrap()
    else {
        panic!("expected a blackhole table read command");
    };
    reply.send(vec![expected.clone()]).unwrap();
    assert_eq!(reading.await.unwrap(), Ok(vec![expected.clone()]));

    let checking = tokio::spawn({
        let handle = handle.clone();
        async move { handle.is_blackholed(identity).await }
    });
    let HostCommand::IdentityBlackhole(IdentityBlackholeHostCommand::IsBlackholed {
        identity: requested,
        reply,
    }) = command_rx.recv().await.unwrap()
    else {
        panic!("expected an identity blackhole query command");
    };
    assert_eq!(requested, identity);
    reply.send(true).unwrap();
    assert_eq!(checking.await.unwrap(), Ok(true));

    let blackholing = tokio::spawn({
        let handle = handle.clone();
        async move {
            handle
                .blackhole_identity(BlackholedIdentity {
                    identity,
                    source,
                    expiry: crate::routing::BlackholeExpiry::Indefinite,
                    reason: Some("operator"),
                })
                .await
        }
    });
    let HostCommand::IdentityBlackhole(IdentityBlackholeHostCommand::Blackhole { entry, reply }) =
        command_rx.recv().await.unwrap()
    else {
        panic!("expected an identity blackhole command");
    };
    assert_eq!(entry, expected);
    reply.send(Ok(BlackholeIdentityOutcome::Added)).unwrap();
    assert_eq!(
        blackholing.await.unwrap(),
        Ok(BlackholeIdentityOutcome::Added)
    );

    let unblackholing = tokio::spawn(async move { handle.unblackhole_identity(identity).await });
    let HostCommand::IdentityBlackhole(IdentityBlackholeHostCommand::Unblackhole {
        identity: requested,
        reply,
    }) = command_rx.recv().await.unwrap()
    else {
        panic!("expected an identity unblackhole command");
    };
    assert_eq!(requested, identity);
    reply
        .send(Ok(crate::routing::UnblackholeIdentityOutcome::Removed))
        .unwrap();
    assert_eq!(
        unblackholing.await.unwrap(),
        Ok(crate::routing::UnblackholeIdentityOutcome::Removed)
    );
}

#[tokio::test]
async fn identity_blackhole_capabilities_report_a_stopped_reactor() {
    let (handle, command_rx) = handle();
    drop(command_rx);
    let identity = crate::identity::IdentityHash::new([0x31; 16]);
    let source = crate::identity::IdentityHash::new([0x41; 16]);

    assert_eq!(
        handle.blackholed_identities().await,
        Err(IdentityBlackholeSourceError::NodeStopped)
    );
    assert_eq!(
        handle.is_blackholed(identity).await,
        Err(IdentityBlackholeSourceError::NodeStopped)
    );
    assert_eq!(
        handle
            .blackhole_identity(BlackholedIdentity {
                identity,
                source,
                expiry: crate::routing::BlackholeExpiry::Indefinite,
                reason: None,
            })
            .await,
        Err(IdentityBlackholeControlError::NodeStopped)
    );
    assert_eq!(
        handle.unblackhole_identity(identity).await,
        Err(IdentityBlackholeControlError::NodeStopped)
    );
}

struct StatusInterface {
    tag: std::vec::Vec<u8>,
    status: TokioInterfaceStatus,
}

impl StatusInterface {
    fn new(tag: &[u8]) -> Self {
        let id = InterfaceId::from_channel_tag(InterfaceKind::Pipe, tag);
        Self {
            tag: tag.to_vec(),
            status: TokioInterfaceStatus::new(id, crate::interfaces::ConnectionState::Connected),
        }
    }

    fn id(&self) -> InterfaceId {
        self.status.id()
    }
}

impl Interface for StatusInterface {
    const HW_MTU: usize = crate::wire::BROADCAST_MTU;
    const KIND: InterfaceKind = InterfaceKind::Pipe;

    fn channel_tag(&self) -> &[u8] {
        &self.tag
    }

    fn descriptor(&self) -> crate::interfaces::InterfaceDescriptor {
        crate::interfaces::InterfaceDescriptor {
            id: self.id(),
            capabilities: crate::interfaces::InterfaceCapabilities {
                ingress: crate::interfaces::IngressCapability::Enabled,
                egress: crate::interfaces::EgressCapability::Enabled(
                    crate::interfaces::TransportCapability::CrossInterfaceOnly,
                ),
            },
            mode: crate::interfaces::InterfaceMode::Full,
            bitrate: crate::interfaces::BitrateBps::guess(1_000_000),
            hardware_mtu: None,
            announce_rate_limit: None,
            announce_bandwidth_cap: crate::interfaces::AnnounceBandwidthCap::Unlimited,
            airtime_duty_cycle: None,
            common: crate::interfaces::InterfaceCommonPolicy::RNS_DEFAULT,
        }
    }

    async fn run<S: crate::reactor::interface_seam::InterfaceSeam>(self, _seam: S) {}
}

impl ReportsStatus for StatusInterface {
    fn status_view(&self) -> Option<StatusView> {
        let status = self.status.clone();
        Some(Arc::new(move || std::vec![InterfaceVitals::of(&status)]))
    }

    fn connection_view(&self) -> Option<ConnectionView> {
        Some(ConnectionView::of(self.status.clone()))
    }
}

#[tokio::test]
async fn runtime_attachment_carries_ifac_wire_and_status_metadata() {
    let (handle, mut command_rx) = handle();
    let interface = StatusInterface::new(b"protected-wire");
    let id = interface.id();
    let ifac = IfacContext::derive(Some("private-net"), Some("secret"), IfacSize::WIDE).unwrap();
    let signature = ifac.ifac_signature();
    let _attached =
        handle.add_interface_with_ifac_name(interface, ifac, Some("private-net".into()));

    let HostCommand::AddInterface(add) = command_rx.recv().await.unwrap() else {
        panic!("expected an interface add");
    };
    assert_eq!(
        add.connection.as_ref().map(ConnectionView::connection),
        Some(crate::interfaces::ConnectionState::Connected)
    );
    let wire_ifac = add.ifac.unwrap();
    assert_eq!(wire_ifac.ifac_signature(), signature);
    assert_eq!(wire_ifac.ifac_size(), IfacSize::WIDE);
    assert!(handle.set_interface_name(id, "Protected wire"));

    assert_eq!(
        handle.interface_inventory(),
        std::vec![InterfaceInventoryEntry {
            name: Some("Protected wire".into()),
            origin: InterfaceOriginKind::Configured,
            snapshot: InterfaceSnapshot {
                id,
                connection: crate::interfaces::ConnectionState::Connected,
                failure_reason: None,
                rx_bytes: 0,
                tx_bytes: 0,
                transfer_rates: None,
                destinations: 0,
                links: 0,
                transported_links: 0,
                membership: Membership::Independent,
            },
            ifac: Some(InterfaceIfacSnapshot {
                signature,
                size: IfacSize::WIDE,
                network_name: Some("private-net".into()),
            }),
        }]
    );
}

#[tokio::test]
async fn a_fleet_member_inherits_its_supervisors_ifac() {
    let supervisor = InterfaceId::new([0x71; 8]);
    let (mut fleet, mut tail) = Fleet::detached(supervisor);
    let ifac = IfacContext::derive(Some("fleet-net"), None, IfacSize::NARROW).unwrap();
    let signature = ifac.ifac_signature();
    fleet.ifac = Some(RuntimeIfac {
        context: ifac,
        network_name: Some("fleet-net".into()),
    });
    let interface = StatusInterface::new(b"fleet-member");
    let id = interface.id();
    let _attached = fleet.add(interface);

    let HostCommand::AddInterface(add) = tail._commands.recv().await.unwrap() else {
        panic!("expected a fleet member add");
    };
    assert_eq!(add.ifac.unwrap().ifac_signature(), signature);
    let map = fleet.interfaces.lock().unwrap();
    assert_eq!(
        map.get(&id)
            .unwrap()
            .ifac
            .as_ref()
            .unwrap()
            .network_name
            .as_deref(),
        Some("fleet-net")
    );
}

#[tokio::test]
async fn request_emits_a_request_any_and_returns_the_response_with_its_rtt() {
    let (handle, mut command_rx) = handle();
    let link = LinkId::new([5; 16]);
    let path_hash = RequestPathHash::new([0x44; 16]);

    let requesting = tokio::spawn(async move { handle.request(link, path_hash, b"ping").await });

    let HostCommand::RequestAny(request) = command_rx.recv().await.unwrap() else {
        panic!("request issues a RequestAny host command");
    };
    assert_eq!(request.link_id, link);
    assert_eq!(request.path_hash, path_hash);
    assert_eq!(request.data.as_slice(), &b"ping"[..]);
    request
        .completion
        .send(Ok((b"pong".to_vec(), RttMillis::new(42))))
        .unwrap();

    let (data, rtt) = requesting.await.unwrap().unwrap();
    assert_eq!(data, b"pong");
    assert_eq!(rtt, RttMillis::new(42));
}

#[tokio::test]
async fn respond_returns_the_links_round_trip() {
    use crate::routing::links::request::RequestId;
    use crate::runtime::request_router::RespondToken;

    let (handle, _command_rx) = handle();
    let token = RespondToken {
        link_id: LinkId::new([1; 16]),
        request_id: RequestId([2; 16]),
        rtt: RttMillis::new(99),
    };
    assert_eq!(
        handle.respond(token, b"answer"),
        Some(RttMillis::new(99)),
        "respond surfaces the rtt the request arrived on",
    );
}

#[tokio::test]
async fn a_large_response_carries_a_bz2_candidate() {
    use crate::routing::links::request::RequestId;
    use crate::runtime::request_router::RespondToken;

    let (handle, mut command_rx) = handle();
    let token = RespondToken {
        link_id: LinkId::new([1; 16]),
        request_id: RequestId([2; 16]),
        rtt: RttMillis::new(50),
    };
    let body = std::vec![42u8; RESPONSE_PACKET_CEILING + 4096];
    assert_eq!(handle.respond(token, &body), Some(RttMillis::new(50)));
    let Some(HostCommand::RespondAny(respond)) = command_rx.recv().await else {
        panic!("expected a RespondAny command");
    };
    assert_eq!(
        respond
            .compressed_candidate
            .as_ref()
            .map(|c| c.as_slice().to_vec()),
        compression::compress_if_smaller(&body),
        "a response past the packet ceiling rides a bz2 candidate matching the codec",
    );
    assert!(respond.compressed_candidate.is_some(), "a run compresses");
}

#[tokio::test]
async fn a_packet_sized_response_skips_compression() {
    use crate::routing::links::request::RequestId;
    use crate::runtime::request_router::RespondToken;

    let (handle, mut command_rx) = handle();
    let token = RespondToken {
        link_id: LinkId::new([1; 16]),
        request_id: RequestId([2; 16]),
        rtt: RttMillis::new(50),
    };
    let body = std::vec![42u8; RESPONSE_PACKET_CEILING];
    handle.respond(token, &body);
    let Some(HostCommand::RespondAny(respond)) = command_rx.recv().await else {
        panic!("expected a RespondAny command");
    };
    assert!(
        respond.compressed_candidate.is_none(),
        "a response that fits a packet never builds a candidate the rung would discard",
    );
}

#[tokio::test]
async fn a_self_completing_interface_run_deregisters_it() {
    let (msg_tx, msg_rx) = mpsc::unbounded_channel::<DriverMsg>();
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<HostCommand>();

    let id = InterfaceId::from_channel_tag(
        crate::interfaces::InterfaceKind::LocalClient,
        b"ephemeral-peer",
    );
    msg_tx
        .send(DriverMsg::Add {
            id,
            supervisor: None,
            build: Box::new(|| {
                let run: Pin<Box<dyn Future<Output = ()>>> = Box::pin(async {});
                run
            }),
        })
        .expect("the driver is listening");
    drop(msg_tx);

    let interfaces = Arc::new(Mutex::new(HashMap::new()));
    tokio::join!(
        drive_interfaces(std::vec![], msg_rx, cmd_tx, interfaces),
        async {
            let command = tokio::time::timeout(std::time::Duration::from_secs(1), cmd_rx.recv())
                .await
                .expect("the driver culls the completed interface within 1s")
                .expect("the command channel stays open");
            assert!(
                    matches!(
                        command,
                        HostCommand::RemoveInterface {
                            id: removed,
                            departure: Departure::MayReturn,
                        } if removed == id
                    ),
                    "an interface whose run ended on its own deregisters itself as a may-return departure"
                );
        }
    );
}

#[tokio::test]
async fn payload_beyond_the_mdu_is_rejected_before_the_wire() {
    let (prns, _command_rx) = handle();
    let oversize = [0u8; MAX_SEND_SINGLE_PACKET_PLAINTEXT_LEN + 1];
    assert_eq!(
        prns.send_single_packet(PEER, &oversize).await,
        Err(SendError::PayloadTooLarge),
    );
}

#[tokio::test]
async fn a_send_on_a_stopped_node_settles_as_node_stopped() {
    let (prns, command_rx) = handle();
    drop(command_rx);
    assert_eq!(
        prns.send_single_packet(PEER, b"ping").await,
        Err(SendError::NodeStopped),
    );
}

#[tokio::test]
async fn an_awaited_send_issues_the_completion_carrying_command() {
    let (prns, mut command_rx) = handle();
    let issuer = prns.clone();
    let send = tokio::spawn(async move { issuer.send_single_packet(PEER, b"ping").await });

    match command_rx.recv().await.expect("the command was issued") {
        HostCommand::AwaitedEngine { issued, completion } => {
            assert!(matches!(issued.command, EngineCommand::SendSinglePacket(_)));
            completion
                .send(Settlement::SendSinglePacket(Ok(PacketReceiptDelivered {
                    rtt: crate::units::RttMillis::new(7),
                })))
                .expect("the awaiter is still parked");
        }
        _ => panic!("send_single must issue an AwaitedEngine command"),
    }

    assert_eq!(
        send.await.expect("the send task joins"),
        Ok(PacketReceiptDelivered {
            rtt: crate::units::RttMillis::new(7),
        }),
    );
}

#[tokio::test]
async fn establish_link_resolves_the_link_id_from_the_settlement() {
    use crate::engine::LinkEstablished;

    let (prns, mut command_rx) = handle();
    let issuer = prns.clone();
    let establish = tokio::spawn(async move { issuer.establish_link(PEER).await });

    match command_rx.recv().await.expect("the command was issued") {
        HostCommand::AwaitedEngine { issued, completion } => {
            assert_eq!(
                issued.command,
                EngineCommand::EstablishLink(EstablishLink { destination: PEER }),
            );
            completion
                .send(Settlement::EstablishLink(Ok(LinkEstablished {
                    link_id: LinkId::new([0x42; 16]),
                    rtt_ms: 11,
                })))
                .expect("the awaiter is still parked");
        }
        _ => panic!("establish_link must issue an AwaitedEngine command"),
    }

    assert_eq!(
        establish.await.expect("the establish task joins"),
        Ok(LinkId::new([0x42; 16])),
    );
}

#[tokio::test]
async fn establish_link_surfaces_a_typed_failure() {
    let (prns, mut command_rx) = handle();
    let issuer = prns.clone();
    let establish = tokio::spawn(async move { issuer.establish_link(PEER).await });

    let HostCommand::AwaitedEngine { completion, .. } =
        command_rx.recv().await.expect("the command was issued")
    else {
        panic!("establish_link must issue an AwaitedEngine command");
    };
    completion
        .send(Settlement::EstablishLink(Err(
            EstablishLinkFailure::Timeout,
        )))
        .expect("the awaiter is still parked");

    assert_eq!(
        establish.await.expect("the establish task joins"),
        Err(SendError::Failed(EstablishLinkFailure::Timeout)),
    );
}

#[tokio::test]
async fn request_path_mints_an_id_and_awaits_the_typed_result() {
    let (prns, mut command_rx) = handle();
    let issuer = prns.clone();
    let requested = tokio::spawn(async move { issuer.request_path(PEER).await });

    let HostCommand::AwaitedEngine { issued, completion } =
        command_rx.recv().await.expect("the command was issued")
    else {
        panic!("request_path must issue an awaited engine command");
    };
    let EngineCommand::RequestPath(request) = issued.command else {
        panic!("request_path must issue its matching engine command");
    };
    assert_eq!(request.destination, PEER);
    completion
        .send(Settlement::RequestPath(Ok(PathFound {
            hops: crate::units::HopCount(3),
        })))
        .expect("the awaiter is still parked");

    assert_eq!(
        requested.await.expect("the request task joins"),
        Ok(PathFound {
            hops: crate::units::HopCount(3),
        })
    );
}

#[tokio::test]
async fn announce_now_awaits_and_surfaces_its_typed_settlement() {
    let (prns, mut command_rx) = handle();
    let command = AnnounceNow {
        destination: PEER,
        target: crate::engine::AnnounceTarget::AllInterfaces,
        app_data: crate::engine::AnnounceAppData::Registered,
    };
    let expected = command.clone();
    let issuer = prns.clone();
    let announced = tokio::spawn(async move { issuer.announce_now(command).await });
    let HostCommand::AwaitedEngine { issued, completion } =
        command_rx.recv().await.expect("the command was issued")
    else {
        panic!("announce_now must issue an awaited engine command");
    };
    assert_eq!(issued.command, EngineCommand::AnnounceNow(expected));
    completion
        .send(Settlement::AnnounceNow(Err(AnnounceNowFailure::Rejected(
            crate::engine::AnnounceNowRejection::UnknownDestination,
        ))))
        .expect("the awaiter is still parked");
    assert_eq!(
        announced.await.expect("the announce task joins"),
        Err(SendError::Failed(AnnounceNowFailure::Rejected(
            crate::engine::AnnounceNowRejection::UnknownDestination,
        ))),
    );
}

#[tokio::test]
async fn byte_stream_reader_is_withheld_until_the_run_loop_acks_registration() {
    let (prns, mut command_rx) = handle();
    let link = LinkId::new([5; 16]);
    let stream = StreamId::new(2).unwrap();
    let opener = prns.clone();
    let open = tokio::spawn(async move { opener.byte_stream_reader(link, stream).await });

    let HostCommand::RegisterStreamReader {
        link_id,
        stream_id,
        ready,
        ..
    } = command_rx
        .recv()
        .await
        .expect("the registration was issued")
    else {
        panic!("byte_stream_reader must register its sink");
    };
    assert_eq!(link_id, link);
    assert_eq!(stream_id, stream);
    assert!(
        !open.is_finished(),
        "the reader is held back until the run loop acknowledges the registration",
    );

    ready.send(()).expect("the opener is parked on the ack");
    open.await.expect("the reader future resolves once acked");
}

#[test]
fn the_prns_node_api_trait_dispatches_to_the_handle() {
    use crate::routing::links::LinkId;
    use crate::runtime::PrnsNodeApi;

    let (prns, mut command_rx) = handle();
    let queued = PrnsNodeApi::close_link(&prns, LinkId::new([3; 16]));
    assert!(
        queued,
        "the trait method reaches the handle and queues the close"
    );
    assert!(
        matches!(command_rx.try_recv(), Ok(HostCommand::Engine(_))),
        "dispatched through PrnsNodeApi, the close rode the channel"
    );
}

const LINK: LinkId = LinkId::new([5; 16]);

#[tokio::test]
async fn send_resource_drains_a_source_into_proven_segments() {
    let (prns, mut command_rx) = handle();
    let total_len = MAX_EFFICIENT_SIZE as u64 + 100;
    let payload: std::vec::Vec<u8> = (0..total_len).map(|i| i as u8).collect();

    let drainer = tokio::spawn(async move {
        let mut got = std::vec::Vec::new();
        loop {
            let Some(HostCommand::SendResourceSegment(seg)) = command_rx.recv().await else {
                panic!("expected a SendResourceSegment command");
            };
            let last = seg.segment_index == seg.total_segments;
            if seg.segment_index == 1 {
                assert!(
                    seg.compressed_candidate.is_some(),
                    "a compressible split segment carries its bz2 candidate",
                );
            }
            got.push((
                seg.segment_index,
                seg.total_segments,
                seg.data.as_slice().to_vec(),
            ));
            seg.completion
                .send(Settlement::SendResource(Ok(())))
                .expect("the awaiter is still parked");
            if last {
                break;
            }
        }
        got
    });

    prns.send_resource(LINK, total_len, &payload[..])
        .await
        .expect("the stream completes");
    let got = drainer.await.unwrap();

    assert_eq!(got.len(), 2, "a payload one segment over splits in two");
    assert_eq!((got[0].0, got[0].1), (1, 2));
    assert_eq!((got[1].0, got[1].1), (2, 2));
    assert_eq!(got[0].2.len(), MAX_EFFICIENT_SIZE);
    assert_eq!(got[1].2.len(), 100);
    let mut reassembled = got[0].2.clone();
    reassembled.extend_from_slice(&got[1].2);
    assert_eq!(
        reassembled, payload,
        "the segments reassemble to the source"
    );
}

#[tokio::test]
async fn a_small_send_resource_is_one_unsplit_segment() {
    let (prns, mut command_rx) = handle();
    let payload = std::vec![3u8; 500];
    let drainer = tokio::spawn(async move {
        let Some(HostCommand::SendResourceSegment(seg)) = command_rx.recv().await else {
            panic!("expected a SendResourceSegment command");
        };
        let placement = (
            seg.segment_index,
            seg.total_segments,
            seg.data.as_slice().len(),
        );
        seg.completion
            .send(Settlement::SendResource(Ok(())))
            .expect("the awaiter is still parked");
        placement
    });
    prns.send_resource(LINK, 500, &payload[..])
        .await
        .expect("the single segment completes");
    assert_eq!(
        drainer.await.unwrap(),
        (1, 1, 500),
        "a sub-segment payload crosses as one unsplit resource",
    );
}

#[tokio::test]
async fn a_resource_length_that_overflows_with_metadata_is_rejected() {
    let (prns, mut command_rx) = handle();
    let error = prns
        .send_resource_with_metadata(LINK, u64::MAX, &[][..], &[0x81])
        .await
        .unwrap_err();
    assert!(matches!(error, ResourceSendError::UnrepresentableLength));
    assert!(command_rx.try_recv().is_err());
}

#[test]
fn a_split_resource_claim_cannot_raise_the_per_segment_inflate_bound() {
    assert_eq!(
        resource_segment_decompression_bound(u64::MAX),
        MAX_EFFICIENT_SIZE as u64,
    );
    assert_eq!(resource_segment_decompression_bound(4096), 4096);
}

#[tokio::test]
async fn send_resource_compresses_a_compressible_segment() {
    let (prns, mut command_rx) = handle();
    let payload = std::vec![7u8; 8192];
    let drainer = tokio::spawn(async move {
        let Some(HostCommand::SendResourceSegment(seg)) = command_rx.recv().await else {
            panic!("expected a SendResourceSegment command");
        };
        let candidate = seg
            .compressed_candidate
            .as_ref()
            .map(|c| c.as_slice().to_vec());
        seg.completion
            .send(Settlement::SendResource(Ok(())))
            .expect("the awaiter is still parked");
        candidate
    });
    prns.send_resource(LINK, payload.len() as u64, &payload[..])
        .await
        .expect("the single segment completes");
    let candidate = drainer.await.unwrap();
    assert_eq!(
        candidate,
        compression::compress_if_smaller(&payload),
        "the segment rides a bz2 candidate matching the codec",
    );
    assert!(
        candidate.is_some_and(|c| c.len() < payload.len()),
        "a run of one byte compresses far below its length",
    );
}

#[tokio::test]
async fn send_resource_declines_to_compress_incompressible_data() {
    let (prns, mut command_rx) = handle();
    let mut x = 0x9e37_79b9_7f4a_7c15u64;
    let payload: std::vec::Vec<u8> = (0..8192)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    let drainer = tokio::spawn(async move {
        let Some(HostCommand::SendResourceSegment(seg)) = command_rx.recv().await else {
            panic!("expected a SendResourceSegment command");
        };
        let compressed = seg.compressed_candidate.is_some();
        seg.completion
            .send(Settlement::SendResource(Ok(())))
            .expect("the awaiter is still parked");
        compressed
    });
    prns.send_resource(LINK, payload.len() as u64, &payload[..])
        .await
        .expect("the single segment completes");
    assert!(
        !drainer.await.unwrap(),
        "high-entropy bytes carry no candidate, so the transfer stays uncompressed",
    );
}

#[tokio::test]
async fn never_compression_ships_a_compressible_segment_uncompressed() {
    let (prns, mut command_rx) = handle();
    let payload = std::vec![7u8; 8192];
    let drainer = tokio::spawn(async move {
        let Some(HostCommand::SendResourceSegment(seg)) = command_rx.recv().await else {
            panic!("expected a SendResourceSegment command");
        };
        let compressed = seg.compressed_candidate.is_some();
        seg.completion
            .send(Settlement::SendResource(Ok(())))
            .expect("the awaiter is still parked");
        compressed
    });
    prns.send_resource_with_compression(
        LINK,
        payload.len() as u64,
        &payload[..],
        SegmentCompression::Never,
    )
    .await
    .expect("the single segment completes");
    assert!(
        !drainer.await.unwrap(),
        "RNS auto_compress=False: no attempt, even on a run that would compress",
    );
}

#[tokio::test]
async fn a_segment_past_the_attempt_ceiling_ships_uncompressed() {
    let (prns, mut command_rx) = handle();
    let payload = std::vec![7u8; 8192];
    let drainer = tokio::spawn(async move {
        let Some(HostCommand::SendResourceSegment(seg)) = command_rx.recv().await else {
            panic!("expected a SendResourceSegment command");
        };
        let compressed = seg.compressed_candidate.is_some();
        seg.completion
            .send(Settlement::SendResource(Ok(())))
            .expect("the awaiter is still parked");
        compressed
    });
    prns.send_resource_with_compression(
        LINK,
        payload.len() as u64,
        &payload[..],
        SegmentCompression::Attempt {
            up_to_byte_len: payload.len() as u64 - 1,
        },
    )
    .await
    .expect("the single segment completes");
    assert!(
        !drainer.await.unwrap(),
        "RNS auto_compress=<int>: a segment over the ceiling is never attempted",
    );
}

#[tokio::test]
async fn send_resource_surfaces_a_segment_rejection_and_stops() {
    let (prns, mut command_rx) = handle();
    let total_len = 2 * MAX_EFFICIENT_SIZE as u64 + 100;
    let payload = std::vec![7u8; total_len as usize];
    let drainer = tokio::spawn(async move {
        let mut issued = 0u32;
        while let Some(command) = command_rx.recv().await {
            let HostCommand::SendResourceSegment(seg) = command else {
                panic!("expected a SendResourceSegment command");
            };
            issued += 1;
            let _ = seg.completion.send(Settlement::SendResource(Err(
                SendResourceFailure::RejectedByPeer,
            )));
        }
        issued
    });

    let result = prns.send_resource(LINK, total_len, &payload[..]).await;
    assert!(matches!(
        result,
        Err(ResourceSendError::Rejected(
            SendResourceFailure::RejectedByPeer
        )),
    ));
    drop(prns);
    assert_eq!(
            drainer.await.unwrap(),
            ENGINE_SEGMENT_LANES as u32,
            "a rejected first segment stops the stream — only its already-staged follower ever issued, the third never does",
        );
}

#[tokio::test]
async fn send_resource_on_a_stopped_node_is_node_stopped() {
    let (prns, command_rx) = handle();
    drop(command_rx);
    let payload = std::vec![0u8; 10];
    assert!(matches!(
        prns.send_resource(LINK, 10, &payload[..]).await,
        Err(ResourceSendError::NodeStopped),
    ));
}

#[tokio::test]
async fn receive_resource_streams_an_inbound_resource_into_the_sink() {
    let (prns, mut command_rx) = handle();
    let original = ResourceHash::new([9; 32]);

    let actor = tokio::spawn(async move {
        let Some(HostCommand::RegisterResourceSink {
            link_id,
            sink,
            ready,
        }) = command_rx.recv().await
        else {
            panic!("expected a RegisterResourceSink command");
        };
        ready.send(()).expect("the receiver awaits registration");
        sink.send(ResourceInbound::Chunk(b"hello ".to_vec()))
            .unwrap();
        sink.send(ResourceInbound::Chunk(b"world".to_vec()))
            .unwrap();
        sink.send(ResourceInbound::Complete {
            original_hash: original,
            total_size: 11,
        })
        .unwrap();
        link_id
    });

    let mut buf = std::vec::Vec::new();
    let receipt = prns
        .receive_resource(LINK, &mut buf)
        .await
        .expect("the resource arrives");
    assert_eq!(
        actor.await.unwrap(),
        LINK,
        "the sink registered on the link"
    );
    assert_eq!(
        buf, b"hello world",
        "the chunks stream into the sink in order"
    );
    assert_eq!(
        receipt,
        ResourceReceipt {
            original_hash: original,
            total_size: 11,
            metadata: None,
        },
    );
}

#[tokio::test]
async fn receive_resource_carries_metadata_on_the_receipt() {
    let (prns, mut command_rx) = handle();
    let original = ResourceHash::new([9; 32]);

    let actor = tokio::spawn(async move {
        let Some(HostCommand::RegisterResourceSink { sink, ready, .. }) = command_rx.recv().await
        else {
            panic!("expected a RegisterResourceSink command");
        };
        ready.send(()).expect("the receiver awaits registration");
        sink.send(ResourceInbound::Metadata(b"packed".to_vec()))
            .unwrap();
        sink.send(ResourceInbound::Chunk(b"payload".to_vec()))
            .unwrap();
        sink.send(ResourceInbound::Complete {
            original_hash: original,
            total_size: 7,
        })
        .unwrap();
    });

    let mut buf = std::vec::Vec::new();
    let receipt = prns
        .receive_resource(LINK, &mut buf)
        .await
        .expect("the resource arrives");
    actor.await.unwrap();
    assert_eq!(buf, b"payload", "the metadata never enters the byte stream");
    assert_eq!(
        receipt,
        ResourceReceipt {
            original_hash: original,
            total_size: 7,
            metadata: Some(b"packed".to_vec()),
        },
    );
}

#[tokio::test]
async fn receive_resource_surfaces_a_failed_transfer() {
    let (prns, mut command_rx) = handle();
    let actor = tokio::spawn(async move {
        let Some(HostCommand::RegisterResourceSink { sink, ready, .. }) = command_rx.recv().await
        else {
            panic!("expected a RegisterResourceSink command");
        };
        ready.send(()).unwrap();
        sink.send(ResourceInbound::Failed).unwrap();
    });
    let mut buf = std::vec::Vec::new();
    let result = prns.receive_resource(LINK, &mut buf).await;
    actor.await.unwrap();
    assert!(matches!(result, Err(ResourceReceiveError::Failed)));
    assert!(buf.is_empty(), "a failed transfer wrote nothing");
}

#[tokio::test]
async fn receive_resource_on_a_stopped_node_is_node_stopped() {
    let (prns, command_rx) = handle();
    drop(command_rx);
    let mut buf = std::vec::Vec::new();
    assert!(matches!(
        prns.receive_resource(LINK, &mut buf).await,
        Err(ResourceReceiveError::NodeStopped),
    ));
}
