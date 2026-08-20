use alloc::boxed::Box;

use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex as BridgeMutex;
use esp_radio::ble::controller::BleConnector;
use personal_rns::bluetooth_auto::{BluetoothAuto, BluetoothAutoShared, BluetoothAutoStatus};
use personal_rns::interfaces::bluetooth_auto::{
    encode_advertisement, BleIdentity, BleRoleCapabilities, Endpoint, Esp32Host, LinkCapabilities,
    Psm, BLE_HW_MTU, MAX_ADVERTISEMENT_LEN,
};
use personal_rns::runtime::Fleet;
use prns_interfaces_embassy::bluetooth_auto::GattCharacteristic;
use prns_interfaces_embassy::bluetooth_auto::{
    self, acceptor, dialer, host_runner, serve_slot, BleHub, CooperativeTransport, GattServer,
    ReticulumGattCharacteristics, ReticulumGattUuids, TroubleController, TroubleStack,
    GATT_VALUE_CAP, L2CAP_PSM, PEER_CAPACITY,
};
use static_cell::StaticCell;
use trouble_host::prelude::*;

#[cfg(target_arch = "riscv32")]
use crate::c6::{BLE_PEER_CAPACITY, LIFECYCLE_CAP, NOTIFY_CAP};
#[cfg(target_arch = "xtensa")]
use crate::s3::{BLE_PEER_CAPACITY, LIFECYCLE_CAP, NOTIFY_CAP};

type BleFleet = Fleet<BridgeMutex, BLE_HW_MTU, NOTIFY_CAP, LIFECYCLE_CAP>;
type Transport = CooperativeTransport<BleConnector<'static>>;
type HostStack = TroubleStack<Transport>;

#[cfg(target_arch = "xtensa")]
const _: () = assert!(
    PEER_CAPACITY == BLE_PEER_CAPACITY,
    "the S3 controller, slot pool, and supervisor must share one peer capacity"
);
#[cfg(target_arch = "riscv32")]
const _: () = assert!(
    PEER_CAPACITY == BLE_PEER_CAPACITY,
    "the C6 controller, slot pool, and supervisor must share one peer capacity"
);
#[cfg(target_arch = "riscv32")]
const _: () = assert!(
    PEER_CAPACITY == 8,
    "C6 serve_slot_task pool_size must equal bluetooth_auto::PEER_CAPACITY"
);
#[cfg(target_arch = "xtensa")]
const _: () = assert!(
    PEER_CAPACITY == 4,
    "S3 serve_slot_task pool_size must equal bluetooth_auto::PEER_CAPACITY"
);

async fn serve_owned_slot(
    idx: usize,
    hub: &'static BleHub,
    stack: &'static HostStack,
    server: &'static GattServer,
    gatt: ReticulumGattOwned,
) {
    let ReticulumGattOwned {
        control,
        data,
        columba_rx,
        columba_tx,
        service_uuid,
        control_uuid,
        data_uuid,
        columba_rx_uuid,
        columba_tx_uuid,
        columba_identity_uuid,
    } = gatt;
    serve_slot(
        idx,
        hub,
        stack,
        server,
        ReticulumGattCharacteristics {
            control: &control,
            data: &data,
            columba_rx: &columba_rx,
            columba_tx: &columba_tx,
        },
        ReticulumGattUuids {
            service: &service_uuid,
            control: &control_uuid,
            data: &data_uuid,
            columba_rx: &columba_rx_uuid,
            columba_tx: &columba_tx_uuid,
            columba_identity: &columba_identity_uuid,
        },
    )
    .await
}

#[cfg(target_arch = "xtensa")]
#[embassy_executor::task(pool_size = 4)]
async fn serve_slot_task(run: core::pin::Pin<&'static mut dyn core::future::Future<Output = ()>>) {
    run.await
}

#[cfg(target_arch = "riscv32")]
#[embassy_executor::task(pool_size = 8)]
async fn serve_slot_task(
    idx: usize,
    hub: &'static BleHub,
    stack: &'static HostStack,
    server: &'static GattServer,
    gatt: ReticulumGattOwned,
) {
    serve_owned_slot(idx, hub, stack, server, gatt).await
}

struct ReticulumGattOwned {
    control: GattCharacteristic,
    data: GattCharacteristic,
    columba_rx: GattCharacteristic,
    columba_tx: GattCharacteristic,
    service_uuid: Uuid,
    control_uuid: Uuid,
    data_uuid: Uuid,
    columba_rx_uuid: Uuid,
    columba_tx_uuid: Uuid,
    columba_identity_uuid: Uuid,
}

