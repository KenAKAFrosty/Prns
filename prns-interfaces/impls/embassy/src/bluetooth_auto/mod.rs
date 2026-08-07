pub mod connection_slots;
#[cfg(feature = "bluetooth-auto-trouble")]
mod cooperative_transport;
mod frame_pool;
mod runtime;

#[cfg(feature = "bluetooth-auto-trouble")]
mod trouble;

pub use frame_pool::{FrameLease, FramePoolError, SharedFramePool};
pub use runtime::{
    BluetoothAuto, BluetoothAutoShared, BluetoothAutoStatus, BluetoothMemberStatus,
    BluetoothRecoveryReason,
};

#[cfg(feature = "bluetooth-auto-trouble")]
pub use cooperative_transport::CooperativeTransport;
#[cfg(feature = "bluetooth-auto-trouble")]
pub use trouble::{
    acceptor, columba_identity_uuid, columba_rx_uuid, columba_tx_uuid, control_uuid, data_uuid,
    dialer, host_runner, reticulum_attribute_table, serve_slot, service_uuid, BleHub, Closed,
    EmbeddedBleBackend, EmbeddedBleLink, EmbeddedBleSink, EmbeddedBleSource, GattCharacteristic,
    GattServer, ReticulumAttributeTable, ReticulumGattCharacteristics, ReticulumGattUuids,
    TroubleController, TroubleStack, TroubleTransport, GATT_VALUE_CAP, L2CAP_PSM, PEER_CAPACITY,
};
