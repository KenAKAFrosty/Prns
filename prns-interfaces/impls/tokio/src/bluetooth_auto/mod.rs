#[cfg(feature = "bluetooth-auto")]
mod host;
#[cfg(all(feature = "bluetooth-auto", target_os = "linux"))]
mod linux;
mod runtime;

#[cfg(feature = "bluetooth-auto")]
pub use host::{
    AttachedBle, AttachedBluetoothLe, AutoBle, AutoBluetoothLe, ConfiguredAutoBle,
    ConfiguredAutoBluetoothLe,
};
#[cfg(all(feature = "bluetooth-auto", target_os = "ios"))]
pub use host::{CoreBluetoothRestorationIdentifiers, CoreBluetoothRestorationIdentifiersError};
#[cfg(all(
    feature = "bluetooth-auto",
    any(target_os = "macos", target_os = "ios")
))]
pub use host::{PreparedAutoBle, PreparedAutoBluetoothLe};
#[cfg(all(feature = "bluetooth-auto", target_os = "linux"))]
pub use linux::{BluerBackend, BluerError};
pub use runtime::{BluetoothAuto, BluetoothAutoStatus, BluetoothPeer};
