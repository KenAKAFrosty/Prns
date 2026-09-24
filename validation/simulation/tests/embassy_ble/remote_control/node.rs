use super::adapter::{Handle, Runtime};
use super::*;
use crate::clock::EmbassyTasks;
use crate::fixture::{RadioFixture, RawMutex, MAX_PEERS};
use crate::node_events::EventLog;
use crate::wire_gate::{GatedBackend, WireGate};
use personal_rns::engine::{InstantMillis, IssuedCommand};
use personal_rns::interfaces::bluetooth_auto::{
    BleIdentity, Endpoint, Esp32Host, LinkCapabilities, BLE_HW_MTU,
};
use personal_rns::interfaces::*;
use personal_rns::runtime::request_endpoints::RequestEndpointSet;
use personal_rns::runtime::*;
use personal_rns::storage::{GrowableHeap, StorageLayout};
use prns_core::entropy::{EntropySource, RuntimeEntropy};
use prns_runtime_embassy::runtime::{
    CompletionPool, EmbassyInterfaceStore, EmbeddedCompactionPolicy, EmbeddedFlashPersistence,
    EmbeddedPersistencePolicy, FixedRouteSnapshotKeys, RequestRoutingCapacity,
    SharedRuntimeEntropy,
};
use prns_simulation::{ble::VirtualBleLab, ManualTaskId};
use std::convert::Infallible;

#[path = "../../remote_control/fixture/persistence/mod.rs"]
#[allow(dead_code)]
pub mod file;

#[derive(Clone)]
pub enum Storage {
    Tokio(file::Storage),
    Embassy(super::persistence::Image),
}
impl Storage {
    pub fn new(runtime: Runtime) -> Self {
        match runtime {
            Runtime::Tokio => Self::Tokio(file::Storage::new()),
            Runtime::Embassy => Self::Embassy(super::persistence::Image::new()),
        }
    }
    pub fn trace(&self) -> serde_json::Value {
        match self {
            Self::Tokio(storage) => {
                serde_json::json!({ "owner": "file_store", "events": storage.io.events().into_iter().map(|event| format!("{event:?}")).collect::<Vec<_>>() })
            }
            Self::Embassy(image) => {
                serde_json::json!({ "owner": "flash_journal", "events": image.trace() })
            }
        }
    }
}
pub type EmbeddedHandle =
    prns_runtime_embassy::runtime::PrnsNodeHandle<'static, RawMutex, 8, 8, 4, 512>;
type InspectionStore = EmbassyInterfaceStore<RawMutex, 4, 4, 8>;

pub struct Node {
    pub handle: Handle,
    pub task: ManualTaskId,
    pub wire: WireGate,
    pub inspection: Inspection,
    pub events: EventLog,
}
struct Entropy(u8);
impl EntropySource for Entropy {
    type Error = Infallible;
    fn try_fill_entropy(&mut self, bytes: &mut [u8]) -> Result<(), Infallible> {
        bytes.fill(self.0);
        self.0 = self.0.wrapping_add(1);
        Ok(())
    }
}

pub fn start(
    tasks: &mut EmbassyTasks<'_>,
    lab: &VirtualBleLab,
    index: usize,
    generation: u64,
    storage: Storage,
    messages: Messages,
) -> Node {
    start_with_crypto(
        tasks,
        lab,
        index,
        generation,
        storage,
        messages,
        CryptoPoolConfig::Inline,
    )
}

