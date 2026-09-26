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

pub(super) const MAX_PEERS: usize = 2;
pub(super) const ADVERTISING_INTERVAL_MS: u64 = 60_000;
pub(super) const GATT_VALUE_BYTES: usize = 20;
pub(super) const GATT_QUEUE_DEPTH: usize = 4;
pub(super) type RawMutex = CriticalSectionRawMutex;
pub(super) type Lifecycle = Channel<RawMutex, InterfaceLifecycle, 4>;
pub(super) type Fleet = EmbassyFleet<RawMutex, BLE_HW_MTU, 2, 4>;
pub(super) type Supervisor = BluetoothAuto<VirtualBleBackend, MAX_PEERS>;

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

pub(super) struct RadioFixture {
    pub supervisor: Supervisor,
    pub fleet: Fleet,
    pub lanes: ManifoldLaneSet<RawMutex, 1, 2>,
    pub notify: &'static Channel<RawMutex, InterfaceId, 2>,
    pub lifecycle: &'static Lifecycle,
}

impl RadioFixture {
    pub fn new(lab: &VirtualBleLab, address: u8, endpoint: Endpoint) -> Self {
        let id = InterfaceId::from_channel_tag(InterfaceKind::BluetoothAuto, &[address]);
        let shared = Box::leak(Box::new(BluetoothAutoShared::new(id)));
        let supervisor = BluetoothAuto::new(
            backend(lab, address),
            BleIdentity::new([address; 16]),
            endpoint,
            LinkCapabilities {
                l2cap: None,
                link_mtu: BLE_HW_MTU as u16,
            },
            shared,
        );
        let lane = Box::leak(Box::new(
            StaticManifoldLane::<RawMutex, BLE_HW_MTU, 2>::new(),
        ));
        let wake = Box::leak(Box::new(Signal::new()));
        let notify = Box::leak(Box::new(Channel::new()));
        let lifecycle = Box::leak(Box::new(Lifecycle::new()));
        let mut lanes = ManifoldLaneSet::<RawMutex, 1, 2>::new();
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
        }
    }
}
