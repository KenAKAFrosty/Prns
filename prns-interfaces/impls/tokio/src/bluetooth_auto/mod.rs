mod host;
#[cfg(target_os = "linux")]
mod linux;
mod runtime;

#[cfg(test)]
mod ba_sim;

pub use host::{
    AttachedBle, AttachedBluetoothLe, AutoBle, AutoBluetoothLe, ConfiguredAutoBle,
    ConfiguredAutoBluetoothLe,
};
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub use host::{PreparedAutoBle, PreparedAutoBluetoothLe};
#[cfg(target_os = "linux")]
pub use linux::{BluerBackend, BluerError};
pub use runtime::{BluetoothAuto, BluetoothAutoStatus, BluetoothPeer};
