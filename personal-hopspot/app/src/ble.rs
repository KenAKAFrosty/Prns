use embassy_futures::join::join;
use embassy_sync_07::blocking_mutex::raw::NoopRawMutex;
use esp_radio::ble::controller::BleConnector;
use heapless_09::Vec as GattVec;
use personal_rns::interfaces::bluetooth_auto::core::{
    encode_advertisement, BLE_SERVICE_UUID_BYTES, MAX_ADVERTISEMENT_LEN,
};
use trouble_host::prelude::*;

const HCI_COMMAND_SLOTS: usize = 20;
const CONNECTIONS: usize = 1;
const L2CAP_CHANNELS: usize = 2;
const ATTRIBUTE_TABLE: usize = 32;
const CCCD_TABLE: usize = 4;
const GATT_VALUE_CAP: usize = 244;

const CONTROL_UUID_LAST: u8 = 0xe7;
const DATA_UUID_LAST: u8 = 0xe8;

fn reticulum_uuid(last: u8) -> Uuid {
    let mut bytes = BLE_SERVICE_UUID_BYTES;
    bytes[15] = last;
    Uuid::from(u128::from_be_bytes(bytes))
}

pub async fn run(connector: BleConnector<'static>, mac: [u8; 6]) {
    let controller = ExternalController::<_, HCI_COMMAND_SLOTS>::new(connector);
    let mut resources: HostResources<DefaultPacketPool, CONNECTIONS, L2CAP_CHANNELS> =
        HostResources::new();

    let mut address = mac;
    address[5] |= 0b1100_0000;
    let stack =
        trouble_host::new(controller, &mut resources).set_random_address(Address::random(address));
    let Host {
        mut peripheral,
        mut runner,
        ..
    } = stack.build();

    let mut control_store = [0u8; GATT_VALUE_CAP];
    let mut data_store = [0u8; GATT_VALUE_CAP];
    let mut table: AttributeTable<NoopRawMutex, ATTRIBUTE_TABLE> = AttributeTable::new();
    if let Err(error) = GapConfig::Peripheral(PeripheralConfig {
        name: "Prns",
        appearance: &appearance::UNKNOWN,
    })
    .build(&mut table)
    {
        log::warn!("ble gap config failed: {error}");
        return;
    }
    let props = [
        CharacteristicProp::Write,
        CharacteristicProp::WriteWithoutResponse,
        CharacteristicProp::Notify,
    ];
    let (control, data) = {
        let mut service = table.add_service(Service::new(reticulum_uuid(0xe3)));
        let control = service
            .add_characteristic(
                reticulum_uuid(CONTROL_UUID_LAST),
                &props[..],
                GattVec::<u8, GATT_VALUE_CAP>::new(),
                &mut control_store,
            )
            .build();
        let data = service
            .add_characteristic(
                reticulum_uuid(DATA_UUID_LAST),
                &props[..],
                GattVec::<u8, GATT_VALUE_CAP>::new(),
                &mut data_store,
            )
            .build();
        service.build();
        (control, data)
    };
    let server: AttributeServer<
        NoopRawMutex,
        DefaultPacketPool,
        ATTRIBUTE_TABLE,
        CCCD_TABLE,
        CONNECTIONS,
    > = AttributeServer::new(table);

    let mut adv_data = [0u8; MAX_ADVERTISEMENT_LEN];
    let adv_len = encode_advertisement(&mut adv_data).expect("advertisement fits");

    let serve = async {
        loop {
            let advertiser = match peripheral
                .advertise(
                    &AdvertisementParameters::default(),
                    Advertisement::ConnectableScannableUndirected {
                        adv_data: &adv_data[..adv_len],
                        scan_data: &[],
                    },
                )
                .await
            {
                Ok(advertiser) => advertiser,
                Err(error) => {
                    log::warn!("ble advertise failed: {error:?}");
                    continue;
                }
            };
            log::info!("ble advertising connectable, awaiting a central");
            let connection = match advertiser.accept().await {
                Ok(connection) => connection,
                Err(error) => {
                    log::warn!("ble accept failed: {error:?}");
                    continue;
                }
            };
            let connection = match connection.with_attribute_server(&server) {
                Ok(connection) => connection,
                Err(error) => {
                    log::warn!("ble attribute server bind failed: {error:?}");
                    continue;
                }
            };
            log::info!("ble central connected");
            loop {
                match connection.next().await {
                    GattConnectionEvent::Disconnected { reason } => {
                        log::info!("ble central disconnected: {reason:?}");
                        break;
                    }
                    GattConnectionEvent::Gatt { event } => {
                        if let GattEvent::Write(write) = &event {
                            let lane = if write.handle() == control.handle {
                                "control"
                            } else if write.handle() == data.handle {
                                "data"
                            } else {
                                "other"
                            };
                            log::info!("ble write on {lane}");
                        }
                        match event.accept() {
                            Ok(reply) => reply.send().await,
                            Err(error) => log::warn!("ble gatt reply failed: {error:?}"),
                        }
                    }
                    _ => {}
                }
            }
        }
    };

    let host = async {
        loop {
            if let Err(error) = runner.run().await {
                log::warn!("ble host runner exited: {error:?}");
            }
        }
    };

    join(host, serve).await;
}
