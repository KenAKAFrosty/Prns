#[cfg(feature = "tokio-host")]
pub use prns_interfaces_tokio::bluetooth_auto::{
    AttachedBle, AutoBle, BluetoothAuto, BluetoothAutoStatus, BluetoothPeer, ConfiguredAutoBle,
};

#[cfg(all(feature = "embassy-host", not(feature = "tokio-host")))]
pub use prns_interfaces_embassy::bluetooth_auto::{
    connection_slots, BluetoothAuto, BluetoothAutoShared, BluetoothAutoStatus,
    BluetoothMemberStatus,
};