pub fn start_with_crypto(
    tasks: &mut EmbassyTasks<'_>,
    lab: &VirtualBleLab,
    index: usize,
    generation: u64,
    storage: Storage,
    messages: Messages,
    crypto: CryptoPoolConfig,
) -> Node {
    start_with_settings(
        tasks,
        lab,
        index,
        generation,
        storage,
        messages,
        NodeSettings::<_, _, 0, 2> {
            crypto,
            storage: GrowableHeap,
            destinations: [] as [PreConfiguredDestination<'static>; 0],
            endpoints: personal_rns::request_endpoints![],
        },
    )
}

pub struct NodeSettings<S, R, const DESTINATIONS: usize, const DEPTH: usize> {
    pub crypto: CryptoPoolConfig,
    pub storage: S,
    pub destinations: [PreConfiguredDestination<'static>; DESTINATIONS],
    pub endpoints: R,
}

pub fn start_with_settings<
    S: StorageLayout + 'static,
    R: RequestEndpointSet<()> + 'static,
    const DESTINATIONS: usize,
    const DEPTH: usize,
>(
    tasks: &mut EmbassyTasks<'_>,
    lab: &VirtualBleLab,
    index: usize,
    generation: u64,
    storage: Storage,
    messages: Messages,
    settings: NodeSettings<S, R, DESTINATIONS, DEPTH>,
) -> Node {
    let NodeSettings {
        crypto,
        storage: engine_storage,
        destinations,
        endpoints,
    } = settings;
    let events = EventLog::new();
    let observed_events = events.clone();
    let address = (index + 1) as u8;
    let logical_start = InstantMillis(1_000_000 + tasks.snapshot().tick.get());
    let observations = messages.1.clone();
    match storage {
        Storage::Tokio(storage) => {
            if let CryptoPoolConfig::Controlled(control) = &crypto {
                tasks.register_crypto(control.clone());
            }
            let wire = WireGate::new();
            let supervisor =
                prns_interfaces_tokio::bluetooth_auto::BluetoothAuto::<_, MAX_PEERS>::new(
                    GatedBackend::new(crate::fixture::backend(lab, address), wire.clone()),
                    BleIdentity::new([address; 16]),
                    Endpoint::Esp32(Esp32Host::Esp32),
                    LinkCapabilities {
                        l2cap: None,
                        link_mtu: BLE_HW_MTU as u16,
                    },
                )
                .with_event_selector(crate::selection::RoundRobinBleEvents::new(
                    crate::selection::FirstEvent::Backend,
                ));
            let (ready, mut received) = tokio::sync::oneshot::channel();
            let seed = address.wrapping_add(0x80).wrapping_add(generation as u8);
            let task = tasks.insert(async move {
                let host = RuntimeEntropy::try_new(move |bytes: &mut [u8]| {
                    bytes.fill(seed);
                    Ok::<(), Infallible>(())
                })
                .expect("fixture host entropy");
                let stream = RuntimeEntropy::try_new(move |bytes: &mut [u8]| {
                    bytes.fill(seed.wrapping_add(8));
                    Ok::<(), Infallible>(())
                })
                .expect("fixture handle entropy");
                let entropy =
                    TokioHandleEntropy::from_sources(stream, move |bytes: &mut [u8]| {
                        bytes.fill(seed.wrapping_add(16));
                        Ok::<(), Infallible>(())
                    });
                let node = PrnsNode::new_with_entropy_sources(
                    |handle| PrnsNodeRecipe {
                        remote_control: RemoteControlNodeSetup::new(service(
                            index,
                            RemoteControlInitialControllerGrants::Nobody,
                        ))
                        .with_handlers(Inspection::Tokio(handle), messages),
                        transport_identity: None,
                        pre_configured_destinations: destinations,
                        app_state: (),
                        storage: engine_storage,
                        request_endpoints: endpoints,
                        on_event: move |event, _: &()| {
                            observed_events.observe(&event);
                            super::pairing::observe(&observations, index, event)
                        },
                        interfaces: move |handle: &PrnsNodeHandle| {
                            handle.supervise(supervisor);
                        },
                        persistence: NodePersistence::custom_dir(storage.directory())
                            .expect("scenario storage")
                            .with_io_driver(storage.io),
                    },
                    personal_rns::manifold::tokio::TokioHost::with_runtime_entropy(
                        logical_start,
                        host,
                    ),
                    entropy,
                )
                .with_crypto_pool(crypto)
                .with_interface_arbitration(InterfaceArbitration::RoundRobin {
                    first: InterfaceEventSource::Message,
                });
                assert!(ready.send(node.handle()).is_ok());
                let result = node.run().await;
                unreachable!("live Tokio RC fixture: {result:?}");
            });
            tasks.settle();
            let handle = received.try_recv().expect("ready handle");
            Node {
                handle: Handle::Tokio(handle.clone()),
                task,
                wire,
                inspection: Inspection::Tokio(handle),
                events,
            }
        }
        Storage::Embassy(image) => {
            let RadioFixture {
                supervisor,
                fleet,
                lanes,
                notify,
                lifecycle,
                wire,
            } = RadioFixture::<DEPTH>::new(lab, address, Endpoint::Esp32(Esp32Host::Esp32));
            let status = supervisor.status();
            let commands = crate::static_storage::allocate(embassy_sync::channel::Channel::<
                RawMutex,
                IssuedCommand,
                8,
            >::new());
            let completions =
                crate::static_storage::allocate(CompletionPool::<RawMutex, 8, 4, 512>::new());
            let handle = EmbeddedHandle::new(commands.sender(), completions);
            let wiring = lanes.into_manifold_wiring(
                notify.receiver(),
                commands.receiver(),
                lifecycle.receiver(),
                handle,
            );
            let entropy = crate::static_storage::allocate(
                SharedRuntimeEntropy::<RawMutex, _>::try_new(Entropy(
                    address.wrapping_add(generation as u8),
                ))
                .expect("fixed fixture entropy"),
            );
            let store: &'static InspectionStore =
                crate::static_storage::allocate(InspectionStore::new());
            let groups = crate::static_storage::allocate(
                prns_runtime_embassy::runtime::DiscoveryGroupConfigurationStoreExchange::new(),
            );
            let recipe = PrnsNodeRecipe {
                remote_control: RemoteControlNodeSetup::new(service(
                    index,
                    RemoteControlInitialControllerGrants::Nobody,
                ))
                .with_handlers(Inspection::Embassy { status, store }, messages),
                transport_identity: None,
                pre_configured_destinations: destinations,
                app_state: (),
                storage: engine_storage,
                request_endpoints: endpoints,
                on_event: move |event, _: &()| {
                    observed_events.observe(&event);
                    super::pairing::observe(&observations, index, event)
                },
                interfaces: ManuallyAttached,
                persistence: NoPersistence,
            };
            let mut node: prns_runtime_embassy::runtime::PrnsNode<
                _,
                _,
                _,
                _,
                _,
                _,
                1,
                MAX_PEERS,
                DEPTH,
                8,
                4,
                8,
                8,
                256,
                4,
                512,
                _,
            > = prns_runtime_embassy::runtime::PrnsNode::new_with_request_capacity(
                recipe,
                wiring,
                prns_runtime_embassy::manifold::driver::EmbassyHost::new_with_timebase(
                    prns_runtime_embassy::manifold::timebase::EmbassyTimebase::start_at(
                        logical_start,
                    ),
                    entropy.handle(),
                ),
                RequestRoutingCapacity::new(),
            );
            let mut owner = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<16>, _, 8, _>::with_discovery_group_store(image, super::persistence::LAYOUT, EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)), FixedRouteSnapshotKeys::new(), |_| {}, &*groups);
            let task = tasks.insert(async move {
                node.restore_embedded_persistence(&mut owner).await;
                tokio::join!(
                    node.run_manifold_with_persistence_and_interface_store(&*store, &mut owner),
                    supervisor.run(fleet)
                );
            });
            tasks.settle();
            Node {
                handle: Handle::Embassy(handle),
                task,
                wire,
                inspection: Inspection::Embassy { status, store },
                events,
            }
        }
    }
}

