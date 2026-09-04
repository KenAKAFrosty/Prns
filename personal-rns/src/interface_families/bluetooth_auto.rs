#[cfg(feature = "tokio-host")]
pub use prns_interfaces_tokio::bluetooth_auto::{
    AttachedBle, AttachedBluetoothLe, AutoBle, AutoBluetoothLe, BluetoothAuto, BluetoothAutoStatus,
    BluetoothPeer, ConfiguredAutoBle, ConfiguredAutoBluetoothLe,
};

#[cfg(all(feature = "tokio-host", target_os = "ios"))]
pub use prns_interfaces_tokio::bluetooth_auto::{
    CoreBluetoothCentralRestorationIdentifier, CoreBluetoothRestorationIdentifiers,
    CoreBluetoothRestorationIdentifiersError,
};

#[cfg(all(feature = "embassy-host", not(feature = "tokio-host")))]
pub use prns_interfaces_embassy::bluetooth_auto::{
    connection_slots, BluetoothAuto, BluetoothAutoShared, BluetoothAutoStatus,
    BluetoothMemberStatus, BluetoothRecoveryCounters, BluetoothRecoveryReason, FrameLease,
    FramePoolError, SharedFramePool,
};
