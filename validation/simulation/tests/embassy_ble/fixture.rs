use std::num::NonZeroUsize;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use personal_rns::interfaces::bluetooth_auto::{
    BleAddress, BleIdentity, BleRoleCapabilities, Endpoint, LinkCapabilities, BLE_HW_MTU,
    CONTROL_MAX_LEN,
};
use personal_rns::interfaces::{InterfaceId, InterfaceKind};
use prns_interfaces_embassy::bluetooth_auto::{BluetoothAuto, BluetoothAutoShared};
use prns_runtime_embassy::manifold::driver::InterfaceLifecycle;
use prns_runtime_embassy::runtime::{EmbassyFleet, ManifoldLaneSet, StaticManifoldLane};
use prns_simulation::ble::{
    BleMediumConfig, VirtualBleBackend, VirtualBleBackendConfig, VirtualBleBackendLimits,
    VirtualBleLab, VirtualBleLinkConfig, VirtualGattConfig,
};
use prns_simulation::{SimulationDurationInTicks, TopologyConfig};

use super::wire_gate::{GatedBackend, WireGate};

pub(super) const MAX_PEERS: usize = 2;
pub(super) const ADVERTISING_INTERVAL_MS: u64 = 60_000;
pub(super) const GATT_VALUE_BYTES: usize = 20;
pub(super) const GATT_QUEUE_DEPTH: usize = 4;
pub(super) type RawMutex = CriticalSectionRawMutex;
pub(super) type Lifecycle = Channel<RawMutex, InterfaceLifecycle<'static>, 4>;
pub(super) type Fleet<const DEPTH: usize = 2> = EmbassyFleet<RawMutex, BLE_HW_MTU, DEPTH, 4>;
pub(super) type Supervisor = BluetoothAuto<GatedBackend, MAX_PEERS>;

pub(super) fn lab() -> VirtualBleLab {
    VirtualBleLab::new(BleMediumConfig::new(TopologyConfig::FullyConnected, 2, 4, 4, 64).unwrap())
}

pub(super) fn backend(lab: &VirtualBleLab, address: u8) -> VirtualBleBackend {
    let gatt = VirtualGattConfig::new(CONTROL_MAX_LEN, GATT_VALUE_BYTES).unwrap();
    let link = VirtualBleLinkConfig::new(4, GATT_QUEUE_DEPTH, BLE_HW_MTU, gatt).unwrap();
    lab.attach_backend(
        VirtualBleBackendConfig::new(
            BleAddress::new([address; 6]),
            -40,
            BleRoleCapabilities::DualRole,
            SimulationDurationInTicks::from_ticks(ADVERTISING_INTERVAL_MS),
            VirtualBleBackendLimits {
                inbound_links: NonZeroUsize::new(MAX_PEERS).unwrap(),
                connections: NonZeroUsize::new(MAX_PEERS).unwrap(),
                discovered_peers: NonZeroUsize::new(MAX_PEERS).unwrap(),
            },
            link,
        )
        .unwrap(),
    )
    .unwrap()
}

pub(super) fn supervisor(
    lab: &VirtualBleLab,
    address: u8,
    endpoint: Endpoint,
) -> (Supervisor, Fleet, &'static Lifecycle) {
    let RadioFixture {
        supervisor,
        fleet,
        lifecycle,
        ..
    } = RadioFixture::new(lab, address, endpoint);
    (supervisor, fleet, lifecycle)
}

pub(super) struct RadioFixture<const DEPTH: usize = 2> {
    pub supervisor: Supervisor,
    pub fleet: Fleet<DEPTH>,
    pub lanes: ManifoldLaneSet<RawMutex, 1, DEPTH>,
    pub notify: &'static Channel<RawMutex, InterfaceId, DEPTH>,
    pub lifecycle: &'static Lifecycle,
    pub wire: WireGate,
}

impl<const DEPTH: usize> RadioFixture<DEPTH> {
    pub fn new(lab: &VirtualBleLab, address: u8, endpoint: Endpoint) -> Self {
        let id = InterfaceId::from_channel_tag(InterfaceKind::BluetoothAuto, &[address]);
        let shared = super::static_storage::allocate(BluetoothAutoShared::new(id));
        let wire = WireGate::new();
        let supervisor = BluetoothAuto::new(
            GatedBackend::new(backend(lab, address), wire.clone()),
            BleIdentity::new([address; 16]),
            endpoint,
            LinkCapabilities {
                l2cap: None,
                link_mtu: BLE_HW_MTU as u16,
            },
            shared,
        );
        let lane = super::static_storage::allocate(
            StaticManifoldLane::<RawMutex, BLE_HW_MTU, DEPTH>::new(),
        );
        let wake = super::static_storage::allocate(Signal::new());
        let notify = super::static_storage::allocate(Channel::new());
        let lifecycle = super::static_storage::allocate(Lifecycle::new());
        let mut lanes = ManifoldLaneSet::<RawMutex, 1, DEPTH>::new();
        let fleet = lanes
            .claim_supervisor(lane, id, wake)
            .unwrap()
            .into_fleet(notify.sender(), lifecycle.sender());
        Self {
            supervisor,
            fleet,
            lanes,
            notify,
            lifecycle,
            wire,
        }
    }
}
