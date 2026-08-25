use embassy_executor::Spawner;
use embassy_futures::join::join3;
use embassy_nrf::usb::vbus_detect::SoftwareVbusDetect;
use nrf_softdevice::ble::l2cap;
use nrf_softdevice::Softdevice;
use personal_rns::bluetooth_auto::BluetoothAuto;
use personal_rns::interfaces::bluetooth_auto::{
    BleIdentity, Endpoint, LinkCapabilities, Nrf52Host,
};
use personal_rns::runtime::{Fleet, SupervisorLane};
use static_cell::StaticCell;

use super::super::bluetooth_auto::{
    acceptor, scanner, serve_slot, set_columba_identity, softdevice_config, softdevice_task,
    L2capPacket, NrfBleBackend, Server, HUB, POOL,
};
use super::{Mtx, LIFECYCLE, LIFECYCLE_CAP, NOTIFY, NOTIFY_CAP};

pub(super) use super::super::bluetooth_auto::{BLE_SHARED, OUTBOUND_WAKE};
pub(super) use super::super::bluetooth_auto::{BLE_SUPERVISOR_ID, MEMBERS};
pub(super) use personal_rns::interfaces::bluetooth_auto::BLE_HW_MTU;

type Supervisor = BluetoothAuto<NrfBleBackend, MEMBERS>;
type BluetoothFleet = Fleet<Mtx, BLE_HW_MTU, NOTIFY_CAP, LIFECYCLE_CAP>;
pub(super) type Runtime = Option<(Supervisor, BluetoothFleet)>;

pub(super) fn enable(
    spawner: Spawner,
    vbus: &'static SoftwareVbusDetect,
    identity: Option<BleIdentity>,
) -> &'static Softdevice {
    let sd = Softdevice::enable(&softdevice_config());
    static SERVER: StaticCell<Server> = StaticCell::new();
    let server: &'static Server = SERVER.init(Server::new(sd).unwrap());
    static L2CAP: StaticCell<l2cap::L2cap<L2capPacket>> = StaticCell::new();
    let l2cap: &'static l2cap::L2cap<L2capPacket> = L2CAP.init(l2cap::L2cap::init(sd));
    let sd: &'static Softdevice = sd;
    spawner.spawn(softdevice_task(sd, vbus).expect("softdevice task fits"));
    if let Some(identity) = identity {
        set_columba_identity(sd, server, identity);
        for idx in 0..POOL {
            spawner.spawn(serve_slot(idx, sd, l2cap, server, &HUB).expect("serve slot fits"));
        }
    }
    sd
}

pub(super) fn prepare(
    identity: Option<BleIdentity>,
    lane: Option<SupervisorLane<Mtx, BLE_HW_MTU>>,
) -> Runtime {
    let backend = NrfBleBackend::new(&HUB);
    identity.zip(lane).map(|(identity, lane)| {
        let supervisor = BluetoothAuto::new(
            backend,
            identity,
            Endpoint::Nrf52(Nrf52Host::Nrf52),
            LinkCapabilities {
                l2cap: None,
                link_mtu: BLE_HW_MTU as u16,
            },
            &BLE_SHARED,
        );
        let fleet = lane.into_fleet(NOTIFY.sender(), LIFECYCLE.sender());
        (supervisor, fleet)
    })
}

pub(super) fn run(sd: &'static Softdevice, runtime: Runtime) -> impl core::future::Future {
    async move {
        match runtime {
            Some((supervisor, fleet)) => {
                join3(acceptor(sd, &HUB), scanner(sd, &HUB), supervisor.run(fleet)).await;
            }
            None => core::future::pending().await,
        }
    }
}

#[cfg(any(feature = "board-t096", feature = "board-t114"))]
pub(super) fn usb_vbus_present() -> bool {
    super::super::bluetooth_auto::usb_vbus_present()
}

#[cfg(any(feature = "board-t096", feature = "board-t114"))]
pub(super) use personal_rns::bluetooth_auto::BluetoothAutoStatus;
