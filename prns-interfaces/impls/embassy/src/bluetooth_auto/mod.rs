mod runtime;

#[cfg(feature = "bluetooth-auto-trouble")]
mod trouble;

pub use runtime::{BluetoothAuto, BluetoothAutoShared, BluetoothAutoStatus, BluetoothMemberStatus};

#[cfg(feature = "bluetooth-auto-trouble")]
pub use trouble::{
    acceptor, columba_identity_uuid, columba_rx_uuid, columba_tx_uuid, control_uuid, data_uuid,
    dialer, host_runner, reticulum_attribute_table, serve_slot, service_uuid, BleHub, Closed,
    EmbeddedBleBackend, EmbeddedBleLink, EmbeddedBleSink, EmbeddedBleSource, GattCharacteristic,
    GattServer, ReticulumAttributeTable, ReticulumGattCharacteristics, ReticulumGattUuids,
    TroubleController, TroubleStack, TroubleTransport, CONNECTIONS, GATT_VALUE_CAP,
    HCI_COMMAND_SLOTS, L2CAP_CHANNELS, L2CAP_PSM, SLOTS,
};