pub async fn run(
    connector: BleConnector<'static>,
    mac: [u8; 6],
    ble_identity: BleIdentity,
    fleet: BleFleet,
    shared: &'static BluetoothAutoShared<BLE_PEER_CAPACITY>,
    spawner: Spawner,
) {
    let controller = TroubleController::<Transport>::new(CooperativeTransport::new(connector));
    static RESOURCES: StaticCell<HostResources<DefaultPacketPool, PEER_CAPACITY, PEER_CAPACITY>> =
        StaticCell::new();
    let resources = RESOURCES.init(HostResources::new());

    let mut address = mac;
    address[5] |= 0b1100_0000;
    // The host stack is parked in a `static` so its `Connection`s are `'static` and can ride the hub's
    // assign channels from the acceptor/dialer to a slot worker (trouble-host's own objects are
    // otherwise lifetime-bound to the stack).
    static STACK: StaticCell<HostStack> = StaticCell::new();
    let stack: &'static HostStack = STACK.init(
        trouble_host::new(controller, resources).set_random_address(Address::random(address)),
    );
    let Host {
        mut peripheral,
        central,
        runner,
        ..
    } = stack.build();

    let control_store = Box::leak(Box::new([0; GATT_VALUE_CAP]));
    let data_store = Box::leak(Box::new([0; GATT_VALUE_CAP]));
    let columba_rx_store = Box::leak(Box::new([0; GATT_VALUE_CAP]));
    let columba_tx_store = Box::leak(Box::new([0; GATT_VALUE_CAP]));
    let columba_identity_store = Box::leak(Box::new([0; GATT_VALUE_CAP]));
    let Some((table, control, data, columba_rx, columba_tx)) =
        bluetooth_auto::reticulum_attribute_table(
            control_store,
            data_store,
            columba_rx_store,
            columba_tx_store,
            columba_identity_store,
            ble_identity,
        )
    else {
        return;
    };
    static SERVER: StaticCell<GattServer> = StaticCell::new();
    let server: &'static GattServer = SERVER.init(AttributeServer::new(table));

    let mut adv_data = [0u8; MAX_ADVERTISEMENT_LEN];
    let adv_len = encode_advertisement(&mut adv_data, BleRoleCapabilities::DualRole)
        .expect("advertisement fits");

    let service_uuid = bluetooth_auto::service_uuid();
    let control_uuid = bluetooth_auto::control_uuid();
    let data_uuid = bluetooth_auto::data_uuid();
    let columba_rx_uuid = bluetooth_auto::columba_rx_uuid();
    let columba_tx_uuid = bluetooth_auto::columba_tx_uuid();
    let columba_identity_uuid = bluetooth_auto::columba_identity_uuid();

    static HUB: StaticCell<BleHub> = StaticCell::new();
    let hub: &'static BleHub = HUB.init(BleHub::new(BluetoothAutoStatus::new(shared)));
    hub.set_local_address(address);

    let supervisor = BluetoothAuto::new(
        hub.backend(),
        ble_identity,
        Endpoint::Esp32(Esp32Host::Esp32),
        LinkCapabilities {
            l2cap: Psm::new(L2CAP_PSM),
            link_mtu: BLE_HW_MTU as u16,
        },
        shared,
    );

    for idx in 0..PEER_CAPACITY {
        let gatt = ReticulumGattOwned {
            control: control.clone(),
            data: data.clone(),
            columba_rx: columba_rx.clone(),
            columba_tx: columba_tx.clone(),
            service_uuid: service_uuid.clone(),
            control_uuid: control_uuid.clone(),
            data_uuid: data_uuid.clone(),
            columba_rx_uuid: columba_rx_uuid.clone(),
            columba_tx_uuid: columba_tx_uuid.clone(),
            columba_identity_uuid: columba_identity_uuid.clone(),
        };
        #[cfg(target_arch = "xtensa")]
        {
            let run =
                crate::storage::allocate_psram(serve_owned_slot(idx, hub, stack, server, gatt));
            let run: core::pin::Pin<&'static mut dyn core::future::Future<Output = ()>> =
                // SAFETY: `allocate_psram` leaks this allocation, so it cannot move or be freed.
                unsafe { core::pin::Pin::new_unchecked(run) };
            spawner.spawn(serve_slot_task(run).expect("ble slot task fits"));
        }
        #[cfg(target_arch = "riscv32")]
        spawner.spawn(serve_slot_task(idx, hub, stack, server, gatt).expect("ble slot task fits"));
    }

    let host = host_runner(hub, runner);
    let radio = join(
        acceptor(hub, &mut peripheral, &adv_data[..adv_len]),
        dialer(hub, central),
    );
    let plane = join(radio, supervisor.run(fleet));
    join(host, plane).await;
}
