use core::time::Duration;
use personal_rns::runtime::NoPersistence;
use std::net::Ipv4Addr;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, PrnsCommand, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::wifi_direct::{
    DataPlanePlan, GoIntent, GroupRole, Initiative, PeerEvidence, SegmentAddress,
};
use personal_rns::interfaces::wifi_direct::{
    DiscoveryMode, WifiDirectBackend, WifiDirectEvent, WifiDirectGroup,
};
use personal_rns::interfaces::MacAddress;
use personal_rns::request_endpoints;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{
    Diagnostic, ManuallyAttached, PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeRecipe,
    ServeMyRequestEndpoints,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::wifi_direct::WifiDirectAuto;
use tokio::sync::mpsc;

fn secret(byte: u8) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    Zeroizing::new([byte; IDENTITY_SECRET_KEY_LEN])
}

fn single(identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>) -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        resource_strategy: ResourceStrategy::AcceptNone,
        app_name: "bench",
        aspects: &["link"],
        identity,
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        maximum_request_bytes: Default::default(),
        request_endpoints: ServeMyRequestEndpoints::No,
    }
}

enum Wire {
    Invite { from: MacAddress },
    Accepted,
}

struct LoopbackGroup {
    role: GroupRole,
    plan: DataPlanePlan,
}

impl WifiDirectGroup for LoopbackGroup {
    fn role(&self) -> GroupRole {
        self.role
    }

    fn data_plane(&self) -> DataPlanePlan {
        self.plan
    }
}

struct LoopbackWifiDirectBackend {
    local: MacAddress,
    peer: MacAddress,
    sighting_pending: bool,
    queued: Option<WifiDirectEvent<LoopbackGroup>>,
    to_peer: mpsc::Sender<Wire>,
    from_peer: mpsc::Receiver<Wire>,
}

impl LoopbackWifiDirectBackend {
    fn pair() -> (Self, Self) {
        let addr_a = MacAddress::new([0xAA; 6]);
        let addr_b = MacAddress::new([0xBB; 6]);
        let (a_tx, a_rx) = mpsc::channel(8);
        let (b_tx, b_rx) = mpsc::channel(8);
        let a = Self {
            local: addr_a,
            peer: addr_b,
            sighting_pending: true,
            queued: None,
            to_peer: b_tx,
            from_peer: a_rx,
        };
        let b = Self {
            local: addr_b,
            peer: addr_a,
            sighting_pending: false,
            queued: None,
            to_peer: a_tx,
            from_peer: b_rx,
        };
        (a, b)
    }
}

impl WifiDirectBackend for LoopbackWifiDirectBackend {
    type Error = std::convert::Infallible;
    type Group = LoopbackGroup;

    async fn set_discovery(&mut self, _mode: DiscoveryMode) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn form_group(&mut self, _peer: MacAddress, _intent: GoIntent) {
        let _ = self.to_peer.send(Wire::Invite { from: self.local }).await;
    }

    async fn accept_invitation(&mut self, _peer: MacAddress, _intent: GoIntent) {
        let _ = self.to_peer.send(Wire::Accepted).await;
        self.queued = Some(WifiDirectEvent::GroupFormed {
            group: LoopbackGroup {
                role: GroupRole::Owner,
                plan: DataPlanePlan::HostRendezvous {
                    local: SegmentAddress::V4(Ipv4Addr::LOCALHOST),
                },
            },
        });
    }

    async fn remove_group(&mut self) {}

    async fn next_event(&mut self) -> WifiDirectEvent<LoopbackGroup> {
        if self.sighting_pending {
            self.sighting_pending = false;
            return WifiDirectEvent::Sighting {
                peer: self.peer,
                evidence: PeerEvidence::ServiceRecord,
                initiative: Initiative::Ours,
            };
        }
        if let Some(event) = self.queued.take() {
            return event;
        }
        match self.from_peer.recv().await {
            Some(Wire::Invite { from }) => WifiDirectEvent::Invitation { peer: from },
            Some(Wire::Accepted) => WifiDirectEvent::GroupFormed {
                group: LoopbackGroup {
                    role: GroupRole::Client,
                    plan: DataPlanePlan::DialOwner {
                        owner: SegmentAddress::V4(Ipv4Addr::LOCALHOST),
                    },
                },
            },
            None => std::future::pending().await,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wifi_direct_group_forms_and_carries_an_announce_between_two_nodes() {
    let single_a = single(secret(0xE1));
    let dest_a = single_a
        .destination_hash()
        .expect("the test destination name is valid");

    let (backend_a, backend_b) = LoopbackWifiDirectBackend::pair();

    let node_a = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [single_a],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: |_event, _state| {},
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
    });
    let commands_a = node_a.handle();
    let _sup_a = commands_a.supervise(WifiDirectAuto::new(backend_a, GoIntent::PREFER_CLIENT));

    let (heard_tx, mut heard_rx) = mpsc::unbounded_channel();
    let node_b = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [single(secret(0xF2))],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = heard_tx.send(destination);
            }
        },
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
    });
    let commands_b = node_b.handle();
    let _sup_b = commands_b.supervise(WifiDirectAuto::new(backend_b, GoIntent::PREFER_OWNER));

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if commands_a
                .issue(PrnsCommand::AnnounceNow(AnnounceNow {
                    destination: dest_a,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }))
                .is_none()
            {
                break;
            }
        }
    });

    let heard = tokio::select! {
        biased;
        heard = tokio::time::timeout(Duration::from_secs(10), heard_rx.recv()) => heard
            .expect("B hears A's announce over the formed group within 10s")
            .expect("the announce channel stays open"),
        result = node_a.run() => unreachable!("node A's run loop returned: {result:?}"),
        result = node_b.run() => unreachable!("node B's run loop returned: {result:?}"),
    };
    assert_eq!(
        heard, dest_a,
        "B heard A's destination across the Wi-Fi Direct data plane"
    );
}
