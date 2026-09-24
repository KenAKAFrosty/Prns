use super::*;
use personal_rns::engine::InstantMillis;
use personal_rns::identity::vault::IdentitySecretKey;
use personal_rns::manifold::tokio::TokioHost;
use personal_rns::runtime::{
    CryptoPoolConfig, InterfaceArbitration, InterfaceEventSource, ManuallyAttached, PrnsNode,
    PrnsNodeRecipe, RemoteControlNodeSetup, TokioHandleEntropy,
};
use personal_rns::storage::GrowableHeap;
use prns_core::entropy::RuntimeEntropy;
use tokio::sync::oneshot;

const TIMELINE_ORIGIN: InstantMillis = InstantMillis(1_000_000);

pub struct Node {
    pub handle: PrnsNodeHandle,
    pub endpoint: EndpointId,
    pub identity: RemoteControlControllerIdentity,
    pub target: RemoteControlTargetIdentity,
    pub(super) task: ManualTaskId,
    pub(super) shutdown: oneshot::Sender<()>,
}

pub(super) fn secrets(node: usize) -> RemoteControlNodeIdentitySecrets {
    let base = 0x31 + node as u8 * 2;
    RemoteControlNodeIdentitySecrets::new(
        RemoteControlControllerIdentitySecret::from(IdentitySecretKey::new([base; 64])),
        RemoteControlTargetIdentitySecret::from(IdentitySecretKey::new([base + 1; 64])),
    )
    .expect("distinct validation identities")
}

impl Lab<'_> {
    pub(super) fn start_node(
        &mut self,
        index: usize,
        policy: ControllerPolicy,
        storage: Option<persistence::Storage>,
    ) {
        self.start_node_with_secrets(index, policy, storage, secrets(index));
    }

    fn start_node_with_secrets(
        &mut self,
        index: usize,
        policy: ControllerPolicy,
        storage: Option<persistence::Storage>,
        secrets: RemoteControlNodeIdentitySecrets,
    ) {
        let timeline_origin = InstantMillis(
            TIMELINE_ORIGIN
                .0
                .checked_add(self.medium.now().get())
                .expect("boot timeline"),
        );
        let generation = self.boot_generations[index];
        self.boot_generations[index] = generation.checked_add(1).expect("boot generation");
        let interface = self
            .medium
            .attach(&(index as u64).to_be_bytes())
            .expect("unique control interface");
        let endpoint = interface.endpoint_id();
        let identity = *secrets.identities().controller();
        let target = RemoteControlTargetIdentity::new(*secrets.identities().target().public_keys());
        let target_destination = target.endpoint().destination_hash();
        let calls = self.calls.clone();
        let gates = self.app_gates.clone();
        let max_invocations = self.budgets.app_invocations;
        let pairing = self.pairing.clone();
        let (ready, mut ready_rx) = oneshot::channel();
        let (shutdown, stopping) = oneshot::channel();
        let task = self
            .runner
            .insert(async move {
                let host_seed = 0x80 + index as u8;
                let host_stream = RuntimeEntropy::try_new(move |output: &mut [u8]| {
                    fill_seed(output, host_seed, generation);
                    Ok::<(), core::convert::Infallible>(())
                })
                .expect("fixed fixture host entropy");
                let handle_stream = RuntimeEntropy::try_new(move |output: &mut [u8]| {
                    fill_seed(output, host_seed + 8, generation);
                    Ok::<(), core::convert::Infallible>(())
                })
                .expect("fixed fixture handle entropy");
                let entropy =
                    TokioHandleEntropy::from_sources(handle_stream, move |output: &mut [u8]| {
                        fill_seed(output, host_seed + 16, generation);
                        Ok::<(), core::convert::Infallible>(())
                    });
                let grants = match policy {
                    ControllerPolicy::Granted(requests) => vec![RemoteControlControllerGrant::new(
                        *self::secrets(CONTROLLER).identities().controller(),
                        RemoteControlControllerAuthority::Operator,
                        requests,
                    )
                    .expect("explicit control grant")],
                    ControllerPolicy::Nobody => vec![],
                    ControllerPolicy::Grants(grants) => grants,
                };
                let persistence = match storage {
                    Some(storage) => PersistenceSelection::Durable(
                        personal_rns::runtime::NodePersistence::custom_dir(storage.directory())
                            .expect("fixture persistence")
                            .with_io_driver(storage.io.clone()),
                    ),
                    None => PersistenceSelection::Disabled,
                };
                let mut node = PrnsNode::new_with_entropy_sources(
                    |handle| {
                        let initial = match grants.as_slice() {
                            [] => RemoteControlInitialControllerGrants::Nobody,
                            grants => RemoteControlInitialControllerGrants::Grants(
                                RemoteControlControllerGrants::try_from(grants)
                                    .expect("one controller"),
                            ),
                        };
                        let mut configured_requests = management_requests();
                        configured_requests.insert(RemoteControlRequestKind::DescribePower);
                        let capabilities =
                            RemoteControlCapabilities::from_requests(configured_requests)
                                .expect("Describe required");
                        PrnsNodeRecipe {
                            remote_control: RemoteControlNodeSetup::new(
                                RemoteControlService::with_capabilities(
                                    secrets,
                                    initial,
                                    RemoteControlSelfAnnouncement::Unavailable,
                                    capabilities,
                                ),
                            )
                            .with_handlers(
                                host::InspectionHost(handle),
                                host::Messages {
                                    calls,
                                    gates,
                                    max_invocations,
                                },
                            ),
                            transport_identity: None,
                            pre_configured_destinations: []
                                as [personal_rns::runtime::PreConfiguredDestination<'static>; 0],
                            app_state: (),
                            storage: GrowableHeap,
                            request_endpoints: personal_rns::request_endpoints![
                                host::LargeResponse
                            ],
                            on_event: move |event, _: &()| {
                                pairing::observe(&pairing, index, generation, event)
                            },
                            interfaces: ManuallyAttached,
                            persistence,
                        }
                    },
                    TokioHost::with_runtime_entropy(timeline_origin, host_stream),
                    entropy,
                )
                .with_crypto_pool(CryptoPoolConfig::Inline)
                .with_interface_arbitration(InterfaceArbitration::RoundRobin {
                    first: InterfaceEventSource::Message,
                });
                let handle = node.handle();
                let _attached = handle.add_interface(interface);
                node.register_request_route::<host::LargeResponse>(&target_destination)
                    .expect("test Resource endpoint");
                assert!(ready.send(handle).is_ok(), "fixture ready receiver");
                Event::Stopped {
                    node: index,
                    result: node
                        .run_until(async {
                            let _ = stopping.await;
                        })
                        .await,
                }
            })
            .expect("bounded node actor");
        assert!(self.settle().is_empty());
        self.nodes.push(Node {
            handle: ready_rx.try_recv().expect("node initialized"),
            endpoint,
            identity,
            target,
            task,
            shutdown,
        });
    }
}

