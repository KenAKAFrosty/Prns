#[cfg(target_arch = "xtensa")]
use core::array;

#[cfg(target_arch = "riscv32")]
use embassy_executor::Spawner;
use embassy_futures::join::join;
#[cfg(target_arch = "xtensa")]
use embassy_futures::join::join_array;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex as BridgeMutex;
use esp_radio::ble::controller::BleConnector;
use personal_rns::ble::{BluetoothAuto, BluetoothAutoShared};
use personal_rns::interfaces::bluetooth_auto::core::{
    encode_advertisement, BleIdentity, BleRoleCapabilities, Endpoint, Esp32Host, LinkCapabilities,
    Psm, BLE_HW_MTU, MAX_ADVERTISEMENT_LEN,
};
use personal_rns::reactor::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;
use personal_rns::runtime::Fleet;
#[cfg(target_arch = "riscv32")]
use prns_interfaces_embassy::ble_trouble::GattCharacteristic;
use prns_interfaces_embassy::ble_trouble::{
    self, acceptor, dialer, host_runner, serve_slot, BleHub, GattServer,
    ReticulumGattCharacteristics, ReticulumGattUuids, TroubleController, TroubleStack, CONNECTIONS,
    GATT_VALUE_CAP, L2CAP_CHANNELS, L2CAP_PSM, SLOTS,
};
use static_cell::StaticCell;
use trouble_host::prelude::*;

#[cfg(target_arch = "riscv32")]
use crate::c6::{BLE_CONTROLLER_CONNECTIONS, BLE_MEMBERS, LIFECYCLE_CAP, NOTIFY_CAP};
#[cfg(target_arch = "xtensa")]
use crate::s3::{BLE_MEMBERS, LIFECYCLE_CAP, NOTIFY_CAP};

type BleFleet = Fleet<BridgeMutex, EMBEDDED_MAX_WIRE_FRAME_LEN, NOTIFY_CAP, LIFECYCLE_CAP>;
type Transport = BleConnector<'static>;
type HostStack = TroubleStack<Transport>;

#[cfg(target_arch = "xtensa")]
const _: () = assert!(
    SLOTS == BLE_MEMBERS,
    "the S3 sizes its slot pool to its settled-member ceiling"
);
#[cfg(target_arch = "riscv32")]
const _: () = assert!(
    SLOTS == BLE_CONTROLLER_CONNECTIONS,
    "the C6 controller is configured with exactly the backend's slot count"
);
#[cfg(target_arch = "riscv32")]
const _: () = assert!(
    SLOTS == 8,
    "C6 serve_slot_task pool_size must equal ble_trouble::SLOTS"
);

#[cfg(target_arch = "riscv32")]
#[embassy_executor::task(pool_size = 8)]
async fn serve_slot_task(
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

#[cfg(target_arch = "riscv32")]
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
    fleet: BleFleet,
    shared: &'static BluetoothAutoShared<BLE_MEMBERS>,
    #[cfg(target_arch = "riscv32")] spawner: Spawner,
) {
    let controller = TroubleController::<Transport>::new(connector);
    static RESOURCES: StaticCell<HostResources<DefaultPacketPool, CONNECTIONS, L2CAP_CHANNELS>> =
        StaticCell::new();
    let resources = RESOURCES.init(HostResources::new());

    let mut address = mac;
    address[5] |= 0b1100_0000;
    let ble_identity = BleIdentity::from_radio_address(&address);
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

    static CONTROL_STORE: StaticCell<[u8; GATT_VALUE_CAP]> = StaticCell::new();
    static DATA_STORE: StaticCell<[u8; GATT_VALUE_CAP]> = StaticCell::new();
    static COLUMBA_RX_STORE: StaticCell<[u8; GATT_VALUE_CAP]> = StaticCell::new();
    static COLUMBA_TX_STORE: StaticCell<[u8; GATT_VALUE_CAP]> = StaticCell::new();
    static COLUMBA_IDENTITY_STORE: StaticCell<[u8; GATT_VALUE_CAP]> = StaticCell::new();
    let control_store = CONTROL_STORE.init([0; GATT_VALUE_CAP]);
    let data_store = DATA_STORE.init([0; GATT_VALUE_CAP]);
    let columba_rx_store = COLUMBA_RX_STORE.init([0; GATT_VALUE_CAP]);
    let columba_tx_store = COLUMBA_TX_STORE.init([0; GATT_VALUE_CAP]);
    let columba_identity_store = COLUMBA_IDENTITY_STORE.init([0; GATT_VALUE_CAP]);
    let Some((table, control, data, columba_rx, columba_tx)) =
        ble_trouble::reticulum_attribute_table(
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

    let service_uuid = ble_trouble::service_uuid();
    let control_uuid = ble_trouble::control_uuid();
    let data_uuid = ble_trouble::data_uuid();
    let columba_rx_uuid = ble_trouble::columba_rx_uuid();
    let columba_tx_uuid = ble_trouble::columba_tx_uuid();
    let columba_identity_uuid = ble_trouble::columba_identity_uuid();

    static HUB: StaticCell<BleHub> = StaticCell::new();
    let hub: &'static BleHub = HUB.init(BleHub::new());
    hub.prime(address);

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

    #[cfg(target_arch = "riscv32")]
    for idx in 0..SLOTS {
        spawner.spawn(
            serve_slot_task(
                idx,
                hub,
                stack,
                server,
                ReticulumGattOwned {
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
                },
            )
            .expect("ble slot task fits"),
        );
    }

    let host = host_runner(hub, runner);

    #[cfg(target_arch = "xtensa")]
    let workers = join_array::<_, SLOTS>(array::from_fn::<_, SLOTS, _>(|idx| {
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
    }));
    let radio = join(
        acceptor(hub, &mut peripheral, &adv_data[..adv_len]),
        dialer(hub, central),
    );
    #[cfg(target_arch = "riscv32")]
    let plane = join(radio, supervisor.run(fleet));
    #[cfg(target_arch = "xtensa")]
    let plane = join(radio, join(workers, supervisor.run(fleet)));
    join(host, plane).await;
}
