#[cfg(target_arch = "xtensa")]
use core::array;

use alloc::boxed::Box;

#[cfg(target_arch = "riscv32")]
use embassy_executor::Spawner;
use embassy_futures::join::join;
#[cfg(target_arch = "xtensa")]
use embassy_futures::join::join_array;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex as BridgeMutex;
use esp_hal::rng::Rng;
use esp_hal::rom::spiflash::{
    esp_rom_spiflash_read, esp_rom_spiflash_write, ESP_ROM_SPIFLASH_RESULT_OK,
};
use esp_radio::ble::controller::BleConnector;
use personal_rns::bluetooth_auto::{BluetoothAuto, BluetoothAutoShared};
use personal_rns::interfaces::bluetooth_auto::{
    decode_persisted_ble_identity, encode_advertisement, encode_persisted_ble_identity,
    BleIdentity, BleRoleCapabilities, Endpoint, Esp32Host, LinkCapabilities,
    PersistedBleIdentityError, Psm, BLE_HW_MTU, MAX_ADVERTISEMENT_LEN, PERSISTED_BLE_IDENTITY_LEN,
};
use personal_rns::reactor::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;
use personal_rns::runtime::Fleet;
#[cfg(target_arch = "riscv32")]
use prns_interfaces_embassy::bluetooth_auto::GattCharacteristic;
use prns_interfaces_embassy::bluetooth_auto::{
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

const BLE_IDENTITY_FLASH_OFFSET: u32 = 0xd000;

#[repr(align(4))]
struct AlignedIdentityRecord([u8; PERSISTED_BLE_IDENTITY_LEN]);

pub fn load_or_create_ble_identity() -> Result<BleIdentity, EspBleIdentityError> {
    if let Some(identity) = read_ble_identity()? {
        return Ok(identity);
    }
    let mut bytes = [0u8; 16];
    Rng::new().read(&mut bytes);
    let identity = BleIdentity::new(bytes);
    let record = AlignedIdentityRecord(encode_persisted_ble_identity(identity));
    write_flash(BLE_IDENTITY_FLASH_OFFSET + 8, &record.0[8..])?;
    write_flash(BLE_IDENTITY_FLASH_OFFSET, &record.0[..8])?;
    match read_ble_identity()? {
        Some(persisted) if persisted == identity => Ok(identity),
        Some(_) | None => Err(EspBleIdentityError::Verification),
    }
}

fn read_ble_identity() -> Result<Option<BleIdentity>, EspBleIdentityError> {
    let mut record = AlignedIdentityRecord([0u8; PERSISTED_BLE_IDENTITY_LEN]);
    let result = read_flash(BLE_IDENTITY_FLASH_OFFSET, &mut record.0);
    if result != ESP_ROM_SPIFLASH_RESULT_OK {
        return Err(EspBleIdentityError::Read(result));
    }
    decode_persisted_ble_identity(&record.0).map_err(EspBleIdentityError::Stored)
}

#[expect(
    clippy::undocumented_unsafe_blocks,
    reason = "the aligned destination is valid for the exact ROM flash read length"
)]
fn read_flash(offset: u32, destination: &mut [u8]) -> i32 {
    unsafe {
        esp_rom_spiflash_read(
            offset,
            destination.as_mut_ptr().cast::<u32>() as *const u32,
            destination.len() as u32,
        )
    }
}

#[expect(
    clippy::undocumented_unsafe_blocks,
    reason = "the aligned source is valid for the exact ROM flash write length"
)]
fn write_flash(offset: u32, source: &[u8]) -> Result<(), EspBleIdentityError> {
    let result = unsafe {
        esp_rom_spiflash_write(offset, source.as_ptr().cast::<u32>(), source.len() as u32)
    };
    if result == ESP_ROM_SPIFLASH_RESULT_OK {
        Ok(())
    } else {
        Err(EspBleIdentityError::Write(result))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EspBleIdentityError {
    Read(i32),
    Write(i32),
    Stored(PersistedBleIdentityError),
    Verification,
}

impl core::fmt::Display for EspBleIdentityError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Read(code) => write!(formatter, "BLE identity flash read failed with {code}"),
            Self::Write(code) => write!(formatter, "BLE identity flash write failed with {code}"),
            Self::Stored(error) => error.fmt(formatter),
            Self::Verification => formatter.write_str("BLE identity flash verification failed"),
        }
    }
}

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
    "C6 serve_slot_task pool_size must equal bluetooth_auto::SLOTS"
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
    ble_identity: BleIdentity,
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
