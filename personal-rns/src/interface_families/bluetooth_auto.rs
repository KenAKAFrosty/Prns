#[cfg(feature = "tokio-host")]
pub use prns_interfaces_tokio::bluetooth_auto::{
    AttachedBle, AttachedBluetoothLe, AutoBle, AutoBluetoothLe, BluetoothAuto, BluetoothAutoStatus,
    BluetoothPeer, ConfiguredAutoBle, ConfiguredAutoBluetoothLe,
};

#[cfg(all(feature = "embassy-host", not(feature = "tokio-host")))]
pub use prns_interfaces_embassy::bluetooth_auto::{
    connection_slots, BluetoothAuto, BluetoothAutoShared, BluetoothAutoStatus,
    BluetoothMemberStatus, BluetoothRecoveryReason, FrameLease, FramePoolError, SharedFramePool,
};
