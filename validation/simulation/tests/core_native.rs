#![allow(clippy::unwrap_used)]
use personal_rns::{
    identity::vault::IdentitySecretKey, interfaces::*, remote_control::*, runtime::*,
};
use prns_runtime_tokio::manifold::interface_seam::{Interface, InterfaceSeam};
use std::{
    num::NonZeroUsize,
    sync::{Arc, Mutex},
    time::Duration,
};
const FRAMES: usize = 64;
const DEADLINE: Duration = Duration::from_secs(10);
struct Loopback {
    id: InterfaceId,
    inbound: tokio::sync::mpsc::Receiver<Vec<u8>>,
    outbound: tokio::sync::mpsc::Sender<Vec<u8>>,
}
impl Interface for Loopback {
    const HW_MTU: usize = personal_rns::wire::BROADCAST_MTU;
    const KIND: InterfaceKind = InterfaceKind::Loopback;
    fn channel_tag(&self) -> &[u8] {
        self.id.as_bytes()
    }
    fn descriptor(&self) -> InterfaceDescriptor {
        InterfaceDescriptor {
            id: self.id,
            capabilities: InterfaceCapabilities {
                ingress: IngressCapability::Enabled,
                egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
            },
            mode: InterfaceMode::Full,
            gravity: InterfaceGravity::ZERO,
            bitrate: BitrateBps::guess(1_000_000),
            hardware_mtu: None,
            announce_rate_limit: None,
            announce_bandwidth_cap: AnnounceBandwidthCap::Unlimited,
            airtime_duty_cycle: None,
            common: InterfaceCommonPolicy::RNS_DEFAULT,
        }
    }
    async fn run<S: InterfaceSeam>(mut self, mut seam: S) {
        loop {
            tokio::select! { frame = self.inbound.recv() => match frame { Some(bytes) => seam.next_inbound(&bytes).await, None => return }, frame = seam.next_outbound() => { if self.outbound.send(frame.to_vec()).await.is_err() { return; } } }
        }
    }
}
impl ReportsStatus for Loopback {}
struct Echo {
    expected: personal_rns::identity::IdentityHash,
}
impl RemoteControlAppMessages<()> for Echo {
    async fn handle_app_message(
        &self,
        _: &(),
        identity: personal_rns::identity::IdentityHash,
        bytes: &[u8],
    ) -> Result<RemoteControlAppMessage, RemoteControlHostCommandError> {
        assert_eq!(identity, self.expected);
        RemoteControlAppMessage::from_slice(bytes)
            .map_err(|_| RemoteControlHostCommandError::ApplyFailed)
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Controller,
    Target,
}
#[derive(Clone, PartialEq, Eq)]
enum Fact {
    Announce(personal_rns::wire::DestinationHash),
    Identified(
        personal_rns::routing::links::LinkId,
        personal_rns::identity::IdentityHash,
    ),
}
#[derive(Clone)]
struct Readiness {
    facts: Arc<Mutex<Vec<(Role, Fact)>>>,
    changed: Arc<tokio::sync::Notify>,
}
impl Readiness {
    fn new() -> Self {
        Self {
            facts: Arc::new(Mutex::new(Vec::new())),
            changed: Arc::new(tokio::sync::Notify::new()),
        }
    }
    fn observe(&self, role: Role, event: PrnsEvent<'_>) {
        let fact = match event {
            PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) => {
                Fact::Announce(destination)
            }
            PrnsEvent::Diagnostic(Diagnostic::PeerIdentified { link_id, identity }) => {
                Fact::Identified(link_id, identity)
            }
            _ => return,
        };
        let mut facts = self.facts.lock().unwrap();
        assert!(facts.len() < 8, "bounded native readiness evidence");
        facts.push((role, fact));
        self.changed.notify_one();
    }
    async fn until(&self, role: Role, fact: Fact) {
        loop {
            let changed = self.changed.notified();
            if self.facts.lock().unwrap().contains(&(role, fact.clone())) {
                return;
            }
            changed.await;
        }
    }
}
fn secrets(role: Role) -> RemoteControlNodeIdentitySecrets {
    let index = match role {
        Role::Controller => 0,
        Role::Target => 1,
    };
    RemoteControlNodeIdentitySecrets::new(
        RemoteControlControllerIdentitySecret::from(IdentitySecretKey::new([0x71 + index * 2; 64])),
        RemoteControlTargetIdentitySecret::from(IdentitySecretKey::new([0x72 + index * 2; 64])),
    )
    .unwrap()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_one_and_four_worker_nodes_deliver_verified_remote_control_requests() {
    use personal_rns::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget};
    for workers in [1, 4] {
        let controller = secrets(Role::Controller)
            .identities()
            .controller()
            .identity_hash();
        let grants = [RemoteControlControllerGrant::new(
            *secrets(Role::Controller).identities().controller(),
            RemoteControlControllerAuthority::Operator,
            RemoteControlRequestSet::only(RemoteControlRequestKind::AppMessage),
        )
        .unwrap()];
        let readiness = Readiness::new();
        let make = |role| {
            let observed = readiness.clone();
            PrnsNode::new(PrnsNodeRecipe {
                remote_control: RemoteControlNodeSetup::new(
                    RemoteControlService::with_capabilities(
                        secrets(role),
                        RemoteControlInitialControllerGrants::Grants(
                            RemoteControlControllerGrants::try_from(grants.as_slice()).unwrap(),
                        ),
                        RemoteControlSelfAnnouncement::Unavailable,
                        RemoteControlCapabilities::describe_only()
                            .with_request(RemoteControlRequestKind::AppMessage),
                    ),
                )
                .with_handlers(
                    NoRemoteControlHostControls,
                    Echo {
                        expected: controller,
                    },
                ),
                transport_identity: None,
                pre_configured_destinations: [],
                app_state: (),
                storage: personal_rns::storage::GrowableHeap,
                request_endpoints: personal_rns::request_endpoints![],
                interfaces: ManuallyAttached,
                persistence: NoPersistence,
                on_event: move |event, _: &()| observed.observe(role, event),
            })
            .with_crypto_pool(CryptoPoolConfig::Pooled {
                workers: PoolWorkers::Fixed(NonZeroUsize::new(workers).unwrap()),
                placement: CryptoWorkerPlacement::SchedulerManaged,
            })
        };
        let left = make(Role::Controller);
        let right = make(Role::Target);
        let left_handle = left.handle();
        let right_handle = right.handle();
        let (left_tx, left_rx) = tokio::sync::mpsc::channel(FRAMES);
        let (right_tx, right_rx) = tokio::sync::mpsc::channel(FRAMES);
        left_handle.add_interface(Loopback {
            id: InterfaceId::from_channel_tag(InterfaceKind::Loopback, b"native-left"),
            inbound: left_rx,
            outbound: right_tx,
        });
        right_handle.add_interface(Loopback {
            id: InterfaceId::from_channel_tag(InterfaceKind::Loopback, b"native-right"),
            inbound: right_rx,
            outbound: left_tx,
        });
        let (stop_left, stopped_left) = tokio::sync::oneshot::channel();
        let (stop_right, stopped_right) = tokio::sync::oneshot::channel();
        let exercise = async move {
            let target = secrets(Role::Target)
                .identities()
                .target()
                .endpoint()
                .destination_hash();
            right_handle
                .announce_now(AnnounceNow {
                    destination: target,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                })
                .await
                .unwrap();
            readiness
                .until(Role::Controller, Fact::Announce(target))
                .await;
            let link = left_handle.establish_link(target).await.unwrap();
            left_handle.identify(link, controller).await.unwrap();
            readiness
                .until(Role::Target, Fact::Identified(link, controller))
                .await;
            for marker in 0..8 {
                let first = RemoteControlAppMessage::from_slice(&[marker; 96]).unwrap();
                let second = RemoteControlAppMessage::from_slice(&[marker + 8; 96]).unwrap();
                let remote = left_handle.remote_control(link);
                let (left, right) = tokio::join!(
                    remote.app_message(first.clone()),
                    remote.app_message(second.clone())
                );
                assert_eq!(left.unwrap().0, first);
                assert_eq!(right.unwrap().0, second);
            }
            stop_left.send(()).unwrap();
            stop_right.send(()).unwrap();
        };
        let bounded = tokio::time::timeout(DEADLINE, exercise);
        let (left_result, right_result, result) = tokio::join!(
            left.run_until(async {
                let _ = stopped_left.await;
            }),
            right.run_until(async {
                let _ = stopped_right.await;
            }),
            bounded
        );
        result.unwrap();
        assert!(left_result.is_ok());
        assert!(right_result.is_ok());
    }
}