enum PersistenceSelection {
    Disabled,
    Durable(personal_rns::runtime::NodePersistence),
}

impl personal_rns::runtime::PersistenceIntent for PersistenceSelection {
    fn into_node_persistence(self) -> Option<personal_rns::runtime::NodePersistence> {
        match self {
            Self::Disabled => None,
            Self::Durable(persistence) => Some(persistence),
        }
    }
}

fn fill_seed(output: &mut [u8], seed: u8, generation: u64) {
    let boot = generation.to_be_bytes();
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = seed ^ boot[index % boot.len()];
    }
}

impl Lab<'_> {
    pub fn restart_target(&mut self, storage: persistence::Storage) {
        self.restart_node(TARGET, storage);
    }
    pub fn restart_node(&mut self, index: usize, storage: persistence::Storage) {
        self.restart_node_with_secrets(index, storage, secrets(index));
    }
    pub fn restart_node_with_secrets(
        &mut self,
        index: usize,
        storage: persistence::Storage,
        secrets: RemoteControlNodeIdentitySecrets,
    ) {
        self.runner
            .cancel(self.nodes[index].task)
            .expect("remove old boot");
        self.start_node_with_secrets(index, ControllerPolicy::Nobody, Some(storage), secrets);
        let last = self.nodes.len() - 1;
        self.nodes.swap(index, last);
        self.nodes.pop().expect("discard old handles");
        for controller in [CONTROLLER, OUTSIDER, OPERATOR] {
            self.set_reachability(controller, Reachability::Reachable);
        }
    }
}