#[derive(Clone)]
pub(crate) enum Inspection {
    Tokio(PrnsNodeHandle),
    Embassy {
        status: prns_interfaces_embassy::bluetooth_auto::BluetoothAutoStatus<MAX_PEERS>,
        store: &'static InspectionStore,
    },
}
impl Inspection {
    pub(crate) fn snapshots(&self) -> Vec<InterfaceSnapshot> {
        match self {
            Self::Tokio(handle) => handle.interfaces(),
            Self::Embassy { status, store } => {
                let make = |status: &dyn InterfaceStatus, membership| {
                    let counts = store.counts(status.id());
                    let descriptor = personal_rns::interfaces::bluetooth_auto::descriptor(
                        status.id(),
                        personal_rns::interfaces::bluetooth_auto::BLE_BITRATE_GUESS_BPS,
                    );
                    InterfaceSnapshot {
                        id: status.id(),
                        mode: descriptor.mode,
                        gravity: descriptor.gravity,
                        connection: status.connection(),
                        failure_reason: status.failure_reason(),
                        rx_bytes: status.rx_bytes(),
                        tx_bytes: status.tx_bytes(),
                        transfer_rates: status.transfer_rates(),
                        destinations: counts.destinations,
                        links: counts.links,
                        transported_links: counts.transported_links,
                        membership,
                        radio: status.radio(),
                        details: status.details(),
                    }
                };
                let mut snapshots = vec![make(status, Membership::Independent)];
                snapshots.extend(status.members().map(|peer| {
                    make(
                        peer,
                        Membership::FleetMember {
                            supervisor_id: status.id(),
                        },
                    )
                }));
                snapshots
            }
        }
    }
}
impl RemoteControlHostControls for Inspection {
    fn supported_requests(&self) -> RemoteControlRequestSet {
        let mut requests = RemoteControlRequestSet::only(RemoteControlRequestKind::DescribeBuild);
        for request in [
            RemoteControlRequestKind::InventoryInterfaces,
            RemoteControlRequestKind::InventoryInterfaceConfig,
            RemoteControlRequestKind::InventoryInterfacePeers,
        ] {
            requests.insert(request);
        }
        requests
    }
    async fn execute_remote_control(
        &self,
        command: RemoteControlHostCommand,
    ) -> Result<RemoteControlHostResponse, RemoteControlHostCommandError> {
        use RemoteControlHostCommand as C;
        use RemoteControlHostResponse as R;
        match command {
            C::DescribeBuild => Ok(R::DescribeBuild(
                RemoteControlBuildVersion::from_text("simulation-v1").expect("bounded build"),
            )),
            C::InventoryInterfaces { page } => {
                personal_hopspot_core::remote_control_inventory_from_snapshots(
                    &self.snapshots(),
                    page,
                )
                .map(R::InventoryInterfaces)
                .map_err(|_| RemoteControlHostCommandError::ApplyFailed)
            }
            C::InventoryInterfaceConfig { id } => {
                personal_hopspot_core::remote_control_interface_config_from_snapshots(
                    &self.snapshots(),
                    id,
                    |_, _| Ok(()),
                )
                .map(R::InventoryInterfaceConfig)
                .map_err(|_| RemoteControlHostCommandError::ApplyFailed)
            }
            C::InventoryInterfacePeers { id, page } => {
                personal_hopspot_core::remote_control_interface_peers_from_snapshots(
                    &self.snapshots(),
                    id,
                    page,
                )
                .map(R::InventoryInterfacePeers)
                .map_err(|_| RemoteControlHostCommandError::ApplyFailed)
            }
            _ => Err(RemoteControlHostCommandError::Unsupported),
        }
    }
}
