use std::time::Duration;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, PrnsCommand, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::bluetooth_auto::{
    AppleHost, BleAddress, BleIdentity, BleRoleCapabilities, BlueZHost, Endpoint, LinkCapabilities,
};
use personal_rns::interfaces::{ConnectionState, InterfaceStatus};
use personal_rns::request_endpoints;
use personal_rns::routing::links::LinkId;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::request_endpoints::{
    Decline, RequestContext, RequestEndpoint, RequestEndpointPolicy,
};
use personal_rns::runtime::{
    Diagnostic, NoPersistence, NoRemoteControlHostControls, PreConfiguredDestination, PrnsEvent,
    PrnsNode, PrnsNodeHandle, PrnsNodeRecipe, ServeMyRequestEndpoints,
};
use personal_rns::storage::GrowableHeap;
use prns_interfaces_tokio::bluetooth_auto::BluetoothAuto;
use prns_simulation::ble::{
    BleMediumConfig, VirtualBleBackendConfig, VirtualBleLab, VirtualBleLinkConfig,
};
use prns_simulation::{SimulationDurationInTicks, SimulationTick};

const MAX_PEERS: usize = 4;
const QUERY_PATH: &str = "/simulation/ble-echo";

struct Echo;

impl RequestEndpoint<NoRemoteControlHostControls> for Echo {
    const ENDPOINT_ID: &'static str = QUERY_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

    async fn handle(
        mut context: RequestContext<'_, NoRemoteControlHostControls>,
        _node: &impl personal_rns::runtime::PrnsNodeApi,
    ) -> Result<(), Decline> {
        let asked = context.data;
        context.respond(asked)
    }
}

async fn round_trip(node: &PrnsNodeHandle, link: LinkId, message: &[u8]) {
    let (answer, _) = node
        .request(link, RequestPathHash::of(QUERY_PATH), message)
        .await
        .unwrap_or_else(|error| unreachable!("BLE request must complete: {error:?}"));
    assert_eq!(answer.as_slice(), message);
}

fn backend_config(address: u8, rssi: i8) -> VirtualBleBackendConfig {
    let link = VirtualBleLinkConfig::new(4, 4, 2_048)
        .unwrap_or_else(|error| unreachable!("test link configuration is valid: {error}"));
    VirtualBleBackendConfig::new(
        BleAddress::new([address; 6]),
        rssi,
        BleRoleCapabilities::DualRole,
        SimulationDurationInTicks::from_ticks(1),
        2,
        MAX_PEERS,
        link,
    )
    .unwrap_or_else(|error| unreachable!("test backend configuration is valid: {error}"))
}

#[tokio::test]
async fn production_nodes_exchange_requests_after_link_and_radio_loss() {
    let medium = BleMediumConfig::new(2, 8, 16, 256)
        .unwrap_or_else(|error| unreachable!("test medium configuration is valid: {error}"));
    let lab = VirtualBleLab::new(medium);
    let first_backend = lab
        .attach_backend(backend_config(1, -41))
        .unwrap_or_else(|error| unreachable!("first backend attaches: {error}"));
    let second_backend = lab
        .attach_backend(backend_config(2, -52))
        .unwrap_or_else(|error| unreachable!("second backend attaches: {error}"));
    let capabilities = LinkCapabilities {
        l2cap: None,
        link_mtu: 500,
    };
    let first = BluetoothAuto::<_, MAX_PEERS>::new(
        first_backend,
        BleIdentity::new([1; 16]),
        Endpoint::CoreBluetooth(AppleHost::MacOs),
        capabilities,
    );
    let second = BluetoothAuto::<_, MAX_PEERS>::new(
        second_backend,
        BleIdentity::new([2; 16]),
        Endpoint::BlueZ(BlueZHost::Linux),
        capabilities,
    );
    let first_status = first.status();
    let second_status = second.status();
    let responder = PreConfiguredDestination::Single {
        resource_strategy: personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
        app_name: "simulation",
        aspects: &["ble"],
        identity: Zeroizing::new([0xA7; IDENTITY_SECRET_KEY_LEN]),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        maximum_request_bytes: Default::default(),
        request_endpoints: ServeMyRequestEndpoints::Yes,
    };
    let destination = responder
        .destination_hash()
        .unwrap_or_else(|error| unreachable!("test destination is valid: {error:?}"));
    let node_a = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [responder],
        app_state: NoRemoteControlHostControls,
        storage: GrowableHeap,
        request_endpoints: request_endpoints![Echo],
        on_event: |_event, _state| {},
        interfaces: move |node: &PrnsNodeHandle| {
            let _attached = node.supervise(first);
        },
        persistence: NoPersistence,
    });
    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::channel(1);
    let node_b = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [],
        app_state: NoRemoteControlHostControls,
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard {
                destination: heard, ..
            }) = event
            {
                if heard == destination {
                    let _ = heard_tx.try_send(());
                }
            }
        },
        interfaces: move |node: &PrnsNodeHandle| {
            let _attached = node.supervise(second);
        },
        persistence: NoPersistence,
    });
    let announcer = node_a.handle();
    let initiator = node_b.handle();
    let drive_lab = async {
        let mut connected_at = None;
        for tick in 0..64 {
            tokio::task::yield_now().await;
            lab.advance_to(SimulationTick::from_ticks(tick))
                .unwrap_or_else(|error| unreachable!("bounded test advance succeeds: {error}"));
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
            if first_status.connection() == ConnectionState::Connected
                && second_status.connection() == ConnectionState::Connected
            {
                connected_at = Some(tick);
                break;
            }
        }
        let Some(mut tick) = connected_at else {
            unreachable!("both production supervisors must attach a member")
        };
        assert!(announcer
            .issue(PrnsCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }))
            .is_some());
        assert_eq!(heard_rx.recv().await, Some(()));
        let link = initiator
            .establish_link(destination)
            .await
            .unwrap_or_else(|error| unreachable!("BLE link must establish: {error:?}"));
        round_trip(&initiator, link, b"before-disconnect").await;
        let disconnected = lab.disconnect_between(BleAddress::new([1; 6]), BleAddress::new([2; 6]));
        assert!(disconnected.connections_closed > 0);

        let mut observed_loss = false;
        for _ in 0..64 {
            tokio::task::yield_now().await;
            if first_status.connection() != ConnectionState::Connected
                && second_status.connection() != ConnectionState::Connected
            {
                observed_loss = true;
                break;
            }
        }
        assert!(
            observed_loss,
            "both supervisors must observe the closed link"
        );

        let mut reconnected = false;
        for _ in 0..64 {
            tick += 1;
            lab.advance_to(SimulationTick::from_ticks(tick))
                .unwrap_or_else(|error| unreachable!("bounded test advance succeeds: {error}"));
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
            if first_status.connection() == ConnectionState::Connected
                && second_status.connection() == ConnectionState::Connected
            {
                reconnected = true;
                break;
            }
        }
        assert!(reconnected, "both production supervisors must reconnect");
        round_trip(&initiator, link, b"after-link-loss").await;

        first_status.disable();
        let mut disabled = false;
        for _ in 0..64 {
            tokio::task::yield_now().await;
            if first_status.connection() == ConnectionState::Disabled
                && second_status.connection() != ConnectionState::Connected
                && lab.active_connection_count() == 0
            {
                disabled = true;
                break;
            }
        }
        assert!(disabled, "radio disable must tear down both sides");

        first_status.enable();
        for _ in 0..64 {
            tick += 1;
            lab.advance_to(SimulationTick::from_ticks(tick))
                .unwrap_or_else(|error| unreachable!("bounded test advance succeeds: {error}"));
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
            if first_status.connection() == ConnectionState::Connected
                && second_status.connection() == ConnectionState::Connected
            {
                round_trip(&initiator, link, b"after-radio-restart").await;
                return;
            }
        }
        unreachable!("re-enabled production supervisor must reconnect")
    };

    tokio::select! {
        result = node_a.run() => unreachable!("responder runs until cancelled: {result:?}"),
        result = node_b.run() => unreachable!("initiator runs until cancelled: {result:?}"),
        result = tokio::time::timeout(Duration::from_secs(10), drive_lab) => {
            assert!(result.is_ok(), "BLE recovery scenario timed out");
        }
    }
}
